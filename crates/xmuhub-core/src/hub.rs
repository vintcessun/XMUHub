//! Domain logic. Reads are served from an in-memory catalog; every write goes through
//! `mutate`, which persists to redb in one transaction and then updates memory and search.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{Db, Tx};
use crate::error::{Error, Result, bad};
use crate::model::*;
use crate::search::{DocType, Filter, Search};
use crate::storage::{Receipt, Storage, UploadTarget};

const SEQ_KEY: &str = "seq";
/// Unfinished uploads are abandoned after this long.
pub const UPLOAD_TTL: i64 = 24 * 3600;

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_file: u64,
    pub max_part: u64,
    /// (files, bytes) per day for Contributor and Trusted.
    pub contributor_daily: (u32, u64),
    pub trusted_daily: (u32, u64),
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_file: 2 * 1024 * 1024 * 1024,
            // Cloudflare's free plan caps a request body at 100 MB; stay clear of it.
            max_part: 95 * 1024 * 1024,
            contributor_daily: (30, 2 * 1024 * 1024 * 1024),
            trusted_daily: (300, 20 * 1024 * 1024 * 1024),
        }
    }
}

#[derive(Default)]
struct State {
    seq: u64,
    courses: HashMap<Id, Course>,
    resources: HashMap<Id, Resource>,
    by_course: HashMap<Id, Vec<Id>>,
    tokens: HashMap<Id, Token>,
    token_by_hash: HashMap<[u8; 32], Id>,
    uploads: HashMap<Id, Upload>,
    blobs: HashMap<String, Blob>,
}

impl State {
    fn from_db(db: &Db) -> Result<State> {
        let snap = db.load()?;
        let mut st = State::default();
        for (k, v) in snap.meta {
            if k == SEQ_KEY && v.len() == 8 {
                st.seq = u64::from_le_bytes(v.try_into().unwrap());
            }
        }
        for c in snap.courses {
            st.courses.insert(c.id, c);
        }
        for r in snap.resources {
            st.by_course.entry(r.course_id).or_default().push(r.id);
            st.resources.insert(r.id, r);
        }
        for t in snap.tokens {
            st.token_by_hash.insert(t.hash, t.id);
            st.tokens.insert(t.id, t);
        }
        for u in snap.uploads {
            st.uploads.insert(u.id, u);
        }
        for b in snap.blobs {
            st.blobs.insert(b.key.clone(), b);
        }
        Ok(st)
    }

    fn next_id(&mut self, tx: &Tx) -> Result<Id> {
        self.seq += 1;
        tx.put_meta(SEQ_KEY, &self.seq.to_le_bytes())?;
        Ok(self.seq)
    }

    fn put_resource(&mut self, tx: &Tx, r: Resource) -> Result<()> {
        tx.put_resource(&r)?;
        if let Some(old) = self.resources.get(&r.id) {
            if old.course_id != r.course_id {
                if let Some(v) = self.by_course.get_mut(&old.course_id) {
                    v.retain(|&x| x != r.id);
                }
                self.by_course.entry(r.course_id).or_default().push(r.id);
            }
        } else {
            self.by_course.entry(r.course_id).or_default().push(r.id);
        }
        self.resources.insert(r.id, r);
        Ok(())
    }

    fn put_course(&mut self, tx: &Tx, c: Course) -> Result<()> {
        tx.put_course(&c)?;
        self.courses.insert(c.id, c);
        Ok(())
    }

    fn put_token(&mut self, tx: &Tx, t: Token) -> Result<()> {
        tx.put_token(&t)?;
        self.token_by_hash.insert(t.hash, t.id);
        self.tokens.insert(t.id, t);
        Ok(())
    }

    fn published_count(&self, course: Id) -> usize {
        self.by_course
            .get(&course)
            .map(|v| v.iter().filter(|id| self.resources[id].status == Status::Published).count())
            .unwrap_or(0)
    }

    fn course_listed(&self, c: &Course) -> bool {
        match c.status {
            CourseStatus::Merged(_) => false,
            CourseStatus::Active => true,
            CourseStatus::Pending => self.published_count(c.id) > 0,
        }
    }

    /// Follows merge links to the surviving course.
    fn resolve_course(&self, mut id: Id) -> Option<&Course> {
        for _ in 0..16 {
            let c = self.courses.get(&id)?;
            match c.status {
                CourseStatus::Merged(into) => id = into,
                _ => return Some(c),
            }
        }
        None
    }
}

/// Who is asking. `None` token = guest.
#[derive(Debug, Clone, Copy)]
pub struct Viewer<'a> {
    pub token: Option<&'a Token>,
}

impl Viewer<'_> {
    pub fn level(&self) -> Level {
        self.token.map(|t| t.level).unwrap_or(Level::Guest)
    }
    fn id(&self) -> Option<Id> {
        self.token.map(|t| t.id)
    }
    fn at_least(&self, l: Level) -> Result<&Token> {
        match self.token {
            Some(t) if t.level >= l => Ok(t),
            Some(_) => Err(Error::Forbidden),
            None => Err(Error::Unauthorized),
        }
    }
    fn can_see(&self, r: &Resource) -> bool {
        match r.status {
            Status::Published => true,
            Status::Removed => self.level() >= Level::Reviewer,
            Status::Pending | Status::Rejected => self.level() >= Level::Reviewer || self.id() == Some(r.uploader),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewCourse {
    pub code: String,
    pub name: String,
    pub college: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResourceInput {
    pub title: String,
    pub kind: String,
    pub year: Option<u16>,
    pub term: Option<u8>,
    #[serde(default)]
    pub teacher: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CoursePatch {
    pub code: Option<String>,
    pub name: Option<String>,
    pub college: Option<String>,
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PartSpec {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadPlan {
    pub upload_id: Id,
    /// True when identical content is already stored: nothing to send.
    pub dedup: bool,
    pub parts: Vec<PartPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PartPlan {
    pub index: usize,
    pub size: u64,
    pub done: bool,
    pub target: Option<UploadTarget>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadPlan {
    pub filename: String,
    pub size: u64,
    pub mime: String,
    pub parts: Vec<DownloadPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadPart {
    pub size: u64,
    pub sha256: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub courses: usize,
    pub resources: usize,
    pub pending: usize,
    pub tokens: usize,
    pub stored_bytes: u64,
    pub downloads: u64,
}

pub fn hash_token(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn check_hex64(s: &str) -> Result<()> {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(bad("sha256 必须是 64 位小写十六进制"))
    }
}

/// Content key of a file: its sha256, or for multi-part files `m` + sha256 of the
/// concatenated part hashes (so every part is verifiable on its own).
pub fn content_key(parts: &[PartSpec]) -> String {
    if parts.len() == 1 {
        parts[0].sha256.clone()
    } else {
        let joined: String = parts.iter().map(|p| p.sha256.as_str()).collect();
        format!("m{}", hex::encode(Sha256::digest(joined.as_bytes())))
    }
}

fn clean(s: &str, max: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

pub struct Hub {
    db: Arc<Db>,
    pub search: Search,
    pub storage: Storage,
    pub limits: Limits,
    st: RwLock<State>,
    /// Serialises writers so read-modify-write sequences are atomic.
    writer: Mutex<()>,
    /// token id → (day, files, bytes)
    quota: Mutex<HashMap<Id, (i64, u32, u64)>>,
    dirty_downloads: Mutex<HashSet<Id>>,
}

impl Hub {
    pub fn open(db: Arc<Db>, storage: Storage, limits: Limits) -> Result<Hub> {
        let st = State::from_db(&db)?;
        let hub = Hub {
            db,
            search: Search::new()?,
            storage,
            limits,
            st: RwLock::new(st),
            writer: Mutex::new(()),
            quota: Mutex::new(HashMap::new()),
            dirty_downloads: Mutex::new(HashSet::new()),
        };
        hub.rebuild_index()?;
        Ok(hub)
    }

    pub fn rebuild_index(&self) -> Result<()> {
        let st = self.st.read();
        for c in st.courses.values() {
            if st.course_listed(c) {
                self.search.put_course(c)?;
            }
        }
        for r in st.resources.values() {
            if r.status == Status::Published {
                if let Some(c) = st.courses.get(&r.course_id) {
                    self.search.put_resource(r, c)?;
                }
            }
        }
        self.search.commit()
    }

    /// One atomic write: `f` stages records into the transaction and the in-memory state.
    /// On failure memory is reloaded from disk so the two never diverge.
    fn mutate<R>(&self, f: impl FnOnce(&mut State, &Tx) -> Result<R>) -> Result<R> {
        let _w = self.writer.lock();
        let mut st = self.st.write();
        let res = self.db.write(|tx| f(&mut st, tx));
        if res.is_err() {
            match State::from_db(&self.db) {
                Ok(fresh) => *st = fresh,
                Err(e) => tracing::error!("reload after failed write: {e}"),
            }
        }
        res
    }

    fn reindex(&self, courses: &[Id], resources: &[Id]) {
        let st = self.st.read();
        let mut courses: Vec<Id> = courses.to_vec();
        for rid in resources {
            let Some(r) = st.resources.get(rid) else { continue };
            courses.push(r.course_id);
            let indexed = r.status == Status::Published
                && st.courses.get(&r.course_id).map(|c| self.search.put_resource(r, c).is_ok()).unwrap_or(false);
            if !indexed {
                self.search.remove(DocType::Resource, *rid);
            }
        }
        courses.sort_unstable();
        courses.dedup();
        for cid in courses {
            match st.courses.get(&cid) {
                Some(c) if st.course_listed(c) => {
                    let _ = self.search.put_course(c);
                }
                _ => self.search.remove(DocType::Course, cid),
            }
        }
        drop(st);
        if let Err(e) = self.search.commit() {
            tracing::error!("search commit: {e}");
        }
    }

    // ---------------------------------------------------------------- tokens

    pub fn authenticate(&self, secret: &str) -> Option<Token> {
        let st = self.st.read();
        let id = st.token_by_hash.get(&hash_token(secret))?;
        st.tokens.get(id).filter(|t| !t.banned).cloned()
    }

    /// Creates a token and returns its secret, which is shown exactly once.
    pub fn issue_token(&self, level: Level, label: &str, ip: &str) -> Result<(String, Token)> {
        let secret = format!("xh{}_{}", level as u8, hex::encode(rand::random::<[u8; 24]>()));
        let t = self.mutate(|st, tx| {
            let t = Token {
                id: st.next_id(tx)?,
                hash: hash_token(&secret),
                level,
                label: clean(label, 60),
                banned: false,
                created_at: now(),
                created_ip: clean(ip, 64),
                uploads: 0,
            };
            st.put_token(tx, t.clone())?;
            Ok(t)
        })?;
        Ok((secret, t))
    }

    pub fn tokens(&self, actor: Viewer) -> Result<Vec<Token>> {
        let me = actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<Token> = st
            .tokens
            .values()
            // Reviewers only manage the tiers below them.
            .filter(|t| me.level == Level::Admin || t.level < Level::Reviewer)
            .cloned()
            .collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.id));
        Ok(v)
    }

    pub fn update_token(&self, actor: Viewer, id: Id, level: Option<Level>, banned: Option<bool>, label: Option<String>) -> Result<Token> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        self.mutate(|st, tx| {
            let mut t = st.tokens.get(&id).cloned().ok_or(Error::NotFound("令牌"))?;
            if t.id == me.id {
                return Err(bad("不能修改自己的令牌"));
            }
            if me.level < Level::Admin {
                let target_ok = t.level < Level::Reviewer && level.is_none_or(|l| l < Level::Reviewer);
                if !target_ok {
                    return Err(Error::Forbidden);
                }
            }
            if let Some(l) = level {
                t.level = l;
            }
            if let Some(b) = banned {
                t.banned = b;
            }
            if let Some(l) = label {
                t.label = clean(&l, 60);
            }
            st.put_token(tx, t.clone())?;
            Ok(t)
        })
    }

    // ---------------------------------------------------------------- courses

    pub fn course(&self, viewer: Viewer, id: Id) -> Result<(Course, usize)> {
        let st = self.st.read();
        let c = st.resolve_course(id).ok_or(Error::NotFound("课程"))?;
        let mine = viewer.id().is_some_and(|v| v == c.created_by);
        if !st.course_listed(c) && viewer.level() < Level::Reviewer && !mine {
            return Err(Error::NotFound("课程"));
        }
        Ok((c.clone(), st.published_count(c.id)))
    }

    /// Listed courses with their published resource counts, most populated first.
    pub fn courses(&self, college: Option<&str>) -> Vec<(Course, usize)> {
        let st = self.st.read();
        let mut v: Vec<(Course, usize)> = st
            .courses
            .values()
            .filter(|c| st.course_listed(c) && college.is_none_or(|col| c.college == col))
            .map(|c| (c.clone(), st.published_count(c.id)))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        v
    }

    pub fn colleges(&self) -> Vec<(String, usize)> {
        let st = self.st.read();
        let mut m: HashMap<&str, usize> = HashMap::new();
        for c in st.courses.values().filter(|c| st.course_listed(c) && !c.college.is_empty()) {
            *m.entry(c.college.as_str()).or_default() += 1;
        }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }

    /// Courses for the upload form's autocomplete — includes pending ones so uploaders
    /// reuse a course someone just created instead of making a duplicate.
    pub fn suggest_courses(&self, q: &str, limit: usize) -> Vec<Course> {
        let st = self.st.read();
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u8, &Course)> = st
            .courses
            .values()
            .filter(|c| !matches!(c.status, CourseStatus::Merged(_)))
            .filter_map(|c| {
                let name = c.name.to_lowercase();
                let code = c.code.to_lowercase();
                let py = crate::text::pinyin_forms(&c.name);
                let score = if code == q || name == q {
                    0
                } else if code.starts_with(&q) || name.starts_with(&q) {
                    1
                } else if name.contains(&q) || c.aliases.iter().any(|a| a.to_lowercase().contains(&q)) {
                    2
                } else if py.split(' ').any(|p| !p.is_empty() && p.starts_with(&q)) {
                    3
                } else {
                    return None;
                };
                Some((score, c))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.len().cmp(&b.1.name.len())));
        scored.into_iter().take(limit).map(|(_, c)| c.clone()).collect()
    }

    fn create_course_in(st: &mut State, tx: &Tx, actor: &Token, nc: &NewCourse) -> Result<Id> {
        let name = clean(&nc.name, 60);
        let code = clean(&nc.code, 30).to_uppercase();
        if name.chars().count() < 2 {
            return Err(bad("课程名称至少两个字"));
        }
        // Reuse an existing course with the same code or exact name.
        let existing = st.courses.values().find(|c| {
            !matches!(c.status, CourseStatus::Merged(_)) && ((!code.is_empty() && c.code == code) || c.name == name)
        });
        if let Some(c) = existing {
            return Ok(c.id);
        }
        let c = Course {
            id: st.next_id(tx)?,
            code,
            name,
            aliases: Vec::new(),
            college: clean(&nc.college, 40),
            status: if actor.level >= Level::Reviewer { CourseStatus::Active } else { CourseStatus::Pending },
            created_by: actor.id,
            created_at: now(),
        };
        let id = c.id;
        st.put_course(tx, c)?;
        Ok(id)
    }

    pub fn update_course(&self, actor: Viewer, id: Id, p: CoursePatch, approve: bool) -> Result<Course> {
        actor.at_least(Level::Reviewer)?;
        let c = self.mutate(|st, tx| {
            let mut c = st.courses.get(&id).cloned().ok_or(Error::NotFound("课程"))?;
            if let Some(v) = p.code {
                c.code = clean(&v, 30).to_uppercase();
            }
            if let Some(v) = p.name {
                let v = clean(&v, 60);
                if v.chars().count() < 2 {
                    return Err(bad("课程名称至少两个字"));
                }
                c.name = v;
            }
            if let Some(v) = p.college {
                c.college = clean(&v, 40);
            }
            if let Some(v) = p.aliases {
                c.aliases = v.iter().map(|a| clean(a, 40)).filter(|a| !a.is_empty()).take(20).collect();
            }
            if approve && c.status == CourseStatus::Pending {
                c.status = CourseStatus::Active;
            }
            st.put_course(tx, c.clone())?;
            Ok(c)
        })?;
        self.reindex(&[id], &[]);
        Ok(c)
    }

    /// Moves every resource of `from` into `into` and retires `from`.
    pub fn merge_course(&self, actor: Viewer, from: Id, into: Id) -> Result<Course> {
        actor.at_least(Level::Reviewer)?;
        if from == into {
            return Err(bad("不能合并到自己"));
        }
        let (target, moved) = self.mutate(|st, tx| {
            let mut src = st.courses.get(&from).cloned().ok_or(Error::NotFound("课程"))?;
            let mut dst = st.courses.get(&into).cloned().ok_or(Error::NotFound("目标课程"))?;
            if matches!(dst.status, CourseStatus::Merged(_)) || matches!(src.status, CourseStatus::Merged(_)) {
                return Err(bad("课程已被合并"));
            }
            let moved: Vec<Id> = st.by_course.get(&from).cloned().unwrap_or_default();
            for rid in &moved {
                let mut r = st.resources[rid].clone();
                r.course_id = into;
                r.updated_at = now();
                st.put_resource(tx, r)?;
            }
            for name in std::iter::once(src.name.clone()).chain(src.aliases.iter().cloned()) {
                if name != dst.name && !dst.aliases.contains(&name) {
                    dst.aliases.push(name);
                }
            }
            if dst.code.is_empty() {
                dst.code = src.code.clone();
            }
            if dst.college.is_empty() {
                dst.college = src.college.clone();
            }
            src.status = CourseStatus::Merged(into);
            st.put_course(tx, src)?;
            st.put_course(tx, dst.clone())?;
            Ok((dst, moved))
        })?;
        self.reindex(&[from, into], &moved);
        Ok(target)
    }

    // ---------------------------------------------------------------- resources

    pub fn resource(&self, viewer: Viewer, id: Id) -> Result<(Resource, Course)> {
        let st = self.st.read();
        let r = st.resources.get(&id).filter(|r| viewer.can_see(r)).ok_or(Error::NotFound("资料"))?;
        let c = st.courses.get(&r.course_id).ok_or(Error::NotFound("课程"))?;
        Ok((r.clone(), c.clone()))
    }

    pub fn course_resources(&self, viewer: Viewer, course: Id) -> Vec<Resource> {
        let st = self.st.read();
        let Some(c) = st.resolve_course(course) else { return Vec::new() };
        let mut v: Vec<Resource> = st
            .by_course
            .get(&c.id)
            .into_iter()
            .flatten()
            .map(|id| &st.resources[id])
            .filter(|r| viewer.can_see(r) && r.status != Status::Removed)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.year.cmp(&a.year).then(b.created_at.cmp(&a.created_at)));
        v
    }

    pub fn recent(&self, limit: usize) -> Vec<(Resource, Course)> {
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.status == Status::Published).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        v.into_iter()
            .take(limit)
            .filter_map(|r| Some((r.clone(), st.courses.get(&r.course_id)?.clone())))
            .collect()
    }

    pub fn popular(&self, limit: usize) -> Vec<(Resource, Course)> {
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.status == Status::Published).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.downloads));
        v.into_iter()
            .take(limit)
            .filter_map(|r| Some((r.clone(), st.courses.get(&r.course_id)?.clone())))
            .collect()
    }

    /// Resources uploaded with this token, newest first (the uploader's own history).
    pub fn my_resources(&self, actor: Viewer) -> Result<Vec<(Resource, Course)>> {
        let me = actor.at_least(Level::Contributor)?;
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.uploader == me.id).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(v.into_iter().filter_map(|r| Some((r.clone(), st.courses.get(&r.course_id)?.clone()))).collect())
    }

    pub fn review_queue(&self, actor: Viewer) -> Result<Vec<(Resource, Course)>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<&Resource> = st
            .resources
            .values()
            .filter(|r| r.status == Status::Pending || (r.status == Status::Published && r.needs_review))
            .collect();
        v.sort_by_key(|r| r.created_at);
        Ok(v.into_iter().filter_map(|r| Some((r.clone(), st.courses.get(&r.course_id)?.clone()))).collect())
    }

    pub fn pending_courses(&self, actor: Viewer) -> Result<Vec<(Course, usize)>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<(Course, usize)> = st
            .courses
            .values()
            .filter(|c| c.status == CourseStatus::Pending)
            .map(|c| (c.clone(), st.by_course.get(&c.id).map(Vec::len).unwrap_or(0)))
            .collect();
        v.sort_by_key(|(c, _)| c.created_at);
        Ok(v)
    }

    fn validate_input(input: &ResourceInput) -> Result<(String, Kind, Option<u16>, Option<u8>)> {
        let title = clean(&input.title, 120);
        if title.chars().count() < 2 {
            return Err(bad("标题至少两个字"));
        }
        let kind = Kind::parse(&input.kind).ok_or_else(|| bad("未知的资料类型"))?;
        let year = input.year.filter(|y| (1990..=2100).contains(y));
        let term = input.term.filter(|t| (1..=3).contains(t));
        Ok((title, kind, year, term))
    }

    /// Creates a resource from a finished upload.
    pub fn create_resource(
        &self,
        actor: Viewer,
        upload_id: Id,
        course_id: Option<Id>,
        new_course: Option<NewCourse>,
        input: ResourceInput,
    ) -> Result<Resource> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let (title, kind, year, term) = Self::validate_input(&input)?;
        let r = self.mutate(|st, tx| {
            let mut up = st.uploads.get(&upload_id).cloned().ok_or(Error::NotFound("上传记录"))?;
            if up.token != me.id {
                return Err(Error::Forbidden);
            }
            if !up.finished {
                return Err(Error::Conflict("文件还没有上传完成".into()));
            }
            if up.consumed {
                return Err(Error::Conflict("这个上传已经提交过了".into()));
            }
            let course_id = match (course_id, new_course) {
                (Some(id), _) => st.resolve_course(id).map(|c| c.id).ok_or(Error::NotFound("课程"))?,
                (None, Some(nc)) => Self::create_course_in(st, tx, &me, &nc)?,
                (None, None) => return Err(bad("请选择或新建课程")),
            };
            let dup = st.by_course.get(&course_id).into_iter().flatten().any(|id| {
                let r = &st.resources[id];
                r.blob == up.key && matches!(r.status, Status::Published | Status::Pending)
            });
            if dup {
                return Err(Error::Conflict("这门课下已经有相同的文件了".into()));
            }
            let direct = me.level.publishes_directly();
            let r = Resource {
                id: st.next_id(tx)?,
                course_id,
                title,
                kind,
                year,
                term,
                teacher: clean(&input.teacher, 40),
                description: clean(&input.description, 1000),
                blob: up.key.clone(),
                filename: up.filename.clone(),
                size: up.size,
                mime: up.mime.clone(),
                status: if direct { Status::Published } else { Status::Pending },
                needs_review: me.level == Level::Trusted,
                review_note: String::new(),
                uploader: me.id,
                reviewed_by: None,
                created_at: now(),
                updated_at: now(),
                downloads: 0,
            };
            st.put_resource(tx, r.clone())?;
            up.consumed = true;
            tx.put_upload(&up)?;
            st.uploads.insert(up.id, up);
            let mut t = st.tokens[&me.id].clone();
            t.uploads += 1;
            st.put_token(tx, t)?;
            Ok(r)
        })?;
        self.reindex(&[r.course_id], &[r.id]);
        Ok(r)
    }

    pub fn update_resource(&self, actor: Viewer, id: Id, input: ResourceInput, course_id: Option<Id>) -> Result<Resource> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let (title, kind, year, term) = Self::validate_input(&input)?;
        let (r, old_course) = self.mutate(|st, tx| {
            let mut r = st.resources.get(&id).cloned().ok_or(Error::NotFound("资料"))?;
            let own_pending = r.uploader == me.id && r.status == Status::Pending;
            if me.level < Level::Reviewer && !own_pending {
                return Err(Error::Forbidden);
            }
            let old_course = r.course_id;
            if let Some(cid) = course_id {
                r.course_id = st.resolve_course(cid).map(|c| c.id).ok_or(Error::NotFound("课程"))?;
            }
            r.title = title;
            r.kind = kind;
            r.year = year;
            r.term = term;
            r.teacher = clean(&input.teacher, 40);
            r.description = clean(&input.description, 1000);
            r.updated_at = now();
            st.put_resource(tx, r.clone())?;
            Ok((r, old_course))
        })?;
        self.reindex(&[old_course, r.course_id], &[r.id]);
        Ok(r)
    }

    /// Approve / reject / take down. Returns storage locations that became unreferenced
    /// and should be deleted by the caller (network I/O stays outside the write lock).
    pub fn review(&self, actor: Viewer, id: Id, action: &str, note: &str) -> Result<(Resource, Vec<Location>)> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        let (r, garbage) = self.mutate(|st, tx| {
            let mut r = st.resources.get(&id).cloned().ok_or(Error::NotFound("资料"))?;
            match action {
                "approve" => {
                    r.status = Status::Published;
                    r.needs_review = false;
                    // Approving a resource also confirms the course it created.
                    if let Some(mut c) = st.courses.get(&r.course_id).cloned() {
                        if c.status == CourseStatus::Pending {
                            c.status = CourseStatus::Active;
                            st.put_course(tx, c)?;
                        }
                    }
                }
                "reject" => r.status = Status::Rejected,
                "remove" => r.status = Status::Removed,
                "restore" => r.status = Status::Published,
                _ => return Err(bad("未知操作")),
            }
            r.review_note = clean(note, 300);
            r.reviewed_by = Some(me.id);
            r.updated_at = now();
            st.put_resource(tx, r.clone())?;
            let garbage = if matches!(r.status, Status::Rejected) { Self::release_blob(st, tx, &r.blob)? } else { Vec::new() };
            Ok((r, garbage))
        })?;
        self.reindex(&[r.course_id], &[r.id]);
        Ok((r, garbage))
    }

    /// Drops a blob no live resource or open upload uses; returns its replicas for deletion.
    /// Removed (taken-down) resources keep their blob so a takedown can be undone.
    fn release_blob(st: &mut State, tx: &Tx, key: &str) -> Result<Vec<Location>> {
        let used = st.resources.values().any(|r| r.blob == key && r.status != Status::Rejected)
            || st.uploads.values().any(|u| u.key == key && !u.consumed);
        if used {
            return Ok(Vec::new());
        }
        let Some(b) = st.blobs.remove(key) else { return Ok(Vec::new()) };
        tx.del_blob(key)?;
        Ok(b.parts.into_iter().flat_map(|p| p.replicas).collect())
    }

    // ---------------------------------------------------------------- uploads

    fn charge_quota(&self, t: &Token, bytes: u64) -> Result<()> {
        let limit = match t.level {
            Level::Guest => return Err(Error::Unauthorized),
            Level::Contributor => self.limits.contributor_daily,
            Level::Trusted => self.limits.trusted_daily,
            Level::Reviewer | Level::Admin => return Ok(()),
        };
        let day = now() / 86400;
        let mut q = self.quota.lock();
        let e = q.entry(t.id).or_insert((day, 0, 0));
        if e.0 != day {
            *e = (day, 0, 0);
        }
        if e.1 + 1 > limit.0 || e.2 + bytes > limit.1 {
            return Err(Error::TooMany("今天的上传额度已用完，明天再来吧".into()));
        }
        e.1 += 1;
        e.2 += bytes;
        Ok(())
    }

    pub async fn begin_upload(&self, actor: Viewer<'_>, filename: &str, mime: &str, parts: Vec<PartSpec>) -> Result<UploadPlan> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let filename = clean(filename, 150);
        if filename.is_empty() {
            return Err(bad("文件名不能为空"));
        }
        if parts.is_empty() || parts.len() > 64 {
            return Err(bad("分片数量不合法"));
        }
        let mut size = 0u64;
        for p in &parts {
            check_hex64(&p.sha256)?;
            if p.size == 0 || p.size > self.limits.max_part {
                return Err(bad("分片大小不合法"));
            }
            size += p.size;
        }
        if size > self.limits.max_file {
            return Err(bad(format!("文件不能超过 {} MB", self.limits.max_file / 1024 / 1024)));
        }
        let key = content_key(&parts);
        let mime = if mime.is_empty() { "application/octet-stream".to_string() } else { clean(mime, 100) };

        let dedup = self.st.read().blobs.contains_key(&key);
        self.charge_quota(&me, if dedup { 0 } else { size })?;

        // Reserve storage slots before touching the database (network I/O, no locks held).
        let mut pending = Vec::with_capacity(parts.len());
        if !dedup {
            for p in &parts {
                let target = self.storage.primary().reserve(&filename, p.size, &p.sha256).await?;
                pending.push(PendingPart { size: p.size, sha256: p.sha256.clone(), target, done: false });
            }
        }
        let up = self.mutate(|st, tx| {
            let up = Upload {
                id: st.next_id(tx)?,
                token: me.id,
                filename: filename.clone(),
                mime,
                size,
                key,
                parts: pending,
                created_at: now(),
                finished: dedup,
                consumed: false,
            };
            tx.put_upload(&up)?;
            st.uploads.insert(up.id, up.clone());
            Ok(up)
        })?;
        self.plan(&up)
    }

    fn plan(&self, up: &Upload) -> Result<UploadPlan> {
        let mut parts = Vec::new();
        for (index, p) in up.parts.iter().enumerate() {
            let target = if p.done { None } else { Some(self.storage.for_location(&p.target)?.upload_target(&p.target, p.size)?) };
            parts.push(PartPlan { index, size: p.size, done: p.done, target });
        }
        Ok(UploadPlan { upload_id: up.id, dedup: up.finished && up.parts.is_empty(), parts })
    }

    /// Current plan of an upload (fresh tickets for parts still to send).
    pub fn upload_plan(&self, actor: Viewer, id: Id) -> Result<UploadPlan> {
        let me = actor.at_least(Level::Contributor)?;
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.token != me.id {
            return Err(Error::Forbidden);
        }
        self.plan(&up)
    }

    /// Moves a not-yet-sent part to a fresh storage slot. Used when a send failed midway:
    /// GitHub keeps a half-written asset name reserved, so the retry needs a new name.
    pub async fn renew_part(&self, actor: Viewer<'_>, id: Id, index: usize) -> Result<PartPlan> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.token != me.id {
            return Err(Error::Forbidden);
        }
        let part = up.parts.get(index).ok_or_else(|| bad("分片序号不合法"))?.clone();
        if part.done {
            return Err(Error::Conflict("这一卷已经上传完成".into()));
        }
        let loc = self.storage.primary().reserve(&up.filename, part.size, &part.sha256).await?;
        let target = self.storage.primary().upload_target(&loc, part.size)?;
        self.mutate(|st, tx| {
            let mut up = st.uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
            up.parts[index].target = loc;
            tx.put_upload(&up)?;
            st.uploads.insert(id, up);
            Ok(())
        })?;
        Ok(PartPlan { index, size: part.size, done: false, target: Some(target) })
    }

    /// Verifies one part with its backend; the last confirmed part creates the blob.
    pub async fn confirm_part(&self, actor: Viewer<'_>, id: Id, index: usize, receipt: Receipt) -> Result<bool> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.token != me.id {
            return Err(Error::Forbidden);
        }
        let part = up.parts.get(index).ok_or_else(|| bad("分片序号不合法"))?.clone();
        if part.done {
            return Ok(up.finished);
        }
        let loc = self.storage.for_location(&part.target)?.confirm(&part.target, part.size, &part.sha256, &receipt).await?;
        self.mutate(|st, tx| {
            let mut up = st.uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
            up.parts[index].target = loc;
            up.parts[index].done = true;
            if up.parts.iter().all(|p| p.done) && !up.finished {
                up.finished = true;
                if !st.blobs.contains_key(&up.key) {
                    let b = Blob {
                        key: up.key.clone(),
                        size: up.size,
                        parts: up
                            .parts
                            .iter()
                            .map(|p| Part { size: p.size, sha256: p.sha256.clone(), replicas: vec![p.target.clone()] })
                            .collect(),
                        created_at: now(),
                    };
                    tx.put_blob(&b)?;
                    st.blobs.insert(b.key.clone(), b);
                }
            }
            tx.put_upload(&up)?;
            let finished = up.finished;
            st.uploads.insert(id, up);
            Ok(finished)
        })
    }

    /// Drops stale uploads; returns orphaned storage locations for the caller to delete.
    pub fn collect_garbage(&self) -> Result<Vec<Location>> {
        let cutoff = now() - UPLOAD_TTL;
        self.mutate(|st, tx| {
            let stale: Vec<Upload> = st.uploads.values().filter(|u| u.created_at < cutoff).cloned().collect();
            let mut garbage = Vec::new();
            for u in stale {
                st.uploads.remove(&u.id);
                tx.del_upload(u.id)?;
                if u.finished {
                    // If nothing ended up using the blob, drop it.
                    garbage.extend(Self::release_blob(st, tx, &u.key)?);
                } else {
                    // Parts sent for an upload that never finished are not in any blob.
                    garbage.extend(u.parts.into_iter().filter(|p| p.done).map(|p| p.target));
                }
            }
            Ok(garbage)
        })
    }

    // ---------------------------------------------------------------- downloads

    pub fn download(&self, viewer: Viewer, id: Id) -> Result<DownloadPlan> {
        let plan = {
            let st = self.st.read();
            let r = st
                .resources
                .get(&id)
                .filter(|r| viewer.can_see(r) && r.status != Status::Removed && r.status != Status::Rejected)
                .ok_or(Error::NotFound("资料"))?;
            let b = st.blobs.get(&r.blob).ok_or(Error::NotFound("文件"))?;
            DownloadPlan {
                filename: r.filename.clone(),
                size: r.size,
                mime: r.mime.clone(),
                parts: b
                    .parts
                    .iter()
                    .map(|p| DownloadPart { size: p.size, sha256: p.sha256.clone(), urls: self.storage.download_urls(&p.replicas) })
                    .collect(),
            }
        };
        if let Some(r) = self.st.write().resources.get_mut(&id) {
            r.downloads += 1;
        }
        self.dirty_downloads.lock().insert(id);
        Ok(plan)
    }

    /// Persists download counters accumulated in memory.
    pub fn flush_downloads(&self) -> Result<()> {
        let ids: Vec<Id> = self.dirty_downloads.lock().drain().collect();
        if ids.is_empty() {
            return Ok(());
        }
        let _w = self.writer.lock();
        let st = self.st.read();
        self.db.write(|tx| {
            for id in &ids {
                if let Some(r) = st.resources.get(id) {
                    tx.put_resource(r)?;
                }
            }
            Ok(())
        })
    }

    pub fn stats(&self) -> Stats {
        let st = self.st.read();
        Stats {
            courses: st.courses.values().filter(|c| st.course_listed(c)).count(),
            resources: st.resources.values().filter(|r| r.status == Status::Published).count(),
            pending: st.resources.values().filter(|r| r.status == Status::Pending || (r.status == Status::Published && r.needs_review)).count(),
            tokens: st.tokens.len(),
            stored_bytes: st.blobs.values().map(|b| b.size).sum(),
            downloads: st.resources.values().map(|r| r.downloads).sum(),
        }
    }

    pub fn search(&self, viewer: Viewer, q: &str, filter: Filter, limit: usize, offset: usize) -> Result<(Vec<SearchItem>, usize)> {
        let (hits, total) = self.search.search(q, filter, limit, offset)?;
        let st = self.st.read();
        let items = hits
            .into_iter()
            .filter_map(|h| match h.ty {
                DocType::Course => {
                    let c = st.courses.get(&h.id)?;
                    Some(SearchItem::Course { course: c.clone(), count: st.published_count(c.id) })
                }
                DocType::Resource => {
                    let r = st.resources.get(&h.id).filter(|r| viewer.can_see(r))?;
                    Some(SearchItem::Resource { resource: r.clone(), course: st.courses.get(&r.course_id)?.clone() })
                }
            })
            .collect();
        Ok((items, total))
    }

    pub fn export(&self) -> Result<Vec<u8>> {
        self.flush_downloads()?;
        self.db.export()
    }
}

pub enum SearchItem {
    Course { course: Course, count: usize },
    Resource { resource: Resource, course: Course },
}
