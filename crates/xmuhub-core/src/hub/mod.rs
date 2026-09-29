//! Domain logic. Reads are served from an in-memory catalog; every write goes through
//! `mutate`, which persists to redb in one transaction and then updates memory and search.
//!
//! Split by concern: `accounts` (users, sessions, codes), `tree` (category nodes),
//! `resources` (metadata, review, reports), `uploads` (storage slots and blobs).

mod accounts;
mod avatars;
mod batches;
mod imports;
mod inbox;
mod links;
mod notices;
mod collections;
mod series;
mod purge;
mod stats_public;
mod resources;
mod social;
mod stats;
mod thumbs;
mod tokens;
mod tree;
mod uploads;
mod wants;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::db::{Db, Tx};
use crate::error::{Error, Result};
use crate::model::*;
use crate::search::{DocType, Placement, Search};
use crate::storage::Storage;

pub use accounts::{CodePurpose, Registration, SESSION_TTL, SYSTEM_EMAIL, needs_captcha, normalize_email};
pub use avatars::{AVATAR_MAX_BYTES, PendingAvatar};
pub use batches::{BATCH_SIZE, QuestionView};
pub use inbox::INBOX_NAME;
pub use imports::{DOC_EXTS, ImportReport, Unpersisted, group_of, is_doc};
pub use stats::DayStats;
pub use thumbs::ThumbJob;
pub use social::{CommentView, FeedbackStatus, RatingSummary, ReviewView};
pub use tokens::{TOKEN_PREFIX, TokenView};
pub use resources::{AdminExtras, DownloadPart, DownloadPlan, ResourceInput};
pub use tree::{NodeInput, NodePage, NodePatch};
pub use uploads::{PartPlan, PartSpec, UploadPlan, content_key};
pub use collections::CollectionView;
pub use purge::PurgeReport;
pub use series::{SeriesItem, SeriesText, SeriesView};
pub use links::LinkSuggestionView;
pub use notices::NoticeList;
pub use stats_public::SiteStats;
pub use wants::{Person, WantFilter, WantReplyView, WantView};

const SEQ_KEY: &str = "seq";
/// Longest search query looked at (characters); the rest is ignored.
pub const SEARCH_MAX_CHARS: usize = 64;
/// The home page's counters are recomputed at most this often (seconds).
const STATS_TTL: i64 = 60;
/// Id of the hidden 「待整理」 node (see inbox.rs), once created.
const INBOX_KEY: &str = "inbox_node";

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
            // Cloudflare's free plan caps a request body at 100 MB and nginx is set to 100m.
            max_part: 95 * 1024 * 1024,
            contributor_daily: (60, 3 * 1024 * 1024 * 1024),
            trusted_daily: (500, 30 * 1024 * 1024 * 1024),
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    seq: u64,
    nodes: HashMap<Id, Node>,
    /// parent → children, sorted; roots live under key 0.
    children: HashMap<Id, Vec<Id>>,
    resources: HashMap<Id, Resource>,
    by_node: HashMap<Id, Vec<Id>>,
    users: HashMap<Id, User>,
    user_by_email: HashMap<String, Id>,
    sessions: HashMap<[u8; 32], Session>,
    reports: HashMap<Id, Report>,
    uploads: HashMap<Id, Upload>,
    blobs: HashMap<String, Blob>,
    /// Published resources in each node's subtree (recomputed after writes).
    counts: HashMap<Id, usize>,
    /// Review audit trail, oldest first.
    reviews: Vec<ReviewEvent>,
    resource_change_requests: HashMap<Id, ResourceChangeRequest>,
    /// resource → user → stars
    ratings: HashMap<Id, HashMap<Id, u8>>,
    comments: HashMap<Id, Comment>,
    comments_by_res: HashMap<Id, Vec<Id>>,
    feedback: HashMap<Id, Feedback>,
    tokens: HashMap<Id, ApiToken>,
    token_by_hash: HashMap<[u8; 32], Id>,
    /// Hand-edited subtitles; resources without one fall back to `auto_subtitle`.
    subtitles: HashMap<Id, String>,
    /// blob key → thumbnail record
    thumbs: HashMap<String, Thumb>,
    /// Study level set on a node (see `LEVEL_*`); descendants without their own inherit it.
    node_levels: HashMap<Id, u8>,
    /// Uploaders who chose to show their nickname on their files.
    public_uploaders: HashSet<Id>,
    /// user → profile picture
    avatars: HashMap<Id, Avatar>,
    /// reviewers' questions to uploaders, by id
    questions: HashMap<Id, Question>,
    /// resource → major (专业) it is for
    majors: HashMap<Id, String>,
    /// recommended outside sources, by id
    links: HashMap<Id, Link>,
    /// 求资料 posts and their replies, by id
    wants: HashMap<Id, Want>,
    want_replies: HashMap<Id, WantReply>,
    /// want → users who said 「我也要」
    want_votes: HashMap<Id, HashSet<Id>>,
    link_suggestions: HashMap<Id, LinkSuggestion>,
    /// node → users following it, and user → nodes they follow
    followers: HashMap<Id, HashSet<Id>>,
    following: HashMap<Id, HashSet<Id>>,
    /// user → their 站内提醒, oldest first
    notices: HashMap<Id, Vec<Notice>>,
    collections: HashMap<Id, Collection>,
    series: HashMap<Id, Series>,
    /// The site-wide notice, once an admin has set one.
    announcement: Option<Announcement>,
    /// The hidden 「待整理」 node: listed nowhere, its files can't be approved in place.
    inbox: Option<Id>,
}

/// Study levels of a course (or of a college / group, inherited by its courses).
/// Untagged courses count as 本科 when filtering.
pub const LEVEL_UNDERGRAD: u8 = 1;
pub const LEVEL_GRADUATE: u8 = 2;
pub const LEVEL_BOTH: u8 = 3;

impl State {
    fn from_db(db: &Db) -> Result<State> {
        let snap = db.load()?;
        let mut st = State::default();
        for (k, v) in snap.meta {
            if k == SEQ_KEY && v.len() == 8 {
                st.seq = u64::from_le_bytes(v.try_into().unwrap());
            } else if k == INBOX_KEY && v.len() == 8 {
                st.inbox = Some(u64::from_le_bytes(v.try_into().unwrap()));
            } else if k == wants::ANNOUNCEMENT_KEY {
                st.announcement = serde_json::from_slice(&v).ok();
            }
        }
        for n in snap.nodes {
            st.nodes.insert(n.id, n);
        }
        for r in snap.resources {
            st.by_node.entry(r.node).or_default().push(r.id);
            st.resources.insert(r.id, r);
        }
        for u in snap.users {
            st.user_by_email.insert(u.email.clone(), u.id);
            st.users.insert(u.id, u);
        }
        for s in snap.sessions {
            st.sessions.insert(s.hash, s);
        }
        for r in snap.reports {
            st.reports.insert(r.id, r);
        }
        for u in snap.uploads {
            st.uploads.insert(u.id, u);
        }
        for b in snap.blobs {
            st.blobs.insert(b.key.clone(), b);
        }
        st.reviews = snap.reviews;
        st.reviews.sort_by_key(|e| e.id);
        st.resource_change_requests = snap.resource_change_requests.into_iter().map(|r| (r.id, r)).collect();
        for r in snap.ratings {
            st.ratings.entry(r.resource).or_default().insert(r.user, r.stars);
        }
        let mut comments = snap.comments;
        comments.sort_by_key(|c| c.id);
        for c in comments {
            st.comments_by_res.entry(c.resource).or_default().push(c.id);
            st.comments.insert(c.id, c);
        }
        for f in snap.feedback {
            st.feedback.insert(f.id, f);
        }
        st.subtitles = snap.subtitles.into_iter().collect();
        st.node_levels = snap.node_levels.into_iter().collect();
        st.public_uploaders = snap.public_uploaders.into_iter().collect();
        st.avatars = snap.avatars.into_iter().map(|a| (a.user, a)).collect();
        st.questions = snap.questions.into_iter().map(|q| (q.id, q)).collect();
        st.majors = snap.majors.into_iter().collect();
        st.links = snap.links.into_iter().map(|l| (l.id, l)).collect();
        st.wants = snap.wants.into_iter().map(|w| (w.id, w)).collect();
        st.want_replies = snap.want_replies.into_iter().map(|r| (r.id, r)).collect();
        st.link_suggestions = snap.link_suggestions.into_iter().map(|s| (s.id, s)).collect();
        for (user, node) in snap.follows {
            st.followers.entry(node).or_default().insert(user);
            st.following.entry(user).or_default().insert(node);
        }
        let mut notices = snap.notices;
        notices.sort_by_key(|n| n.id);
        for n in notices {
            st.notices.entry(n.user).or_default().push(n);
        }
        st.collections = snap.collections.into_iter().map(|c| (c.id, c)).collect();
        st.series = snap.series.into_iter().map(|s| (s.id, s)).collect();
        for (want, user) in snap.want_votes {
            st.want_votes.entry(want).or_default().insert(user);
        }
        st.thumbs = snap.thumbs.into_iter().map(|t| (t.key.clone(), t)).collect();
        for t in snap.tokens {
            st.token_by_hash.insert(t.hash, t.id);
            st.tokens.insert(t.id, t);
        }
        st.rebuild_children();
        st.recount();
        Ok(st)
    }

    fn next_id(&mut self, tx: &Tx) -> Result<Id> {
        self.seq += 1;
        tx.put_meta(SEQ_KEY, &self.seq.to_le_bytes())?;
        Ok(self.seq)
    }

    fn rebuild_children(&mut self) {
        let mut children: HashMap<Id, Vec<Id>> = HashMap::new();
        for n in self.nodes.values() {
            if !matches!(n.status, NodeStatus::Merged(_)) {
                children.entry(n.parent.unwrap_or(0)).or_default().push(n.id);
            }
        }
        for v in children.values_mut() {
            v.sort_by(|a, b| {
                let (a, b) = (&self.nodes[a], &self.nodes[b]);
                a.sort.cmp(&b.sort).then_with(|| a.code.cmp(&b.code)).then_with(|| a.name.cmp(&b.name))
            });
        }
        self.children = children;
    }

    fn recount(&mut self) {
        let mut counts: HashMap<Id, usize> = HashMap::new();
        for r in self.resources.values().filter(|r| r.status == Status::Published) {
            let mut cur = Some(r.node);
            let mut guard = 0;
            while let Some(id) = cur {
                *counts.entry(id).or_default() += 1;
                cur = self.nodes.get(&id).and_then(|n| n.parent);
                guard += 1;
                if guard > 32 {
                    break;
                }
            }
        }
        self.counts = counts;
    }

    fn put_resource(&mut self, tx: &Tx, r: Resource) -> Result<()> {
        tx.put_resource(&r)?;
        // Newly public (a new file, or one just approved): tell whoever follows its course.
        let was = self.resources.get(&r.id).map(|o| o.status);
        let newly_public = r.status == Status::Published && matches!(was, None | Some(Status::Pending));
        match self.resources.get(&r.id) {
            Some(old) if old.node != r.node => {
                if let Some(v) = self.by_node.get_mut(&old.node) {
                    v.retain(|&x| x != r.id);
                }
                self.by_node.entry(r.node).or_default().push(r.id);
            }
            Some(_) => {}
            None => self.by_node.entry(r.node).or_default().push(r.id),
        }
        if newly_public {
            self.tell_followers(tx, &r)?;
        }
        self.resources.insert(r.id, r);
        Ok(())
    }

    /// The public subtitle: the hand-edited one, else one derived from the original file name.
    pub(crate) fn subtitle(&self, r: &Resource) -> String {
        match self.subtitles.get(&r.id) {
            Some(s) => s.clone(),
            None => crate::text::auto_subtitle(&r.original_name, &r.name.stem()),
        }
    }

    fn put_node(&mut self, tx: &Tx, n: Node) -> Result<()> {
        tx.put_node(&n)?;
        self.nodes.insert(n.id, n);
        Ok(())
    }

    fn put_user(&mut self, tx: &Tx, u: User) -> Result<()> {
        tx.put_user(&u)?;
        self.user_by_email.insert(u.email.clone(), u.id);
        self.users.insert(u.id, u);
        Ok(())
    }

    /// Follows merge links to the surviving node.
    fn resolve(&self, mut id: Id) -> Option<&Node> {
        for _ in 0..16 {
            let n = self.nodes.get(&id)?;
            match n.status {
                NodeStatus::Merged(into) => id = into,
                _ => return Some(n),
            }
        }
        None
    }

    /// Ancestors of `id`, root first (excluding `id`).
    /// The node's study level: its own, else the nearest ancestor's; 0 when none is set.
    fn level_of(&self, id: Id) -> u8 {
        if let Some(l) = self.node_levels.get(&id) {
            return *l;
        }
        self.ancestors(id).iter().rev().find_map(|a| self.node_levels.get(a).copied()).unwrap_or(0)
    }

    fn ancestors(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut cur = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(p) = cur {
            if out.contains(&p) || out.len() > 32 {
                break;
            }
            out.push(p);
            cur = self.nodes.get(&p).and_then(|n| n.parent);
        }
        out.reverse();
        out
    }

    fn path_text(&self, id: Id) -> String {
        let mut names: Vec<&str> = self.ancestors(id).iter().filter_map(|a| self.nodes.get(a)).map(|n| n.name.as_str()).collect();
        if let Some(n) = self.nodes.get(&id) {
            names.push(&n.name);
        }
        names.join(" ")
    }

    fn aliases_text(&self, id: Id) -> String {
        let mut out = Vec::new();
        for a in self.ancestors(id).into_iter().chain(std::iter::once(id)) {
            if let Some(n) = self.nodes.get(&a) {
                out.extend(n.aliases.iter().map(String::as_str));
            }
        }
        out.join(" ")
    }

    fn subtree(&self, id: Id) -> Vec<Id> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            if let Some(c) = self.children.get(&out[i]) {
                out.extend(c.iter().copied());
            }
            i += 1;
            if out.len() > 100_000 {
                break;
            }
        }
        out
    }
}

/// Who is asking. `None` = not signed in.
#[derive(Debug, Clone, Copy)]
pub struct Viewer<'a> {
    pub user: Option<&'a User>,
}

impl Viewer<'_> {
    pub fn level(&self) -> Level {
        self.user.map(|u| u.level).unwrap_or(Level::Guest)
    }
    pub fn id(&self) -> Option<Id> {
        self.user.map(|u| u.id)
    }
    pub fn staff(&self) -> bool {
        self.level() >= Level::Reviewer
    }
    /// The owner's import account (the script token).
    pub fn system(&self) -> bool {
        self.user.is_some_and(|u| u.email == SYSTEM_EMAIL)
    }
    /// Skips review: admins and the import account. Everything anyone else submits,
    /// reviewers included, waits for a reviewer who didn't submit it.
    pub fn exempt(&self) -> bool {
        self.system() || self.level() >= Level::Admin
    }
    pub(crate) fn at_least(&self, l: Level) -> Result<&User> {
        match self.user {
            Some(u) if u.level >= l => Ok(u),
            Some(_) => Err(Error::Forbidden),
            None => Err(Error::Unauthorized),
        }
    }
    pub fn can_see(&self, r: &Resource) -> bool {
        match r.status {
            Status::Published => true,
            Status::Pending | Status::Rejected => self.staff() || self.id() == Some(r.uploader),
            Status::Removed | Status::Restricted => self.staff(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub nodes: usize,
    pub resources: usize,
    pub pending: usize,
    pub reports: usize,
    pub users: usize,
    pub stored_bytes: u64,
    pub downloads: u64,
}

pub enum SearchItem {
    Node { node: Node, path: Vec<Node>, count: usize },
    Resource { resource: Box<Resource>, node: Node, path: Vec<Node> },
}

/// A complaint with the file it is about (gone when the file was deleted from the catalog).
pub type ReportItem = (Report, Option<(Resource, Node)>);

pub(crate) fn clean(s: &str, max: usize) -> String {
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
    /// user id → (day, files, bytes)
    quota: Mutex<HashMap<Id, (i64, u32, u64)>>,
    dirty_downloads: Mutex<HashSet<Id>>,
    /// (resource, visitor) → day it was last counted as a download
    download_seen: Mutex<HashMap<(Id, String), i64>>,
    /// (computed at, value) of `stats()`, which walks every resource and blob
    stats_cache: Mutex<Option<(i64, Stats)>>,
    batches: batches::Batches,
    auth: accounts::AuthState,
}

impl Hub {
    pub fn open(db: Arc<Db>, storage: Storage, limits: Limits, admins: Vec<String>) -> Result<Hub> {
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
            download_seen: Mutex::new(HashMap::new()),
            stats_cache: Mutex::new(None),
            batches: batches::Batches::default(),
            auth: accounts::AuthState::new(Vec::new()),
        };
        hub.set_admins(admins)?;
        hub.rebuild_index()?;
        Ok(hub)
    }

    /// Search rank multiplier (×100) for a node: courses first, empty nodes last.
    fn node_weight(st: &State, n: &Node) -> u64 {
        let base = match n.kind {
            NodeKind::Course => 300,
            NodeKind::Group => 200,
            NodeKind::Level => 150,
            NodeKind::Section => 100,
        };
        if st.counts.get(&n.id).copied().unwrap_or(0) == 0 { base / 3 } else { base }
    }

    /// (own, effective) study level of a node; see `LEVEL_*`, 0 = not set.
    pub fn node_level(&self, id: Id) -> (u8, u8) {
        let st = self.st.read();
        (st.node_levels.get(&id).copied().unwrap_or(0), st.level_of(id))
    }

    /// Whether the uploader chose to show their nickname on their files.
    pub fn public_uploader(&self, user: Id) -> bool {
        self.st.read().public_uploaders.contains(&user)
    }

    pub fn set_public_uploader(&self, actor: Viewer, public: bool) -> Result<bool> {
        let me = actor.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            tx.put_public_uploader(me, public)?;
            if public { st.public_uploaders.insert(me); } else { st.public_uploaders.remove(&me); }
            Ok(public)
        })
    }

    fn is_course(st: &State, n: &Node) -> bool {
        n.kind == NodeKind::Course
            && !n.parent.and_then(|id| st.nodes.get(&id)).is_some_and(|parent| parent.kind == NodeKind::Section)
    }

    fn placement_parts(st: &State, node: Id) -> (Vec<Id>, String, String) {
        (st.ancestors(node), st.path_text(node), st.aliases_text(node))
    }

    pub fn rebuild_index(&self) -> Result<()> {
        let st = self.st.read();
        for n in st.nodes.values() {
            if !matches!(n.status, NodeStatus::Merged(_)) && n.kind != NodeKind::Section && Some(n.id) != st.inbox {
                let (anc, path, aliases) = Self::placement_parts(&st, n.id);
                let parent_path = anc.iter().filter_map(|a| st.nodes.get(a)).map(|n| n.name.as_str()).collect::<Vec<_>>().join(" ");
                let _ = path;
                self.search.put_node(n, &Placement { node: n.id, ancestors: &anc, path_text: &parent_path, aliases_text: &aliases }, Self::node_weight(&st, n), Self::is_course(&st, n), st.level_of(n.id))?;
            }
        }
        for r in st.resources.values() {
            if r.status == Status::Published {
                let (anc, path, aliases) = Self::placement_parts(&st, r.node);
                self.search.put_resource(r, &Placement { node: r.node, ancestors: &anc, path_text: &path, aliases_text: &aliases }, &st.subtitle(r), st.level_of(r.node))?;
            }
        }
        self.search.commit()
    }

    /// One atomic write: `f` stages records into the transaction and the in-memory state.
    /// On failure memory is reloaded from disk so the two never diverge.
    fn mutate<R>(&self, f: impl FnOnce(&mut State, &Tx) -> Result<R>) -> Result<R> {
        let _w = self.writer.lock();
        // The in-memory update is quick; the commit waits for the disk. Readers only wait
        // for the former — holding the state lock through the fsync stalled every page
        // (and, on the server's two async threads, every request) behind each write.
        let res = self.db.begin().and_then(|txn| {
            let r = f(&mut self.st.write(), &txn.tx())?;
            txn.commit()?;
            Ok(r)
        });
        if res.is_err() {
            // Memory may hold changes the database never got: start over from the database.
            match State::from_db(&self.db) {
                Ok(fresh) => *self.st.write() = fresh,
                Err(e) => tracing::error!("reload after failed write: {e}"),
            }
        }
        res
    }

    /// Refreshes counts, the child index and search entries after a write.
    fn reindex(&self, nodes: &[Id], resources: &[Id]) {
        {
            let mut st = self.st.write();
            st.rebuild_children();
            st.recount();
        }
        let st = self.st.read();
        // A resource changing state can flip its ancestors between empty and non-empty,
        // which changes their search weight.
        let mut nodes: Vec<Id> = nodes.to_vec();
        for rid in resources {
            if let Some(r) = st.resources.get(rid) {
                nodes.push(r.node);
                nodes.extend(st.ancestors(r.node));
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
        for rid in resources {
            let Some(r) = st.resources.get(rid) else {
                self.search.remove(DocType::Resource, *rid);
                continue;
            };
            if r.status == Status::Published {
                let (anc, path, aliases) = Self::placement_parts(&st, r.node);
                let _ = self.search.put_resource(r, &Placement { node: r.node, ancestors: &anc, path_text: &path, aliases_text: &aliases }, &st.subtitle(r), st.level_of(r.node));
            } else {
                self.search.remove(DocType::Resource, *rid);
            }
        }
        for nid in &nodes {
            match st.nodes.get(nid) {
                Some(n) if !matches!(n.status, NodeStatus::Merged(_)) && n.kind != NodeKind::Section && Some(n.id) != st.inbox => {
                    let (anc, _, aliases) = Self::placement_parts(&st, n.id);
                    let parent_path = anc.iter().filter_map(|a| st.nodes.get(a)).map(|n| n.name.as_str()).collect::<Vec<_>>().join(" ");
                    let _ = self.search.put_node(n, &Placement { node: n.id, ancestors: &anc, path_text: &parent_path, aliases_text: &aliases }, Self::node_weight(&st, n), Self::is_course(&st, n), st.level_of(n.id));
                }
                _ => self.search.remove(DocType::Node, *nid),
            }
        }
        drop(st);
        if let Err(e) = self.search.commit() {
            tracing::error!("search commit: {e}");
        }
    }

    /// Site counters for the home page (served from a cache refreshed every `STATS_TTL`).
    pub fn stats(&self) -> Stats {
        let now = now();
        if let Some((at, s)) = &*self.stats_cache.lock()
            && now - at < STATS_TTL
        {
            return s.clone();
        }
        let s = self.compute_stats();
        *self.stats_cache.lock() = Some((now, s.clone()));
        s
    }

    fn compute_stats(&self) -> Stats {
        let st = self.st.read();
        Stats {
            nodes: st.nodes.values().filter(|n| {
                n.kind == NodeKind::Course
                    && Some(n.id) != st.inbox
                    && !n.parent.and_then(|id| st.nodes.get(&id)).is_some_and(|parent| parent.kind == NodeKind::Section)
                    && st.counts.get(&n.id).copied().unwrap_or(0) > 0
            }).count(),
            resources: st.resources.values().filter(|r| r.status == Status::Published).count(),
            pending: st.resources.values().filter(|r| r.status == Status::Pending || (r.status == Status::Published && r.needs_review)).count(),
            reports: st.reports.values().filter(|r| !r.handled).count(),
            users: st.users.len(),
            stored_bytes: st.blobs.values().map(|b| b.size).sum(),
            downloads: st.resources.values().map(|r| r.downloads).sum(),
        }
    }

    pub fn search(&self, viewer: Viewer, q: &str, filter: crate::search::Filter, limit: usize, offset: usize) -> Result<(Vec<SearchItem>, usize)> {
        // Course names and keywords are short; a pasted page of text would only turn into
        // thousands of search terms (and CPU time) for nothing.
        let q: String = q.chars().take(SEARCH_MAX_CHARS).collect();
        let (hits, total) = self.search.search(&q, filter, limit, offset)?;
        let st = self.st.read();
        let path_of = |id: Id| -> Vec<Node> { st.ancestors(id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect() };
        let items = hits
            .into_iter()
            .filter_map(|h| match h.ty {
                DocType::Node => {
                    let n = st.nodes.get(&h.id)?;
                    Some(SearchItem::Node { node: n.clone(), path: path_of(n.id), count: st.counts.get(&n.id).copied().unwrap_or(0) })
                }
                DocType::Resource => {
                    let r = st.resources.get(&h.id).filter(|r| viewer.can_see(r))?;
                    let n = st.nodes.get(&r.node)?;
                    Some(SearchItem::Resource { resource: Box::new(r.clone()), node: n.clone(), path: path_of(n.id) })
                }
            })
            .collect();
        Ok((items, total))
    }

    pub fn export(&self) -> Result<Vec<u8>> {
        self.flush_downloads()?;
        self.db.export()
    }

    /// Persists download counters accumulated in memory.
    pub fn flush_downloads(&self) -> Result<()> {
        let ids: Vec<Id> = self.dirty_downloads.lock().drain().collect();
        if ids.is_empty() {
            return Ok(());
        }
        let _w = self.writer.lock();
        let rows: Vec<Resource> = {
            let st = self.st.read();
            ids.iter().filter_map(|id| st.resources.get(id).cloned()).collect()
        };
        self.db.write(|tx| rows.iter().try_for_each(|r| tx.put_resource(r)))
    }
}
