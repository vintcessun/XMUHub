//! Accounts: email verification codes, argon2id passwords, cookie sessions, roles.

use std::collections::HashMap;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use super::{Hub, State, Viewer, clean};
use crate::db::Tx;
use crate::error::{Error, Result, bad};
use crate::model::*;

pub const SESSION_TTL: i64 = 30 * 86400;
const CODE_TTL: i64 = 10 * 60;
const CODE_COOLDOWN: i64 = 60;
const CODE_MAX_ATTEMPTS: u32 = 5;
const CODES_PER_EMAIL_DAY: u32 = 10;
const CODES_PER_IP_HOUR: u32 = 20;
const LOGIN_FAILS_WINDOW: i64 = 15 * 60;
const LOGIN_FAILS_MAX: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodePurpose {
    Register,
    Reset,
}

struct Code {
    hash: [u8; 32],
    expires: i64,
    sent_at: i64,
    attempts: u32,
}

pub(super) struct AuthState {
    admins: Vec<String>,
    codes: Mutex<HashMap<(String, CodePurpose), Code>>,
    /// email → (day, codes sent)
    per_email: Mutex<HashMap<String, (i64, u32)>>,
    /// ip → (hour, codes sent)
    per_ip: Mutex<HashMap<String, (i64, u32)>>,
    /// email or ip → (window start, failures)
    fails: Mutex<HashMap<String, (i64, u32)>>,
}

impl AuthState {
    pub(super) fn new(admins: Vec<String>) -> AuthState {
        AuthState {
            admins: admins.into_iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect(),
            codes: Mutex::new(HashMap::new()),
            per_email: Mutex::new(HashMap::new()),
            per_ip: Mutex::new(HashMap::new()),
            fails: Mutex::new(HashMap::new()),
        }
    }

    fn too_many_fails(&self, keys: &[&str]) -> bool {
        let now = now();
        let f = self.fails.lock();
        keys.iter().any(|k| f.get(*k).is_some_and(|(start, n)| now - start < LOGIN_FAILS_WINDOW && *n >= LOGIN_FAILS_MAX))
    }

    fn record_fail(&self, keys: &[&str]) {
        let now = now();
        let mut f = self.fails.lock();
        for k in keys {
            let e = f.entry(k.to_string()).or_insert((now, 0));
            if now - e.0 >= LOGIN_FAILS_WINDOW {
                *e = (now, 0);
            }
            e.1 += 1;
        }
    }

    fn clear_fails(&self, key: &str) {
        self.fails.lock().remove(key);
    }
}

pub struct Registration {
    pub email: String,
    pub code: String,
    pub password: String,
    pub nickname: String,
}

fn sha(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

pub fn normalize_email(e: &str) -> Result<String> {
    let e = e.trim().to_lowercase();
    let ok = e.len() <= 100
        && e.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && e.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
        });
    if ok { Ok(e) } else { Err(bad("邮箱格式不正确")) }
}

fn check_password(p: &str) -> Result<()> {
    let n = p.chars().count();
    if !(8..=72).contains(&n) {
        return Err(bad("密码长度需要 8–72 位"));
    }
    if p.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad("密码不能全是数字"));
    }
    Ok(())
}

fn hash_password(p: &str) -> Result<String> {
    Argon2::default()
        .hash_password(p.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("argon2: {e}")))
}

fn verify_password(p: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|h| Argon2::default().verify_password(p.as_bytes(), &h).is_ok())
}

fn clean_nickname(n: &str) -> Result<String> {
    let n = clean(n, 20);
    if n.chars().count() < 2 {
        return Err(bad("昵称至少两个字"));
    }
    Ok(n)
}

impl Hub {
    // ------------------------------------------------------------ verification codes

    /// Issues a 6-digit code for `email` and returns it for the caller to mail.
    pub fn request_code(&self, email: &str, purpose: CodePurpose, ip: &str) -> Result<(String, String)> {
        let email = normalize_email(email)?;
        let exists = self.st.read().user_by_email.contains_key(&email);
        match purpose {
            CodePurpose::Register if exists => return Err(Error::Conflict("这个邮箱已经注册过了，请直接登录或找回密码".into())),
            CodePurpose::Reset if !exists => return Err(bad("这个邮箱还没有注册")),
            _ => {}
        }
        let now = now();
        let a = &self.auth;
        if let Some(c) = a.codes.lock().get(&(email.clone(), purpose)) {
            if now - c.sent_at < CODE_COOLDOWN {
                return Err(Error::TooMany(format!("请 {} 秒后再获取验证码", CODE_COOLDOWN - (now - c.sent_at))));
            }
        }
        {
            let hour = now / 3600;
            let mut m = a.per_ip.lock();
            let e = m.entry(ip.to_string()).or_insert((hour, 0));
            if e.0 != hour {
                *e = (hour, 0);
            }
            if e.1 >= CODES_PER_IP_HOUR {
                return Err(Error::TooMany("请求太频繁，请稍后再试".into()));
            }
            e.1 += 1;
        }
        {
            let day = now / 86400;
            let mut m = a.per_email.lock();
            let e = m.entry(email.clone()).or_insert((day, 0));
            if e.0 != day {
                *e = (day, 0);
            }
            if e.1 >= CODES_PER_EMAIL_DAY {
                return Err(Error::TooMany("这个邮箱今天获取验证码的次数太多了".into()));
            }
            e.1 += 1;
        }
        let code = format!("{:06}", u32::from_le_bytes(rand::random::<[u8; 4]>()) % 1_000_000);
        a.codes.lock().insert((email.clone(), purpose), Code { hash: sha(&code), expires: now + CODE_TTL, sent_at: now, attempts: 0 });
        Ok((email, code))
    }

    /// Forgets a code whose email could not be sent, so the user can retry at once.
    pub fn cancel_code(&self, email: &str, purpose: CodePurpose) {
        self.auth.codes.lock().remove(&(email.to_string(), purpose));
    }

    fn take_code(&self, email: &str, purpose: CodePurpose, code: &str) -> Result<()> {
        let mut codes = self.auth.codes.lock();
        let key = (email.to_string(), purpose);
        let Some(c) = codes.get_mut(&key) else { return Err(bad("请先获取验证码")) };
        if c.expires < now() {
            codes.remove(&key);
            return Err(bad("验证码已过期，请重新获取"));
        }
        if c.hash != sha(code.trim()) {
            c.attempts += 1;
            if c.attempts >= CODE_MAX_ATTEMPTS {
                codes.remove(&key);
                return Err(bad("验证码错误次数过多，请重新获取"));
            }
            return Err(bad("验证码不正确"));
        }
        codes.remove(&key);
        Ok(())
    }

    // ------------------------------------------------------------ sessions

    fn new_session(st: &mut State, tx: &Tx, user: Id, ip: &str) -> Result<String> {
        let secret = hex::encode(rand::random::<[u8; 32]>());
        let s = Session { hash: sha(&secret), user, created_at: now(), expires_at: now() + SESSION_TTL, ip: clean(ip, 64) };
        tx.put_session(&s)?;
        st.sessions.insert(s.hash, s);
        Ok(secret)
    }

    /// The signed-in user for a session cookie value, if valid.
    pub fn session_user(&self, secret: &str) -> Option<User> {
        let st = self.st.read();
        let s = st.sessions.get(&sha(secret))?;
        if s.expires_at < now() {
            return None;
        }
        st.users.get(&s.user).filter(|u| !u.banned).cloned()
    }

    fn role_for(&self, email: &str, current: Level) -> Level {
        if self.auth.admins.iter().any(|a| a == email) { Level::Admin } else { current }
    }

    pub fn register(&self, r: Registration, ip: &str) -> Result<(String, User)> {
        let email = normalize_email(&r.email)?;
        check_password(&r.password)?;
        let nickname = clean_nickname(&r.nickname)?;
        if self.st.read().user_by_email.contains_key(&email) {
            return Err(Error::Conflict("这个邮箱已经注册过了".into()));
        }
        self.take_code(&email, CodePurpose::Register, &r.code)?;
        let password = hash_password(&r.password)?;
        let level = self.role_for(&email, Level::Contributor);
        self.mutate(|st, tx| {
            if st.user_by_email.contains_key(&email) {
                return Err(Error::Conflict("这个邮箱已经注册过了".into()));
            }
            let u = User {
                id: st.next_id(tx)?,
                email: email.clone(),
                nickname,
                password,
                level,
                banned: false,
                created_at: now(),
                created_ip: clean(ip, 64),
                last_login: now(),
                uploads: 0,
            };
            st.put_user(tx, u.clone())?;
            let secret = Self::new_session(st, tx, u.id, ip)?;
            Ok((secret, u))
        })
    }

    pub fn login(&self, email: &str, password: &str, ip: &str) -> Result<(String, User)> {
        let email = normalize_email(email)?;
        let ip_key = format!("ip:{ip}");
        if self.auth.too_many_fails(&[&email, &ip_key]) {
            return Err(Error::TooMany("登录失败次数太多，请 15 分钟后再试或找回密码".into()));
        }
        let user = {
            let st = self.st.read();
            st.user_by_email.get(&email).and_then(|id| st.users.get(id).cloned())
        };
        let ok = match &user {
            Some(u) => verify_password(password, &u.password),
            // Same work either way so response time doesn't reveal registered emails.
            None => {
                static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
                let dummy = DUMMY.get_or_init(|| hash_password("xmuhub-timing-dummy").unwrap_or_default());
                let _ = verify_password(password, dummy);
                false
            }
        };
        let Some(user) = user.filter(|_| ok) else {
            self.auth.record_fail(&[&email, &ip_key]);
            return Err(bad("邮箱或密码不正确"));
        };
        if user.banned {
            return Err(Error::Forbidden);
        }
        self.auth.clear_fails(&email);
        let level = self.role_for(&email, user.level);
        self.mutate(|st, tx| {
            let mut u = st.users[&user.id].clone();
            u.last_login = now();
            u.level = level;
            st.put_user(tx, u.clone())?;
            let secret = Self::new_session(st, tx, u.id, ip)?;
            Ok((secret, u))
        })
    }

    pub fn logout(&self, secret: &str) -> Result<()> {
        let h = sha(secret);
        self.mutate(|st, tx| {
            st.sessions.remove(&h);
            tx.del_session(&h)
        })
    }

    fn drop_sessions(st: &mut State, tx: &Tx, user: Id, keep: Option<[u8; 32]>) -> Result<()> {
        let doomed: Vec<[u8; 32]> = st.sessions.values().filter(|s| s.user == user && Some(s.hash) != keep).map(|s| s.hash).collect();
        for h in doomed {
            st.sessions.remove(&h);
            tx.del_session(&h)?;
        }
        Ok(())
    }

    pub fn reset_password(&self, email: &str, code: &str, password: &str, ip: &str) -> Result<(String, User)> {
        let email = normalize_email(email)?;
        check_password(password)?;
        let id = *self.st.read().user_by_email.get(&email).ok_or_else(|| bad("这个邮箱还没有注册"))?;
        self.take_code(&email, CodePurpose::Reset, code)?;
        let hash = hash_password(password)?;
        self.auth.clear_fails(&email);
        self.mutate(|st, tx| {
            let mut u = st.users[&id].clone();
            u.password = hash;
            st.put_user(tx, u.clone())?;
            // A reset signs out every other device.
            Self::drop_sessions(st, tx, id, None)?;
            let secret = Self::new_session(st, tx, id, ip)?;
            Ok((secret, u))
        })
    }

    pub fn update_profile(&self, me: &User, session: &str, nickname: Option<&str>, old_password: Option<&str>, new_password: Option<&str>) -> Result<User> {
        let nickname = nickname.map(clean_nickname).transpose()?;
        let new_hash = match new_password {
            Some(p) => {
                check_password(p)?;
                if !old_password.is_some_and(|o| verify_password(o, &me.password)) {
                    return Err(bad("原密码不正确"));
                }
                Some(hash_password(p)?)
            }
            None => None,
        };
        let keep = sha(session);
        self.mutate(|st, tx| {
            let mut u = st.users.get(&me.id).cloned().ok_or(Error::NotFound("用户"))?;
            if let Some(n) = nickname {
                u.nickname = n;
            }
            if let Some(h) = new_hash {
                u.password = h;
                Self::drop_sessions(st, tx, u.id, Some(keep))?;
            }
            st.put_user(tx, u.clone())?;
            Ok(u)
        })
    }

    // ------------------------------------------------------------ roles

    pub fn users(&self, actor: Viewer, q: &str) -> Result<Vec<User>> {
        let me = actor.at_least(Level::Reviewer)?;
        let q = q.trim().to_lowercase();
        let st = self.st.read();
        let mut v: Vec<User> = st
            .users
            .values()
            .filter(|u| me.level == Level::Admin || u.level < Level::Reviewer)
            .filter(|u| q.is_empty() || u.email.contains(&q) || u.nickname.to_lowercase().contains(&q))
            .cloned()
            .collect();
        v.sort_by_key(|u| std::cmp::Reverse(u.id));
        v.truncate(500);
        Ok(v)
    }

    pub fn update_user(&self, actor: Viewer, id: Id, level: Option<Level>, banned: Option<bool>) -> Result<User> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        self.mutate(|st, tx| {
            let mut u = st.users.get(&id).cloned().ok_or(Error::NotFound("用户"))?;
            if u.id == me.id {
                return Err(bad("不能修改自己的权限"));
            }
            if me.level < Level::Admin {
                // Reviewers manage contributors only, and cannot mint more reviewers.
                let ok = u.level < Level::Reviewer && level.is_none_or(|l| l < Level::Reviewer);
                if !ok {
                    return Err(Error::Forbidden);
                }
            }
            if let Some(l) = level {
                u.level = l;
            }
            if let Some(b) = banned {
                u.banned = b;
                if b {
                    Self::drop_sessions(st, tx, u.id, None)?;
                }
            }
            st.put_user(tx, u.clone())?;
            Ok(u)
        })
    }

    pub fn user(&self, id: Id) -> Option<User> {
        self.st.read().users.get(&id).cloned()
    }

    /// The built-in account automation (imports) acts as; created on first use.
    pub fn system_user(&self) -> Result<User> {
        const EMAIL: &str = "system@xmuhub.local";
        {
            let st = self.st.read();
            if let Some(u) = st.user_by_email.get(EMAIL).and_then(|id| st.users.get(id)) {
                return Ok(u.clone());
            }
        }
        self.mutate(|st, tx| {
            let u = User {
                id: st.next_id(tx)?,
                email: EMAIL.into(),
                nickname: "资料库导入".into(),
                // Not a valid PHC string: this account can never log in with a password.
                password: "!".into(),
                level: Level::Admin,
                banned: false,
                created_at: now(),
                created_ip: "system".into(),
                last_login: 0,
                uploads: 0,
            };
            st.put_user(tx, u.clone())?;
            Ok(u)
        })
    }

    /// Drops expired sessions (housekeeping).
    pub fn purge_sessions(&self) -> Result<usize> {
        let now = now();
        self.mutate(|st, tx| {
            let doomed: Vec<[u8; 32]> = st.sessions.values().filter(|s| s.expires_at < now).map(|s| s.hash).collect();
            for h in &doomed {
                st.sessions.remove(h);
                tx.del_session(h)?;
            }
            Ok(doomed.len())
        })
    }
}
