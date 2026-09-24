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

/// Account roles. Ordering matters: a higher level implies every lower level's rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Level {
    /// Not signed in: browse, search, download.
    Guest = 0,
    /// Registered account: uploads wait for review before they are visible.
    Contributor = 1,
    /// Promoted by a reviewer: uploads are visible immediately and reviewed afterwards.
    Trusted = 2,
    /// Works the review queue, edits metadata and the category tree, handles reports.
    Reviewer = 3,
    /// Everything, including changing anyone's role.
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
pub struct User {
    pub id: Id,
    /// Lower-cased; unique.
    pub email: String,
    pub nickname: String,
    /// Argon2id PHC string.
    pub password: String,
    pub level: Level,
    pub banned: bool,
    pub created_at: i64,
    pub created_ip: String,
    pub last_login: i64,
    pub uploads: u64,
}

impl User {
    /// Registered with a Xiamen University address.
    pub fn xmu_verified(&self) -> bool {
        self.email.ends_with("@xmu.edu.cn") || self.email.ends_with(".xmu.edu.cn")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// sha256 of the cookie value.
    pub hash: [u8; 32],
    pub user: Id,
    pub created_at: i64,
    pub expires_at: i64,
    pub ip: String,
}

/// What a category-tree node represents; drives page layout, not permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// A 公共课 / B 专业课 / C 教材与参考书 / D 工具模板与校园服务.
    Section,
    /// A1 思政, a college, C1 数学, D1 工具 …
    Group,
    /// A single course (A2-1 微积分I, B 信息学院/数据结构).
    Course,
    /// A level inside a course (I-1, 上, 线代I, 202406).
    Level,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Section => "section",
            NodeKind::Group => "group",
            NodeKind::Course => "course",
            NodeKind::Level => "level",
        }
    }
    pub fn parse(s: &str) -> Option<NodeKind> {
        Some(match s {
            "section" => NodeKind::Section,
            "group" => NodeKind::Group,
            "course" => NodeKind::Course,
            "level" => NodeKind::Level,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// Created by an uploader; visible, awaiting a reviewer's confirmation.
    Pending,
    Active,
    /// Folded into another node; its resources and children were moved there.
    Merged(Id),
}

/// A node of the category tree (目录即分类).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: Id,
    pub parent: Option<Id>,
    pub kind: NodeKind,
    /// Scheme code such as "A2-1" (may be empty).
    pub code: String,
    /// Display name, e.g. "微积分I" or "I-1".
    pub name: String,
    /// Course segment used in file names, e.g. "微积分I-1" (教务全称 + 层次).
    pub label: String,
    /// Search aliases (群内俗称: 思修、史纲、毛概 …).
    pub aliases: Vec<String>,
    /// A 公共课 courses group files into 01–04 buckets on their page.
    pub bucketed: bool,
    pub sort: u32,
    pub status: NodeStatus,
    pub created_by: Id,
    pub created_at: i64,
}

/// Resource type tags T1–T9 of the classification scheme (exactly one per resource).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tag {
    Exam,
    Answer,
    Notes,
    Bank,
    Slides,
    Lab,
    Bundle,
    Book,
    Tool,
}

impl Tag {
    pub const ALL: [Tag; 9] = [
        Tag::Exam,
        Tag::Answer,
        Tag::Notes,
        Tag::Bank,
        Tag::Slides,
        Tag::Lab,
        Tag::Bundle,
        Tag::Book,
        Tag::Tool,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Tag::Exam => "T1",
            Tag::Answer => "T2",
            Tag::Notes => "T3",
            Tag::Bank => "T4",
            Tag::Slides => "T5",
            Tag::Lab => "T6",
            Tag::Bundle => "T7",
            Tag::Book => "T8",
            Tag::Tool => "T9",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tag::Exam => "真题试卷",
            Tag::Answer => "答案与解析",
            Tag::Notes => "提纲笔记",
            Tag::Bank => "题库刷题",
            Tag::Slides => "课件讲义",
            Tag::Lab => "实验资料",
            Tag::Bundle => "打包合集",
            Tag::Book => "电子书",
            Tag::Tool => "工具模板",
        }
    }

    pub fn parse(s: &str) -> Option<Tag> {
        Tag::ALL.into_iter().find(|t| t.code() == s)
    }

    /// Which of the fixed A 公共课 folders (01–04) the tag lives in.
    pub fn bucket(self) -> u8 {
        match self {
            Tag::Exam | Tag::Answer => 1,
            Tag::Notes => 2,
            Tag::Bank => 3,
            _ => 4,
        }
    }
}

/// Closed set of type words used in file names (分类规则 §4) and the tag each implies.
pub const TYPE_WORDS: &[(&str, Tag)] = &[
    ("期中试卷", Tag::Exam),
    ("期末试卷", Tag::Exam),
    ("小测", Tag::Exam),
    ("往年试卷", Tag::Exam),
    ("答案", Tag::Answer),
    ("重点", Tag::Notes),
    ("提纲", Tag::Notes),
    ("笔记", Tag::Notes),
    ("单词表", Tag::Notes),
    ("题库", Tag::Bank),
    ("思考题", Tag::Bank),
    ("课件", Tag::Slides),
    ("讲义", Tag::Slides),
    ("资料", Tag::Slides),
    ("实验报告", Tag::Lab),
    ("合集", Tag::Bundle),
    ("教材", Tag::Book),
    ("模板", Tag::Tool),
    ("工具", Tag::Tool),
];

pub fn type_word_tag(word: &str) -> Option<Tag> {
    TYPE_WORDS.iter().find(|(w, _)| *w == word).map(|(_, t)| *t)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// compliance = review: waiting for a reviewer, invisible to the public.
    Pending,
    /// compliance = public.
    Published,
    Rejected,
    /// Taken down (e.g. after a report); reversible.
    Removed,
    /// compliance = restricted: kept for staff only, never public or searchable.
    Restricted,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Published => "published",
            Status::Rejected => "rejected",
            Status::Removed => "removed",
            Status::Restricted => "restricted",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "pending" => Status::Pending,
            "published" => Status::Published,
            "rejected" => Status::Rejected,
            "removed" => Status::Removed,
            "restricted" => Status::Restricted,
            _ => return None,
        })
    }
}

/// Structured name parts; the file name is generated from these (课程_时间_类型).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NameParts {
    /// Course segment, normally the node's label.
    pub course: String,
    /// "2023-2024秋", "2025春", "2023", "202406" or empty.
    pub time: String,
    /// One of `TYPE_WORDS`.
    pub type_word: String,
    /// "A卷", "B卷" or empty.
    pub paper: String,
    pub with_answer: bool,
    /// Extra bracket detail such as "201题" or "第1册".
    pub extra: String,
    /// 1 = no suffix, 2 = "_v2", …
    pub version: u16,
}

impl NameParts {
    /// The file name stem without version suffix, e.g. `微积分I-1_2023-2024秋_期中试卷(A卷、含答案)`.
    pub fn base(&self) -> String {
        let mut s = self.course.clone();
        if !self.time.is_empty() {
            s.push('_');
            s.push_str(&self.time);
        }
        s.push('_');
        s.push_str(&self.type_word);
        let mut detail: Vec<&str> = Vec::new();
        if !self.paper.is_empty() {
            detail.push(&self.paper);
        }
        if self.with_answer {
            detail.push("含答案");
        }
        if !self.extra.is_empty() {
            detail.push(&self.extra);
        }
        if !detail.is_empty() {
            s.push('(');
            s.push_str(&detail.join("、"));
            s.push(')');
        }
        s
    }

    pub fn stem(&self) -> String {
        if self.version > 1 { format!("{}_v{}", self.base(), self.version) } else { self.base() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub id: Id,
    pub node: Id,
    pub tag: Tag,
    pub name: NameParts,
    /// Lower-case extension without dot ("pdf").
    pub ext: String,
    /// Public note (备注) shown on the resource page.
    pub note: String,
    /// Original upload file name — staff only (分类规则 §5.7).
    pub original_name: String,
    /// Archive / source description — staff only.
    pub source: String,
    /// Category or content not yet verified (the archive's "（不确定）").
    pub uncertain: bool,
    pub blob: String,
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

impl Resource {
    pub fn filename(&self) -> String {
        if self.ext.is_empty() { self.name.stem() } else { format!("{}.{}", self.name.stem(), self.ext) }
    }
}

/// A takedown / problem report (侵权投诉通道).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: Id,
    pub resource: Id,
    pub reason: String,
    pub contact: String,
    pub reporter: Option<Id>,
    pub ip: String,
    pub created_at: i64,
    pub handled: bool,
    pub handled_by: Option<Id>,
    pub handled_note: String,
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
    /// A file inside someone else's public repository, pinned to a commit (imports by
    /// reference). Appended last so existing postcard records keep their variant indices.
    GitHubRepo {
        owner: String,
        repo: String,
        commit: String,
        path: String,
    },
}

impl Location {
    pub fn backend(&self) -> &'static str {
        match self {
            Location::Local { .. } => "local",
            Location::GitHub { .. } => "github",
            Location::GitHubRepo { .. } => "github-repo",
        }
    }

    /// Stored in our own storage (not merely referenced).
    pub fn owned(&self) -> bool {
        !matches!(self, Location::GitHubRepo { .. })
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
    pub user: Id,
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

/// One review decision, kept as an audit trail (who approved / rejected what, and when).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub id: Id,
    pub resource: Id,
    pub actor: Id,
    /// approve / reject / remove / restrict / restore
    pub action: String,
    pub note: String,
    pub at: i64,
}

/// A signed-in user's 1–5 star rating of a resource (one per user and resource).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rating {
    pub resource: Id,
    pub user: Id,
    pub stars: u8,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: Id,
    pub resource: Id,
    pub user: Id,
    pub body: String,
    pub created_at: i64,
    /// Soft-deleted by its author or staff; kept for moderation history.
    pub deleted_by: Option<Id>,
}

/// Site feedback (意见反馈), read by staff in the admin panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub id: Id,
    pub user: Option<Id>,
    pub body: String,
    pub contact: String,
    /// Page the sender came from.
    pub page: String,
    pub ip: String,
    pub created_at: i64,
    pub handled: bool,
    pub handled_by: Option<Id>,
    pub handled_note: String,
}

/// A personal access token for scripts and AI agents (API and MCP). Only its hash is kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: Id,
    pub user: Id,
    pub name: String,
    /// sha256 of the secret.
    pub hash: [u8; 32],
    /// First characters of the secret, to tell tokens apart.
    pub prefix: String,
    pub created_at: i64,
    pub last_used: i64,
}

/// A generated first-page thumbnail of a stored file (keyed by blob), or a record that
/// making one failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumb {
    pub key: String,
    pub loc: Option<Location>,
    /// Failed attempts; files that can't have a thumbnail are marked with a high count.
    pub tries: u8,
    pub at: i64,
}
