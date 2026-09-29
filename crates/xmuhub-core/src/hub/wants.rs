//! 求资料: students post what they are looking for; others point to a file or leave a hint.
//! A post is public only after a reviewer approves it (staff posts go up at once); replies are
//! like comments, public at once and removable by their author or staff. Only text and
//! resource ids are kept — nothing is fetched.

use serde::Serialize;

use super::{Hub, State, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

const TITLE_MAX: usize = 60;
const BODY_MAX: usize = 500;
/// Posts of one user that are waiting for review or still open.
const OPEN_PER_USER: usize = 5;
const POSTS_PER_DAY: usize = 5;
/// Waiting for review site-wide: past this, new posts wait until reviewers catch up.
const PENDING_MAX: usize = 200;
const REPLY_GAP: i64 = 10;
const REPLIES_PER_DAY: usize = 30;

/// Someone shown next to a post or reply.
#[derive(Debug, Clone, Serialize)]
pub struct Person {
    pub nickname: String,
    pub avatar: Vec<String>,
    /// 「审核员」 / 「管理员」 for staff, so a look-alike nickname can't pass for them.
    pub role: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub id: Id,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WantView {
    pub id: Id,
    pub title: String,
    pub body: String,
    pub node: Option<Brief>,
    pub status: &'static str,
    pub author: Person,
    pub created_at: i64,
    pub updated_at: i64,
    /// 「我也要」 count, and whether the viewer is one of them.
    pub votes: usize,
    pub voted: bool,
    pub replies: usize,
    /// The file that answers it.
    pub resource: Option<Brief>,
    pub mine: bool,
    /// The author or staff may mark it found / closed / open again.
    pub can_manage: bool,
    /// A reviewer's reason for rejecting it (author and staff only).
    pub review_note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WantReplyView {
    pub id: Id,
    pub author: Person,
    pub body: String,
    pub resource: Option<Brief>,
    pub created_at: i64,
    pub can_delete: bool,
}

/// Which posts a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WantFilter {
    /// Still looking, most wanted first.
    Open,
    /// Answered.
    Found,
    /// The viewer's own posts, whatever their state.
    Mine,
    /// Waiting for review (staff).
    Pending,
}

fn text(s: &str, max: usize) -> String {
    // Keep line breaks, drop other control characters.
    s.trim().chars().filter(|c| *c == '\n' || !c.is_control()).take(max).collect()
}

impl Hub {
    pub(super) fn person(&self, st: &State, user: Id) -> Person {
        let u = st.users.get(&user);
        Person {
            nickname: u.map(|u| u.nickname.clone()).unwrap_or_default(),
            avatar: st.avatars.get(&user).map(|a| self.picture_urls(st, &a.current)).unwrap_or_default(),
            role: match u.map(|u| u.level) {
                Some(Level::Admin) => "管理员",
                Some(Level::Reviewer) => "审核员",
                _ => "",
            },
        }
    }

    /// A published file, by id, as a link target.
    fn file_brief(st: &State, id: Option<Id>) -> Option<Brief> {
        let r = st.resources.get(&id?).filter(|r| r.status == Status::Published)?;
        Some(Brief { id: r.id, name: r.name.stem() })
    }

    fn want_visible(viewer: Viewer, w: &Want) -> bool {
        match w.status {
            WantStatus::Open | WantStatus::Found | WantStatus::Closed => true,
            WantStatus::Pending | WantStatus::Rejected => viewer.staff() || viewer.id() == Some(w.user),
        }
    }

    fn want_view(&self, st: &State, viewer: Viewer, w: &Want) -> WantView {
        let me = viewer.id();
        let mine = me == Some(w.user);
        let votes = st.want_votes.get(&w.id);
        WantView {
            id: w.id,
            title: w.title.clone(),
            body: w.body.clone(),
            node: w.node.and_then(|id| st.nodes.get(&id)).map(|n| Brief { id: n.id, name: n.name.clone() }),
            status: w.status.as_str(),
            author: self.person(st, w.user),
            created_at: w.created_at,
            updated_at: w.updated_at,
            votes: votes.map_or(0, |v| v.len()),
            voted: me.is_some_and(|me| votes.is_some_and(|v| v.contains(&me))),
            replies: st.want_replies.values().filter(|r| r.want == w.id && r.deleted_by.is_none()).count(),
            resource: Self::file_brief(st, w.resource),
            mine,
            can_manage: mine || viewer.staff(),
            review_note: if mine || viewer.staff() { w.review_note.clone() } else { String::new() },
        }
    }

    /// Posts for a list, newest activity first (open ones: most 「我也要」 first).
    pub fn wants(&self, viewer: Viewer, filter: WantFilter, node: Option<Id>) -> Result<Vec<WantView>> {
        if filter == WantFilter::Pending && !viewer.staff() {
            return Err(Error::Forbidden);
        }
        if filter == WantFilter::Mine && viewer.id().is_none() {
            return Err(Error::Unauthorized);
        }
        let st = self.st.read();
        let mut v: Vec<&Want> = st
            .wants
            .values()
            .filter(|w| match filter {
                WantFilter::Open => w.status == WantStatus::Open,
                WantFilter::Found => w.status == WantStatus::Found,
                WantFilter::Mine => viewer.id() == Some(w.user),
                WantFilter::Pending => w.status == WantStatus::Pending,
            })
            .filter(|w| node.is_none() || w.node == node)
            // A banned account's posts disappear with it (and come back if unbanned).
            .filter(|w| filter == WantFilter::Mine || st.users.get(&w.user).is_some_and(|u| !u.banned))
            .collect();
        let votes = |w: &Want| st.want_votes.get(&w.id).map_or(0, |v| v.len());
        match filter {
            WantFilter::Open => v.sort_by(|a, b| votes(b).cmp(&votes(a)).then(b.updated_at.cmp(&a.updated_at))),
            WantFilter::Pending => v.sort_by_key(|w| w.created_at),
            _ => v.sort_by_key(|w| std::cmp::Reverse(w.updated_at)),
        }
        Ok(v.into_iter().take(300).map(|w| self.want_view(&st, viewer, w)).collect())
    }

    /// One post with its replies.
    pub fn want(&self, viewer: Viewer, id: Id) -> Result<(WantView, Vec<WantReplyView>)> {
        let st = self.st.read();
        let w = st.wants.get(&id).filter(|w| Self::want_visible(viewer, w)).ok_or(Error::NotFound("求助"))?;
        let me = viewer.id();
        let mut replies: Vec<&WantReply> = st
            .want_replies
            .values()
            .filter(|r| r.want == id && r.deleted_by.is_none())
            .filter(|r| st.users.get(&r.user).is_some_and(|u| !u.banned))
            .collect();
        replies.sort_by_key(|r| r.id);
        let replies = replies
            .into_iter()
            .map(|r| WantReplyView {
                id: r.id,
                author: self.person(&st, r.user),
                body: r.body.clone(),
                resource: Self::file_brief(&st, r.resource),
                created_at: r.created_at,
                can_delete: me == Some(r.user) || viewer.staff(),
            })
            .collect();
        Ok((self.want_view(&st, viewer, w), replies))
    }

    /// Posts a request. It waits for a reviewer unless the poster is staff.
    pub fn add_want(&self, viewer: Viewer, title: &str, body: &str, node: Option<Id>) -> Result<WantView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let title = clean(title, TITLE_MAX);
        let body = text(body, BODY_MAX);
        if title.chars().count() < 2 {
            return Err(bad("请写清楚想要什么资料"));
        }
        let t = now();
        {
            let st = self.st.read();
            if let Some(n) = node
                && !st.nodes.contains_key(&n)
            {
                return Err(Error::NotFound("分类"));
            }
            let own: Vec<&Want> = st.wants.values().filter(|w| w.user == me).collect();
            if own.iter().filter(|w| matches!(w.status, WantStatus::Pending | WantStatus::Open)).count() >= OPEN_PER_USER {
                return Err(Error::TooMany(format!("你已经有 {OPEN_PER_USER} 条求助还没结束，找到或关闭一些再发")));
            }
            if own.iter().filter(|w| t - w.created_at < 86400).count() >= POSTS_PER_DAY {
                return Err(Error::TooMany("今天发的求助已达上限".into()));
            }
            if !viewer.staff() && st.wants.values().filter(|w| w.status == WantStatus::Pending).count() >= PENDING_MAX {
                return Err(Error::TooMany("待审核的求助太多了，请稍后再发".into()));
            }
        }
        let status = if viewer.system() { WantStatus::Open } else { WantStatus::Pending };
        let w = self.mutate(|st, tx| {
            let w = Want {
                id: st.next_id(tx)?,
                user: me,
                node,
                title,
                body,
                created_at: t,
                status,
                reviewed_by: viewer.system().then_some(me),
                review_note: String::new(),
                resource: None,
                updated_at: t,
            };
            tx.put_want(&w)?;
            st.wants.insert(w.id, w.clone());
            Ok(w)
        })?;
        Ok(self.want_view(&self.st.read(), viewer, &w))
    }

    /// A reviewer approves (public) or rejects (with a reason for the author) a post.
    pub fn review_want(&self, viewer: Viewer, id: Id, approve: bool, note: &str) -> Result<()> {
        let me = viewer.at_least(Level::Reviewer)?.id;
        let note = clean(note, 200);
        self.mutate(|st, tx| {
            let mut w = st.wants.get(&id).cloned().ok_or(Error::NotFound("求助"))?;
            if w.user == me {
                return Err(bad("自己发的求助要由其他审核员审核"));
            }
            if w.status != WantStatus::Pending {
                return Err(Error::Conflict("这条求助已经审核过了".into()));
            }
            w.status = if approve { WantStatus::Open } else { WantStatus::Rejected };
            w.reviewed_by = Some(me);
            w.review_note = if approve { String::new() } else { note };
            w.updated_at = now();
            tx.put_want(&w)?;
            st.wants.insert(id, w);
            Ok(())
        })
    }

    /// The author or staff mark a public post found (optionally with the file), closed, or open again.
    pub fn resolve_want(&self, viewer: Viewer, id: Id, status: WantStatus, resource: Option<Id>) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        if !matches!(status, WantStatus::Open | WantStatus::Found | WantStatus::Closed) {
            return Err(bad("状态不对"));
        }
        {
            let st = self.st.read();
            let w = st.wants.get(&id).ok_or(Error::NotFound("求助"))?;
            if w.user != me && !viewer.staff() {
                return Err(Error::Forbidden);
            }
            if matches!(w.status, WantStatus::Pending | WantStatus::Rejected) {
                return Err(bad("审核通过后才能改状态"));
            }
            if resource.is_some() && Self::file_brief(&st, resource).is_none() {
                return Err(Error::NotFound("资料"));
            }
        }
        self.mutate(|st, tx| {
            let mut w = st.wants.get(&id).cloned().ok_or(Error::NotFound("求助"))?;
            w.status = status;
            w.resource = if status == WantStatus::Found { resource } else { None };
            w.updated_at = now();
            tx.put_want(&w)?;
            st.wants.insert(id, w);
            Ok(())
        })
    }

    /// 「我也要」 on or off.
    pub fn vote_want(&self, viewer: Viewer, id: Id, on: bool) -> Result<usize> {
        let me = viewer.at_least(Level::Contributor)?.id;
        {
            let st = self.st.read();
            let w = st.wants.get(&id).ok_or(Error::NotFound("求助"))?;
            if w.status != WantStatus::Open {
                return Err(bad("这条求助已经结束了"));
            }
        }
        self.mutate(|st, tx| {
            tx.put_want_vote(id, me, on)?;
            let v = st.want_votes.entry(id).or_default();
            if on {
                v.insert(me);
            } else {
                v.remove(&me);
            }
            Ok(v.len())
        })
    }

    /// A reply: a hint, and/or the file on the site that has it.
    pub fn reply_want(&self, viewer: Viewer, id: Id, body: &str, resource: Option<Id>) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let body = text(body, BODY_MAX);
        if body.is_empty() && resource.is_none() {
            return Err(bad("回复不能为空"));
        }
        let t = now();
        {
            let st = self.st.read();
            let w = st.wants.get(&id).filter(|w| Self::want_visible(viewer, w)).ok_or(Error::NotFound("求助"))?;
            if matches!(w.status, WantStatus::Pending | WantStatus::Rejected) {
                return Err(bad("审核通过后才能回复"));
            }
            if resource.is_some() && Self::file_brief(&st, resource).is_none() {
                return Err(Error::NotFound("资料"));
            }
            let recent: Vec<&WantReply> = st.want_replies.values().filter(|r| r.user == me && t - r.created_at < 86400).collect();
            if recent.iter().any(|r| t - r.created_at < REPLY_GAP) {
                return Err(Error::TooMany("回复太快了，请稍等几秒".into()));
            }
            if recent.len() >= REPLIES_PER_DAY {
                return Err(Error::TooMany("今天的回复次数已达上限".into()));
            }
        }
        self.mutate(|st, tx| {
            let r = WantReply { id: st.next_id(tx)?, want: id, user: me, body, resource, created_at: t, deleted_by: None };
            tx.put_want_reply(&r)?;
            st.want_replies.insert(r.id, r);
            if let Some(mut w) = st.wants.get(&id).cloned() {
                w.updated_at = t;
                tx.put_want(&w)?;
                st.wants.insert(id, w);
            }
            Ok(())
        })
    }

    /// Authors delete their own replies; staff delete anyone's (kept, marked deleted).
    pub fn delete_want_reply(&self, viewer: Viewer, id: Id) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let mut r = st.want_replies.get(&id).cloned().ok_or(Error::NotFound("回复"))?;
            if r.user != me && !viewer.staff() {
                return Err(Error::Forbidden);
            }
            r.deleted_by = Some(me);
            tx.put_want_reply(&r)?;
            st.want_replies.insert(id, r);
            Ok(())
        })
    }

    /// Posts waiting for a reviewer (for the admin dashboard's counter).
    pub fn pending_wants(&self) -> usize {
        self.st.read().wants.values().filter(|w| w.status == WantStatus::Pending).count()
    }

    // ---------------------------------------------------------------- announcement

    /// The site-wide notice; until an admin sets one, the default below.
    pub fn announcement(&self) -> Announcement {
        self.st.read().announcement.clone().unwrap_or_else(|| Announcement { text: DEFAULT_ANNOUNCEMENT.into(), at: 1_790_000_000 })
    }

    /// Admins set the notice; empty text hides it.
    pub fn set_announcement(&self, viewer: Viewer, text_in: &str) -> Result<Announcement> {
        viewer.at_least(Level::Admin)?;
        let a = Announcement { text: text(text_in, 300), at: now() };
        let bytes = serde_json::to_vec(&a).map_err(|e| Error::Internal(e.to_string()))?;
        self.mutate(|st, tx| {
            tx.put_meta(ANNOUNCEMENT_KEY, &bytes)?;
            st.announcement = Some(a.clone());
            Ok(())
        })?;
        Ok(a)
    }
}

pub(super) const ANNOUNCEMENT_KEY: &str = "announcement";
const DEFAULT_ANNOUNCEMENT: &str = "鹭岛书阁目前只面向厦门大学的同学。资料都由同学上传，不保证准确、完整，下载使用前请自行仔细甄别。";
