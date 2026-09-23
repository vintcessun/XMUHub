//! Persistent records. Every record is stored as `[SCHEMA_VERSION] ++ postcard(record)`;
//! bump the version and add a migration arm in `db::decode` when a layout changes.

use serde::{Deserialize, Serialize};

pub type Id = u64;

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Permission tiers. Ordering matters: a higher level implies every lower level's rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Level {
    /// No token: browse, search, download.
    Guest = 0,
    /// Self-claimed token: uploads wait for review before they are visible.
    Contributor = 1,
    /// Promoted by a reviewer: uploads are visible immediately and reviewed afterwards.
    Trusted = 2,
    /// Works the review queue, edits metadata, merges courses, promotes/bans L1–L2.
    Reviewer = 3,
    /// Issues/revokes any token, manages storage and mirrors.
    Admin = 4,
}

impl Level {
    pub fn from_u8(v: u8) -> Option<Level> {
        Some(match v {
            0 => Level::Guest,
            1 => Level::Contributor,
            2 => Level::Trusted,
            3 => Level::Reviewer,
            4 => Level::Admin,
            _ => return None,
        })
    }

    /// Whether uploads at this level are published before review.
    pub fn publishes_directly(self) -> bool {
        self >= Level::Trusted
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub id: Id,
    /// sha256 of the secret token string; the secret itself is never stored.
    pub hash: [u8; 32],
    pub level: Level,
    pub label: String,
    pub banned: bool,
    pub created_at: i64,
    pub created_ip: String,
    pub uploads: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CourseStatus {
    /// Created by an upload, not yet confirmed by a reviewer. Still browsable.
    Pending,
    Active,
    /// Folded into another course; resources were moved there.
    Merged(Id),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    pub id: Id,
    /// Official course code (may be empty when unknown).
    pub code: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub college: String,
    pub status: CourseStatus,
    pub created_by: Id,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Exam,
    Notes,
    Slides,
    Homework,
    Lab,
    Textbook,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Exam,
        Kind::Notes,
        Kind::Slides,
        Kind::Homework,
        Kind::Lab,
        Kind::Textbook,
        Kind::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Exam => "exam",
            Kind::Notes => "notes",
            Kind::Slides => "slides",
            Kind::Homework => "homework",
            Kind::Lab => "lab",
            Kind::Textbook => "textbook",
            Kind::Other => "other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Exam => "试卷",
            Kind::Notes => "笔记",
            Kind::Slides => "课件",
            Kind::Homework => "作业",
            Kind::Lab => "实验",
            Kind::Textbook => "教材",
            Kind::Other => "其他",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Waiting for review, invisible to guests.
    Pending,
    Published,
    Rejected,
    Removed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Published => "published",
            Status::Rejected => "rejected",
            Status::Removed => "removed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub id: Id,
    pub course_id: Id,
    pub title: String,
    pub kind: Kind,
    pub year: Option<u16>,
    /// 1 = autumn, 2 = spring, 3 = summer.
    pub term: Option<u8>,
    pub teacher: String,
    pub description: String,
    /// Content hash key into the blob table.
    pub blob: String,
    pub filename: String,
    pub size: u64,
    pub mime: String,
    pub status: Status,
    /// Published before review (Trusted uploads) and not yet looked at.
    pub needs_review: bool,
    pub review_note: String,
    pub uploader: Id,
    pub reviewed_by: Option<Id>,
    pub created_at: i64,
    pub updated_at: i64,
    pub downloads: u64,
}

/// Where one stored part physically lives. A part may have several replicas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    Local {
        path: String,
    },
    GitHub {
        owner: String,
        repo: String,
        release_id: u64,
        tag: String,
        asset_id: u64,
        name: String,
    },
}

impl Location {
    pub fn backend(&self) -> &'static str {
        match self {
            Location::Local { .. } => "local",
            Location::GitHub { .. } => "github",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub size: u64,
    pub sha256: String,
    pub replicas: Vec<Location>,
}

/// A stored file, keyed by content hash so identical uploads are stored once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    pub key: String,
    pub size: u64,
    pub parts: Vec<Part>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPart {
    pub size: u64,
    pub sha256: String,
    /// Where the uploader was told to put this part.
    pub target: Location,
    pub done: bool,
}

/// An upload in flight: storage slots reserved, bytes travelling browser → storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upload {
    pub id: Id,
    pub token: Id,
    pub filename: String,
    pub mime: String,
    pub size: u64,
    pub key: String,
    pub parts: Vec<PendingPart>,
    pub created_at: i64,
    /// Set once every part is confirmed and the blob row exists.
    pub finished: bool,
    /// Set once a resource was created from this upload.
    pub consumed: bool,
}
