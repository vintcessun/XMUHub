//! redb persistence. The database is the single source of truth; everything else
//! (in-memory catalog, search index) is rebuilt from it at boot.

use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Error, Result};
use crate::model::*;

const SCHEMA_VERSION: u8 = 2;

pub const NODES: TableDefinition<u64, &[u8]> = TableDefinition::new("nodes");
pub const RESOURCES: TableDefinition<u64, &[u8]> = TableDefinition::new("resources.v2");
pub const USERS: TableDefinition<u64, &[u8]> = TableDefinition::new("users");
pub const SESSIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("sessions");
pub const REPORTS: TableDefinition<u64, &[u8]> = TableDefinition::new("reports");
pub const UPLOADS: TableDefinition<u64, &[u8]> = TableDefinition::new("uploads.v2");
pub const BLOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("blobs");
pub const REVIEWS: TableDefinition<u64, &[u8]> = TableDefinition::new("reviews");
pub const RESOURCE_CHANGE_REQUESTS: TableDefinition<u64, &[u8]> = TableDefinition::new("resource_change_requests");
/// Key: resource id ++ user id, both big-endian.
pub const RATINGS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("ratings");
pub const COMMENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("comments");
pub const FEEDBACK: TableDefinition<u64, &[u8]> = TableDefinition::new("feedback");
pub const TOKENS: TableDefinition<u64, &[u8]> = TableDefinition::new("tokens");
pub const THUMBS: TableDefinition<&str, &[u8]> = TableDefinition::new("thumbs");
/// Resource id → hand-edited subtitle (plain UTF-8; empty = deliberately none).
pub const SUBTITLES: TableDefinition<u64, &str> = TableDefinition::new("subtitles");
/// Node id → study level set on that node (1 本科, 2 研究生, 3 both); descendants inherit it.
pub const NODE_LEVELS: TableDefinition<u64, u8> = TableDefinition::new("node_levels");
/// Users who chose to show their nickname on the files they uploaded (value unused).
pub const PUBLIC_UPLOADERS: TableDefinition<u64, u8> = TableDefinition::new("public_uploaders");
/// user id → Avatar (profile picture, stored like any uploaded file).
pub const AVATARS: TableDefinition<u64, &[u8]> = TableDefinition::new("avatars");
/// id → Question (a reviewer asking a file's uploader something).
pub const QUESTIONS: TableDefinition<u64, &[u8]> = TableDefinition::new("questions");
/// resource id → the major (专业) its content is for, e.g. 软件工程 (absent = any).
pub const RESOURCE_MAJORS: TableDefinition<u64, &str> = TableDefinition::new("resource_majors");
/// id → Link (a recommended outside source: another repo, a netdisk collection …).
pub const LINKS: TableDefinition<u64, &[u8]> = TableDefinition::new("links");
/// id → Want (a 求资料 post).
pub const WANTS: TableDefinition<u64, &[u8]> = TableDefinition::new("wants");
/// id → WantReply
pub const WANT_REPLIES: TableDefinition<u64, &[u8]> = TableDefinition::new("want_replies");
/// (want id, user id) → 1: 「我也要」.
pub const WANT_VOTES: TableDefinition<&[u8], u8> = TableDefinition::new("want_votes");
/// Free-form small state: id sequences, storage bucket cursors, settings.
pub const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut out = vec![SCHEMA_VERSION];
    out.extend(postcard::to_stdvec(v).expect("postcard encoding of in-memory record"));
    out
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    match bytes.split_first() {
        Some((&SCHEMA_VERSION, body)) => Ok(postcard::from_bytes(body)?),
        Some((v, _)) => Err(Error::Internal(format!("unknown record version {v}"))),
        None => Err(Error::Internal("empty record".into())),
    }
}

pub struct Db {
    pub inner: Database,
}

/// Everything loaded at boot.
#[derive(Default)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
    pub resources: Vec<Resource>,
    pub users: Vec<User>,
    pub sessions: Vec<Session>,
    pub reports: Vec<Report>,
    pub uploads: Vec<Upload>,
    pub blobs: Vec<Blob>,
    pub meta: Vec<(String, Vec<u8>)>,
    pub reviews: Vec<ReviewEvent>,
    pub resource_change_requests: Vec<ResourceChangeRequest>,
    pub ratings: Vec<Rating>,
    pub comments: Vec<Comment>,
    pub feedback: Vec<Feedback>,
    pub tokens: Vec<ApiToken>,
    pub subtitles: Vec<(Id, String)>,
    pub thumbs: Vec<Thumb>,
    pub node_levels: Vec<(Id, u8)>,
    pub public_uploaders: Vec<Id>,
    pub avatars: Vec<Avatar>,
    pub questions: Vec<Question>,
    pub majors: Vec<(Id, String)>,
    pub links: Vec<Link>,
    pub wants: Vec<Want>,
    pub want_replies: Vec<WantReply>,
    pub want_votes: Vec<(Id, Id)>,
}

fn rating_key(resource: Id, user: Id) -> [u8; 16] {
    let mut k = [0u8; 16];
    k[..8].copy_from_slice(&resource.to_be_bytes());
    k[8..].copy_from_slice(&user.to_be_bytes());
    k
}

impl Db {
    pub fn open(path: &Path, cache_bytes: usize) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // redb defaults to a 1 GiB page cache; the host has well under that free.
        let inner = Database::builder().set_cache_size(cache_bytes).create(path)?;
        let txn = inner.begin_write()?;
        txn.open_table(NODES)?;
        txn.open_table(RESOURCES)?;
        txn.open_table(USERS)?;
        txn.open_table(SESSIONS)?;
        txn.open_table(REPORTS)?;
        txn.open_table(UPLOADS)?;
        txn.open_table(BLOBS)?;
        txn.open_table(META)?;
        txn.open_table(REVIEWS)?;
        txn.open_table(RESOURCE_CHANGE_REQUESTS)?;
        txn.open_table(RATINGS)?;
        txn.open_table(COMMENTS)?;
        txn.open_table(FEEDBACK)?;
        txn.open_table(TOKENS)?;
        txn.open_table(SUBTITLES)?;
        txn.open_table(NODE_LEVELS)?;
        txn.open_table(PUBLIC_UPLOADERS)?;
        txn.open_table(AVATARS)?;
        txn.open_table(QUESTIONS)?;
        txn.open_table(RESOURCE_MAJORS)?;
        txn.open_table(LINKS)?;
        txn.open_table(WANTS)?;
        txn.open_table(WANT_REPLIES)?;
        txn.open_table(WANT_VOTES)?;
        txn.open_table(THUMBS)?;
        txn.commit()?;
        Ok(Db { inner })
    }

    pub fn load(&self) -> Result<Snapshot> {
        let txn = self.inner.begin_read()?;
        let mut snap = Snapshot::default();
        for row in txn.open_table(NODES)?.iter()? {
            snap.nodes.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(RESOURCES)?.iter()? {
            snap.resources.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(USERS)?.iter()? {
            snap.users.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(SESSIONS)?.iter()? {
            snap.sessions.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(REPORTS)?.iter()? {
            snap.reports.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(UPLOADS)?.iter()? {
            snap.uploads.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(BLOBS)?.iter()? {
            snap.blobs.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(META)?.iter()? {
            let (k, v) = row?;
            snap.meta.push((k.value().to_string(), v.value().to_vec()));
        }
        for row in txn.open_table(REVIEWS)?.iter()? {
            snap.reviews.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(RESOURCE_CHANGE_REQUESTS)?.iter()? {
            snap.resource_change_requests.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(RATINGS)?.iter()? {
            snap.ratings.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(COMMENTS)?.iter()? {
            snap.comments.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(FEEDBACK)?.iter()? {
            snap.feedback.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(TOKENS)?.iter()? {
            snap.tokens.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(SUBTITLES)?.iter()? {
            let (k, v) = row?;
            snap.subtitles.push((k.value(), v.value().to_string()));
        }
        for row in txn.open_table(THUMBS)?.iter()? {
            snap.thumbs.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(NODE_LEVELS)?.iter()? {
            let (k, v) = row?;
            snap.node_levels.push((k.value(), v.value()));
        }
        for row in txn.open_table(PUBLIC_UPLOADERS)?.iter()? {
            snap.public_uploaders.push(row?.0.value());
        }
        for row in txn.open_table(AVATARS)?.iter()? {
            snap.avatars.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(QUESTIONS)?.iter()? {
            snap.questions.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(RESOURCE_MAJORS)?.iter()? {
            let (k, v) = row?;
            snap.majors.push((k.value(), v.value().to_string()));
        }
        for row in txn.open_table(LINKS)?.iter()? {
            snap.links.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(WANTS)?.iter()? {
            snap.wants.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(WANT_REPLIES)?.iter()? {
            snap.want_replies.push(decode(row?.1.value())?);
        }
        for row in txn.open_table(WANT_VOTES)?.iter()? {
            let k = row?.0.value().to_vec();
            if k.len() == 16 {
                snap.want_votes.push((u64::from_be_bytes(k[..8].try_into().unwrap()), u64::from_be_bytes(k[8..].try_into().unwrap())));
            }
        }
        Ok(snap)
    }

    /// Runs `f` inside one write transaction and commits it.
    pub fn write<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        let txn = self.begin()?;
        let r = f(&txn.tx())?;
        txn.commit()?;
        Ok(r)
    }

    /// Opens a write transaction whose commit (the slow, fsync-ing part) the caller does
    /// separately — so it can happen outside any in-memory lock.
    pub fn begin(&self) -> Result<WriteTxn> {
        Ok(WriteTxn(self.inner.begin_write()?))
    }

    /// Dumps every table into one self-describing blob for off-site backup.
    pub fn export(&self) -> Result<Vec<u8>> {
        let snap = self.load()?;
        let dump = Dump {
            version: SCHEMA_VERSION,
            nodes: snap.nodes,
            resources: snap.resources,
            users: snap.users,
            reports: snap.reports,
            blobs: snap.blobs,
            meta: snap.meta,
            reviews: snap.reviews,
            resource_change_requests: snap.resource_change_requests,
            ratings: snap.ratings,
            comments: snap.comments,
            feedback: snap.feedback,
            tokens: snap.tokens,
            subtitles: snap.subtitles,
            thumbs: snap.thumbs,
            node_levels: snap.node_levels,
            public_uploaders: snap.public_uploaders,
            avatars: snap.avatars,
            questions: snap.questions,
            majors: snap.majors,
            links: snap.links,
            wants: snap.wants,
            want_replies: snap.want_replies,
            want_votes: snap.want_votes,
        };
        serde_json::to_vec(&dump).map_err(|e| Error::Internal(e.to_string()))
    }
}

#[derive(Serialize, serde::Deserialize)]
pub struct Dump {
    pub version: u8,
    pub nodes: Vec<Node>,
    pub resources: Vec<Resource>,
    pub users: Vec<User>,
    pub reports: Vec<Report>,
    pub blobs: Vec<Blob>,
    pub meta: Vec<(String, Vec<u8>)>,
    #[serde(default)]
    pub reviews: Vec<ReviewEvent>,
    #[serde(default)]
    pub resource_change_requests: Vec<ResourceChangeRequest>,
    #[serde(default)]
    pub ratings: Vec<Rating>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub feedback: Vec<Feedback>,
    #[serde(default)]
    pub tokens: Vec<ApiToken>,
    #[serde(default)]
    pub subtitles: Vec<(Id, String)>,
    #[serde(default)]
    pub thumbs: Vec<Thumb>,
    #[serde(default)]
    pub node_levels: Vec<(Id, u8)>,
    #[serde(default)]
    pub public_uploaders: Vec<Id>,
    #[serde(default)]
    pub avatars: Vec<Avatar>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub majors: Vec<(Id, String)>,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default)]
    pub wants: Vec<Want>,
    #[serde(default)]
    pub want_replies: Vec<WantReply>,
    #[serde(default)]
    pub want_votes: Vec<(Id, Id)>,
}

/// An open write transaction; dropping it without `commit` aborts it.
pub struct WriteTxn(redb::WriteTransaction);

impl WriteTxn {
    pub fn tx(&self) -> Tx<'_> {
        Tx { txn: &self.0 }
    }
    pub fn commit(self) -> Result<()> {
        self.0.commit()?;
        Ok(())
    }
}

pub struct Tx<'a> {
    txn: &'a redb::WriteTransaction,
}

impl Tx<'_> {
    pub fn put_node(&self, n: &Node) -> Result<()> {
        self.txn.open_table(NODES)?.insert(n.id, encode(n).as_slice())?;
        Ok(())
    }
    pub fn del_node(&self, id: Id) -> Result<()> {
        self.txn.open_table(NODES)?.remove(id)?;
        Ok(())
    }
    pub fn put_session(&self, s: &Session) -> Result<()> {
        self.txn.open_table(SESSIONS)?.insert(s.hash.as_slice(), encode(s).as_slice())?;
        Ok(())
    }
    pub fn del_session(&self, hash: &[u8; 32]) -> Result<()> {
        self.txn.open_table(SESSIONS)?.remove(hash.as_slice())?;
        Ok(())
    }
    pub fn del_report(&self, id: Id) -> Result<()> {
        self.txn.open_table(REPORTS)?.remove(id)?;
        Ok(())
    }
    pub fn put_report(&self, r: &Report) -> Result<()> {
        self.txn.open_table(REPORTS)?.insert(r.id, encode(r).as_slice())?;
        Ok(())
    }
    pub fn put_resource(&self, r: &Resource) -> Result<()> {
        self.txn.open_table(RESOURCES)?.insert(r.id, encode(r).as_slice())?;
        Ok(())
    }
    pub fn put_user(&self, u: &User) -> Result<()> {
        self.txn.open_table(USERS)?.insert(u.id, encode(u).as_slice())?;
        Ok(())
    }
    pub fn put_upload(&self, u: &Upload) -> Result<()> {
        self.txn.open_table(UPLOADS)?.insert(u.id, encode(u).as_slice())?;
        Ok(())
    }
    pub fn del_upload(&self, id: Id) -> Result<()> {
        self.txn.open_table(UPLOADS)?.remove(id)?;
        Ok(())
    }
    pub fn put_blob(&self, b: &Blob) -> Result<()> {
        self.txn.open_table(BLOBS)?.insert(b.key.as_str(), encode(b).as_slice())?;
        Ok(())
    }
    pub fn del_blob(&self, key: &str) -> Result<()> {
        self.txn.open_table(BLOBS)?.remove(key)?;
        Ok(())
    }
    pub fn put_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.txn.open_table(META)?.insert(key, value)?;
        Ok(())
    }
    pub fn put_review(&self, e: &ReviewEvent) -> Result<()> {
        self.txn.open_table(REVIEWS)?.insert(e.id, encode(e).as_slice())?;
        Ok(())
    }
    pub fn put_resource_change_request(&self, r: &ResourceChangeRequest) -> Result<()> {
        self.txn.open_table(RESOURCE_CHANGE_REQUESTS)?.insert(r.id, encode(r).as_slice())?;
        Ok(())
    }
    pub fn put_rating(&self, r: &Rating) -> Result<()> {
        self.txn.open_table(RATINGS)?.insert(rating_key(r.resource, r.user).as_slice(), encode(r).as_slice())?;
        Ok(())
    }
    pub fn del_rating(&self, resource: Id, user: Id) -> Result<()> {
        self.txn.open_table(RATINGS)?.remove(rating_key(resource, user).as_slice())?;
        Ok(())
    }
    pub fn put_comment(&self, c: &Comment) -> Result<()> {
        self.txn.open_table(COMMENTS)?.insert(c.id, encode(c).as_slice())?;
        Ok(())
    }
    pub fn put_feedback(&self, f: &Feedback) -> Result<()> {
        self.txn.open_table(FEEDBACK)?.insert(f.id, encode(f).as_slice())?;
        Ok(())
    }
    pub fn put_token(&self, t: &ApiToken) -> Result<()> {
        self.txn.open_table(TOKENS)?.insert(t.id, encode(t).as_slice())?;
        Ok(())
    }
    pub fn put_thumb(&self, t: &Thumb) -> Result<()> {
        self.txn.open_table(THUMBS)?.insert(t.key.as_str(), encode(t).as_slice())?;
        Ok(())
    }
    pub fn put_subtitle(&self, id: Id, s: &str) -> Result<()> {
        self.txn.open_table(SUBTITLES)?.insert(id, s)?;
        Ok(())
    }
    /// 0 clears the node's own level (it then inherits its parent's).
    pub fn put_node_level(&self, id: Id, level: u8) -> Result<()> {
        let mut t = self.txn.open_table(NODE_LEVELS)?;
        if level == 0 { t.remove(id)?; } else { t.insert(id, level)?; }
        Ok(())
    }
    /// Sets (or, with "", clears) the major a resource is for.
    pub fn put_major(&self, id: Id, major: &str) -> Result<()> {
        let mut t = self.txn.open_table(RESOURCE_MAJORS)?;
        if major.is_empty() { t.remove(id)?; } else { t.insert(id, major)?; }
        Ok(())
    }
    pub fn put_link(&self, l: &Link) -> Result<()> {
        self.txn.open_table(LINKS)?.insert(l.id, encode(l).as_slice())?;
        Ok(())
    }
    pub fn del_link(&self, id: Id) -> Result<()> {
        self.txn.open_table(LINKS)?.remove(id)?;
        Ok(())
    }
    pub fn put_want(&self, w: &Want) -> Result<()> {
        self.txn.open_table(WANTS)?.insert(w.id, encode(w).as_slice())?;
        Ok(())
    }
    pub fn put_want_reply(&self, r: &WantReply) -> Result<()> {
        self.txn.open_table(WANT_REPLIES)?.insert(r.id, encode(r).as_slice())?;
        Ok(())
    }
    pub fn put_want_vote(&self, want: Id, user: Id, on: bool) -> Result<()> {
        let mut t = self.txn.open_table(WANT_VOTES)?;
        let k = rating_key(want, user);
        if on { t.insert(k.as_slice(), 1u8)?; } else { t.remove(k.as_slice())?; }
        Ok(())
    }
    pub fn put_question(&self, q: &Question) -> Result<()> {
        self.txn.open_table(QUESTIONS)?.insert(q.id, encode(q).as_slice())?;
        Ok(())
    }
    pub fn put_avatar(&self, a: &Avatar) -> Result<()> {
        self.txn.open_table(AVATARS)?.insert(a.user, encode(a).as_slice())?;
        Ok(())
    }
    pub fn put_public_uploader(&self, id: Id, public: bool) -> Result<()> {
        let mut t = self.txn.open_table(PUBLIC_UPLOADERS)?;
        if public { t.insert(id, 1u8)?; } else { t.remove(id)?; }
        Ok(())
    }
    pub fn del_token(&self, id: Id) -> Result<()> {
        self.txn.open_table(TOKENS)?.remove(id)?;
        Ok(())
    }
}
