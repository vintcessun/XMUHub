//! Accounts: email verification codes, argon2id passwords, cookie sessions, roles.

use std::collections::HashMap;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};

use super::{Hub, State, Viewer, clean};
use crate::db::Tx;
use crate::error::{Error, Result, bad};
use crate::model::*;

pub const SESSION_TTL: i64 = 30 * 86400;
// Some mailboxes (greylisting) receive the mail 10+ minutes late; the code must outlive that.
const CODE_TTL: i64 = 30 * 60;
const CODE_COOLDOWN: i64 = 60;
const CODE_MAX_ATTEMPTS: u32 = 5;
const CODES_PER_EMAIL_DAY: u32 = 10;
const CODES_PER_IP_HOUR: u32 = 20;
/// Whole site, per day: keeps a flood of sign-ups from using up the mail service's quota.
const CODES_PER_DAY: u32 = 600;
const LOGIN_FAILS_WINDOW: i64 = 15 * 60;
const LOGIN_FAILS_MAX: u32 = 8;
/// Successful sign-ins per account per hour (each one makes a session).
const LOGINS_PER_HOUR: u32 = 30;
/// Sessions kept per account; signing in again drops the oldest.
const SESSIONS_PER_USER: usize = 20;
/// Password hashes computed at once. Each takes ~20 MB and a blocking thread for a moment;
/// more than this means someone is hammering the sign-in form.
const HASHES_AT_ONCE: u32 = 3;
/// In-memory counters are pruned of stale entries once they grow past this.
const COUNTER_CAP: usize = 20_000;

/// Mail providers people actually use (plus the school's own, from the site config). Signing
/// up with any other domain (or a `+tag` alias) needs a human check first (Cloudflare Turnstile).
const COMMON_EMAIL_DOMAINS: &[&str] = &[
    "qq.com", "vip.qq.com", "foxmail.com", "163.com", "vip.163.com", "126.com", "vip.126.com", "yeah.net", "188.com",
    "sina.com", "sina.cn", "vip.sina.com", "sohu.com", "139.com", "189.cn", "wo.cn", "aliyun.com", "88.com",
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "live.cn", "msn.com", "icloud.com",
    "me.com", "mac.com", "yahoo.com", "proton.me", "protonmail.com",
];

/// Whether signing up with `email` (already normalized) needs a human check first.
pub fn needs_captcha(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return true };
    let common = COMMON_EMAIL_DOMAINS.contains(&domain) || crate::site::is_school_domain(domain);
    !common || local.contains('+')
}

/// Names nobody but staff may pick: they would pass for the site or its staff.
/// The site's own name (from the site config) is reserved as well.
const RESERVED_NICKNAMES: &[&str] = &["管理员", "审核员", "站长", "官方", "客服", "admin", "administrator", "moderator", "system", "xmuhub"];

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

/// Built-in account used by automation; never subject to the admin list.
pub const SYSTEM_EMAIL: &str = "system@xmuhub.local";

pub(super) struct AuthState {
    /// The admin list (maintained in a file outside the repo). Admin is granted only by it.
    admins: RwLock<Vec<String>>,
    codes: Mutex<HashMap<(String, CodePurpose), Code>>,
    /// email → (day, codes sent)
    per_email: Mutex<HashMap<String, (i64, u32)>>,
    /// ip → (hour, codes sent)
    per_ip: Mutex<HashMap<String, (i64, u32)>>,
    /// email or ip → (window start, failures)
    fails: Mutex<HashMap<String, (i64, u32)>>,
    /// (day, codes sent site-wide)
    per_day: Mutex<(i64, u32)>,
    /// user → (hour, successful sign-ins)
    logins: Mutex<HashMap<Id, (i64, u32)>>,
    /// password hashes being computed right now
    hashing: Mutex<u32>,
}

/// Holds one of the `HASHES_AT_ONCE` slots while a password hash is computed.
struct HashSlot<'a>(&'a Mutex<u32>);
impl Drop for HashSlot<'_> {
    fn drop(&mut self) {
        *self.0.lock() -= 1;
    }
}

/// Drops entries whose window is over once a counter map gets big (keys are client IPs,
/// emails …, so an attacker could otherwise grow it without bound).
fn prune<K: std::hash::Hash + Eq, V>(m: &mut HashMap<K, V>, live: impl Fn(&V) -> bool) {
    if m.len() > COUNTER_CAP {
        m.retain(|_, v| live(v));
    }
}

impl AuthState {
    pub(super) fn new(admins: Vec<String>) -> AuthState {
        AuthState {
            admins: RwLock::new(normalize_list(admins)),
            codes: Mutex::new(HashMap::new()),
            per_email: Mutex::new(HashMap::new()),
            per_ip: Mutex::new(HashMap::new()),
            fails: Mutex::new(HashMap::new()),
            per_day: Mutex::new((0, 0)),
            logins: Mutex::new(HashMap::new()),
            hashing: Mutex::new(0),
        }
    }

    fn hash_slot(&self) -> Result<HashSlot<'_>> {
        let mut n = self.hashing.lock();
        if *n >= HASHES_AT_ONCE {
            return Err(Error::TooMany("登录的人太多了，请稍后再试".into()));
        }
        *n += 1;
        Ok(HashSlot(&self.hashing))
    }

    fn too_many_fails(&self, keys: &[&str]) -> bool {
        let now = now();
        let f = self.fails.lock();
        keys.iter().any(|k| f.get(*k).is_some_and(|(start, n)| now - start < LOGIN_FAILS_WINDOW && *n >= LOGIN_FAILS_MAX))
    }

    fn record_fail(&self, keys: &[&str]) {
        let now = now();
        let mut f = self.fails.lock();
        prune(&mut f, |(start, _)| now - start < LOGIN_FAILS_WINDOW);
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

    /// Takes back a failure counted ahead of a check that then succeeded.
    fn unrecord_fail(&self, keys: &[&str]) {
        let mut f = self.fails.lock();
        for k in keys {
            if let Some(e) = f.get_mut(*k) {
                e.1 = e.1.saturating_sub(1);
            }
        }
    }
}

fn normalize_list(list: Vec<String>) -> Vec<String> {
    let mut v: Vec<String> = list.into_iter().map(|a| a.trim().to_lowercase()).filter(|a| a.contains('@')).collect();
    v.sort();
    v.dedup();
    v
}

pub struct Registration {
    pub email: String,
    pub code: String,
    pub password: String,
    pub nickname: String,
}

pub(super) fn sha(s: &str) -> [u8; 32] {
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

fn clean_nickname(n: &str, staff: bool) -> Result<String> {
    let n = clean(n, 20);
    // Single-character nicknames are fine (澈); only whitespace-only is rejected.
    if n.is_empty() {
        return Err(bad("请填写昵称"));
    }
    let folded: String = n.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let site_name = crate::site::get().name.to_lowercase();
    if !staff && (RESERVED_NICKNAMES.iter().any(|r| folded.contains(r)) || (!site_name.is_empty() && folded.contains(site_name.as_str()))) {
        return Err(bad("这个昵称容易被误认为本站或工作人员，换一个吧"));
    }
    Ok(n)
}

impl Hub {
    // ------------------------------------------------------------ verification codes

    /// Issues a 6-digit code for `email` and returns it for the caller to mail. The caller
    /// checks the human test (`needs_captcha`) before this for sign-ups that need one.
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
        if let Some(c) = a.codes.lock().get(&(email.clone(), purpose))
            && now - c.sent_at < CODE_COOLDOWN
        {
            return Err(Error::TooMany(format!("请 {} 秒后再获取验证码", CODE_COOLDOWN - (now - c.sent_at))));
        }
        {
            let day = now / 86400;
            let d = a.per_day.lock();
            if d.0 == day && d.1 >= CODES_PER_DAY {
                return Err(Error::TooMany("今天发出的验证码已经太多了，请明天再来，或联系管理员".into()));
            }
        }
        {
            let hour = now / 3600;
            let mut m = a.per_ip.lock();
            prune(&mut m, |(h, _)| *h == hour);
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
            prune(&mut m, |(d, _)| *d == day);
            let e = m.entry(email.clone()).or_insert((day, 0));
            if e.0 != day {
                *e = (day, 0);
            }
            if e.1 >= CODES_PER_EMAIL_DAY {
                return Err(Error::TooMany("这个邮箱今天获取验证码的次数太多了".into()));
            }
            e.1 += 1;
        }
        {
            let day = now / 86400;
            let mut d = a.per_day.lock();
            if d.0 != day {
                *d = (day, 0);
            }
            d.1 += 1;
        }
        let code = format!("{:06}", u32::from_le_bytes(rand::random::<[u8; 4]>()) % 1_000_000);
        let mut codes = a.codes.lock();
        prune(&mut codes, |c| c.expires > now);
        codes.insert((email.clone(), purpose), Code { hash: sha(&code), expires: now + CODE_TTL, sent_at: now, attempts: 0 });
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
        // Keep the newest few per account: signing in in a loop can't pile sessions up.
        let mut mine: Vec<(i64, [u8; 32])> = st.sessions.values().filter(|s| s.user == user).map(|s| (s.created_at, s.hash)).collect();
        if mine.len() >= SESSIONS_PER_USER {
            mine.sort_unstable();
            for (_, h) in &mine[..=mine.len() - SESSIONS_PER_USER] {
                st.sessions.remove(h);
                tx.del_session(h)?;
            }
        }
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

    /// Admin comes only from the list: listed → Admin, unlisted admin → Reviewer.
    fn role_for(&self, email: &str, current: Level) -> Level {
        if email == SYSTEM_EMAIL {
            return current;
        }
        let listed = self.auth.admins.read().iter().any(|a| a == email);
        match (listed, current) {
            (true, _) => Level::Admin,
            (false, Level::Admin) => Level::Reviewer,
            (false, l) => l,
        }
    }

    pub fn admin_list(&self) -> Vec<String> {
        self.auth.admins.read().clone()
    }

    /// Replaces the admin list and applies it to existing accounts. Returns how many changed.
    pub fn set_admins(&self, list: Vec<String>) -> Result<usize> {
        let list = normalize_list(list);
        if *self.auth.admins.read() == list {
            return Ok(0);
        }
        *self.auth.admins.write() = list;
        let changes: Vec<(Id, Level)> = {
            let st = self.st.read();
            st.users.values().filter_map(|u| {
                let want = self.role_for(&u.email, u.level);
                (want != u.level).then_some((u.id, want))
            }).collect()
        };
        if changes.is_empty() {
            return Ok(0);
        }
        self.mutate(|st, tx| {
            for (id, level) in &changes {
                let mut u = st.users[id].clone();
                tracing::info!(email = %u.email, from = u.level as u8, to = *level as u8, "admin list applied");
                u.level = *level;
                st.put_user(tx, u)?;
            }
            Ok(changes.len())
        })
    }

    pub fn register(&self, r: Registration, ip: &str) -> Result<(String, User)> {
        let email = normalize_email(&r.email)?;
        check_password(&r.password)?;
        // Someone on the admin list is staff from the start and may use a staff-looking name.
        let level = self.role_for(&email, Level::Contributor);
        let nickname = clean_nickname(&r.nickname, level >= Level::Reviewer)?;
        if self.st.read().user_by_email.contains_key(&email) {
            return Err(Error::Conflict("这个邮箱已经注册过了".into()));
        }
        self.take_code(&email, CodePurpose::Register, &r.code)?;
        let password = {
            let _slot = self.auth.hash_slot()?;
            hash_password(&r.password)?
        };
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
        let slot = self.auth.hash_slot()?;
        // Counted before the (slow) check and taken back on success, so a burst of guesses
        // can't all get past the limit while their hashes are still being computed.
        self.auth.record_fail(&[&email, &ip_key]);
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
        drop(slot);
        let Some(user) = user.filter(|_| ok) else {
            return Err(bad("邮箱或密码不正确"));
        };
        self.auth.unrecord_fail(&[&ip_key]);
        if user.banned {
            return Err(Error::Forbidden);
        }
        self.auth.clear_fails(&email);
        {
            let hour = now() / 3600;
            let mut m = self.auth.logins.lock();
            prune(&mut m, |(h, _)| *h == hour);
            let e = m.entry(user.id).or_insert((hour, 0));
            if e.0 != hour {
                *e = (hour, 0);
            }
            if e.1 >= LOGINS_PER_HOUR {
                return Err(Error::TooMany("这个账号登录太频繁了，请稍后再试".into()));
            }
            e.1 += 1;
        }
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

    /// Revokes every personal API token of `user` (after a password change or reset: a token
    /// made by whoever had the old password must stop working too).
    fn drop_tokens(st: &mut State, tx: &Tx, user: Id) -> Result<()> {
        let doomed: Vec<Id> = st.tokens.values().filter(|t| t.user == user).map(|t| t.id).collect();
        for id in doomed {
            if let Some(t) = st.tokens.remove(&id) {
                st.token_by_hash.remove(&t.hash);
            }
            tx.del_token(id)?;
        }
        Ok(())
    }

    pub fn reset_password(&self, email: &str, code: &str, password: &str, ip: &str) -> Result<(String, User)> {
        let email = normalize_email(email)?;
        check_password(password)?;
        let id = *self.st.read().user_by_email.get(&email).ok_or_else(|| bad("这个邮箱还没有注册"))?;
        self.take_code(&email, CodePurpose::Reset, code)?;
        let hash = {
            let _slot = self.auth.hash_slot()?;
            hash_password(password)?
        };
        self.auth.clear_fails(&email);
        self.mutate(|st, tx| {
            let mut u = st.users[&id].clone();
            u.password = hash;
            st.put_user(tx, u.clone())?;
            // A reset signs out every other device and revokes API tokens.
            Self::drop_sessions(st, tx, id, None)?;
            Self::drop_tokens(st, tx, id)?;
            let secret = Self::new_session(st, tx, id, ip)?;
            Ok((secret, u))
        })
    }

    pub fn update_profile(&self, me: &User, session: &str, nickname: Option<&str>, old_password: Option<&str>, new_password: Option<&str>) -> Result<User> {
        let staff = me.level >= Level::Reviewer;
        let nickname = nickname.map(|n| clean_nickname(n, staff)).transpose()?;
        let new_hash = match new_password {
            Some(p) => {
                check_password(p)?;
                let _slot = self.auth.hash_slot()?;
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
                Self::drop_tokens(st, tx, u.id)?;
            }
            st.put_user(tx, u.clone())?;
            Ok(u)
        })
    }

    /// 注销账号: the owner (password re-entered) erases what identifies them: email, nickname,
    /// password, sign-up IP, avatar, sessions and tokens; the nickname is no longer shown on
    /// their files and their collections turn private. The account id stays (files, review
    /// records and ratings point at it) but can never sign in again; the address is free to
    /// register afresh. Admins are defined by the list, so they ask to be taken off it first.
    pub fn delete_account(&self, me: &User, password: &str) -> Result<()> {
        if me.level == Level::Admin {
            return Err(bad("管理员请先让其他管理员把你移出管理员名单，再注销"));
        }
        {
            let _slot = self.auth.hash_slot()?;
            if !verify_password(password, &me.password) {
                return Err(bad("密码不正确"));
            }
        }
        self.mutate(|st, tx| {
            let mut u = st.users.get(&me.id).cloned().ok_or(Error::NotFound("用户"))?;
            st.user_by_email.remove(&u.email);
            u.email = format!("deleted-{}@deleted.invalid", u.id);
            u.nickname = "已注销用户".into();
            u.password = String::new();
            u.created_ip = String::new();
            u.banned = true;
            st.put_user(tx, u)?;
            Self::drop_sessions(st, tx, me.id, None)?;
            Self::drop_tokens(st, tx, me.id)?;
            if let Some(a) = st.avatars.get_mut(&me.id) {
                a.current.clear();
                a.pending.clear();
                tx.put_avatar(a)?;
            }
            if st.public_uploaders.remove(&me.id) {
                tx.put_public_uploader(me.id, false)?;
            }
            for c in st.collections.values_mut().filter(|c| c.user == me.id && c.status != "private") {
                c.status = "private".into();
                tx.put_collection(c)?;
            }
            Ok(())
        })
    }

    // ------------------------------------------------------------ roles

    pub fn users(&self, actor: Viewer, q: &str) -> Result<Vec<User>> {
        // Accounts, complaints, feedback and the full review log are for admins only.
        let me = actor.at_least(Level::Admin)?;
        let q = q.trim().to_lowercase();
        let st = self.st.read();
        let mut v: Vec<User> = st
            .users
            .values()
            .filter(|u| u.email != SYSTEM_EMAIL)
            .filter(|u| me.level == Level::Admin || u.level < Level::Reviewer)
            .filter(|u| q.is_empty() || u.email.contains(&q) || u.nickname.to_lowercase().contains(&q))
            .cloned()
            .collect();
        v.sort_by_key(|u| std::cmp::Reverse(u.id));
        v.truncate(500);
        Ok(v)
    }

    pub fn update_user(&self, actor: Viewer, id: Id, level: Option<Level>, banned: Option<bool>) -> Result<User> {
        let me = actor.at_least(Level::Admin)?.clone();
        self.mutate(|st, tx| {
            let mut u = st.users.get(&id).cloned().ok_or(Error::NotFound("用户"))?;
            if u.id == me.id {
                return Err(bad("不能修改自己的权限"));
            }
            // Admins are defined by the maintained list only, never by clicks.
            if level == Some(Level::Admin) || (u.level == Level::Admin && level.is_some()) {
                return Err(bad("管理员由名单统一维护，请修改管理员名单"));
            }
            // Peers are equals: nobody bans or re-ranks an account at or above their own level
            // (admins can't ban admins, reviewers can't touch reviewers).
            if u.level >= me.level {
                return Err(bad("不能修改同级或更高级别的账号"));
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
        const EMAIL: &str = SYSTEM_EMAIL;
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
