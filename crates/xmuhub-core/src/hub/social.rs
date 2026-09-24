//! Ratings, comments, site feedback and the review audit trail.

use serde::Serialize;

use super::{Hub, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

const COMMENT_MAX: usize = 500;
const COMMENT_GAP: i64 = 10;
const COMMENTS_PER_DAY: usize = 60;
const FEEDBACK_OPEN_PER_IP: usize = 10;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct RatingSummary {
    /// Mean stars, one decimal; 0 when unrated.
    pub avg: f32,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommentView {
    pub id: Id,
    pub nickname: String,
    pub body: String,
    pub created_at: i64,
    pub mine: bool,
    pub can_delete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewView {
    pub id: Id,
    pub resource: Id,
    pub title: String,
    pub actor: String,
    pub action: String,
    pub note: String,
    pub at: i64,
}

impl Hub {
    fn visible(&self, viewer: Viewer, resource: Id) -> Result<()> {
        let st = self.st.read();
        st.resources.get(&resource).filter(|r| viewer.can_see(r)).map(|_| ()).ok_or(Error::NotFound("资料"))
    }

    pub fn rating(&self, resource: Id) -> RatingSummary {
        let st = self.st.read();
        let Some(m) = st.ratings.get(&resource).filter(|m| !m.is_empty()) else { return RatingSummary::default() };
        let sum: u32 = m.values().map(|&s| s as u32).sum();
        RatingSummary { avg: (sum as f32 * 10.0 / m.len() as f32).round() / 10.0, count: m.len() }
    }

    pub fn my_rating(&self, viewer: Viewer, resource: Id) -> u8 {
        let Some(me) = viewer.id() else { return 0 };
        self.st.read().ratings.get(&resource).and_then(|m| m.get(&me)).copied().unwrap_or(0)
    }

    /// Sets (1–5) or clears (0) the viewer's rating.
    pub fn rate(&self, viewer: Viewer, resource: Id, stars: u8) -> Result<RatingSummary> {
        let me = viewer.at_least(Level::Contributor)?.id;
        if stars > 5 {
            return Err(bad("评分为 1–5 星"));
        }
        self.visible(viewer, resource)?;
        self.mutate(|st, tx| {
            if stars == 0 {
                tx.del_rating(resource, me)?;
                if let Some(m) = st.ratings.get_mut(&resource) {
                    m.remove(&me);
                }
            } else {
                tx.put_rating(&Rating { resource, user: me, stars, at: now() })?;
                st.ratings.entry(resource).or_default().insert(me, stars);
            }
            Ok(())
        })?;
        Ok(self.rating(resource))
    }

    pub fn comments(&self, viewer: Viewer, resource: Id) -> Result<Vec<CommentView>> {
        self.visible(viewer, resource)?;
        let st = self.st.read();
        let me = viewer.id();
        Ok(st
            .comments_by_res
            .get(&resource)
            .into_iter()
            .flatten()
            .filter_map(|id| st.comments.get(id))
            .filter(|c| c.deleted_by.is_none())
            .map(|c| CommentView {
                id: c.id,
                nickname: st.users.get(&c.user).map(|u| u.nickname.clone()).unwrap_or_default(),
                body: c.body.clone(),
                created_at: c.created_at,
                mine: me == Some(c.user),
                can_delete: me == Some(c.user) || viewer.staff(),
            })
            .collect())
    }

    pub fn add_comment(&self, viewer: Viewer, resource: Id, body: &str) -> Result<Comment> {
        let me = viewer.at_least(Level::Contributor)?.id;
        // Keep line breaks, drop other control characters.
        let body: String = body.trim().chars().filter(|c| *c == '\n' || !c.is_control()).take(COMMENT_MAX).collect();
        if body.is_empty() {
            return Err(bad("评论不能为空"));
        }
        self.visible(viewer, resource)?;
        self.mutate(|st, tx| {
            let t = now();
            let mine: Vec<&Comment> = st.comments.values().filter(|c| c.user == me && t - c.created_at < 86400).collect();
            if mine.iter().any(|c| t - c.created_at < COMMENT_GAP) {
                return Err(Error::TooMany("评论太快了，请稍等几秒".into()));
            }
            if mine.len() >= COMMENTS_PER_DAY {
                return Err(Error::TooMany("今天的评论次数已达上限".into()));
            }
            let c = Comment { id: st.next_id(tx)?, resource, user: me, body, created_at: t, deleted_by: None };
            tx.put_comment(&c)?;
            st.comments_by_res.entry(resource).or_default().push(c.id);
            st.comments.insert(c.id, c.clone());
            Ok(c)
        })
    }

    /// Authors delete their own comments; staff delete anyone's.
    pub fn delete_comment(&self, viewer: Viewer, id: Id) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let mut c = st.comments.get(&id).cloned().ok_or(Error::NotFound("评论"))?;
            if c.user != me && !viewer.staff() {
                return Err(Error::Forbidden);
            }
            c.deleted_by = Some(me);
            tx.put_comment(&c)?;
            st.comments.insert(id, c);
            Ok(())
        })
    }

    // ---------------------------------------------------------------- feedback

    pub fn submit_feedback(&self, viewer: Viewer, body: &str, contact: &str, page: &str, ip: &str) -> Result<Feedback> {
        let body: String = body.trim().chars().filter(|c| *c == '\n' || !c.is_control()).take(2000).collect();
        if body.chars().count() < 4 {
            return Err(bad("请多写几个字，说清楚问题或建议"));
        }
        self.mutate(|st, tx| {
            if st.feedback.values().filter(|f| f.ip == ip && !f.handled).count() >= FEEDBACK_OPEN_PER_IP {
                return Err(Error::TooMany("你提交的反馈较多，请等待处理".into()));
            }
            let f = Feedback {
                id: st.next_id(tx)?,
                user: viewer.id(),
                body,
                contact: clean(contact, 100),
                page: clean(page, 200),
                ip: clean(ip, 64),
                created_at: now(),
                handled: false,
                handled_by: None,
                handled_note: String::new(),
            };
            tx.put_feedback(&f)?;
            st.feedback.insert(f.id, f.clone());
            Ok(f)
        })
    }

    /// Feedback, newest first, with the sender's nickname (if signed in).
    pub fn feedback(&self, actor: Viewer, include_handled: bool) -> Result<Vec<(Feedback, String)>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<(Feedback, String)> = st
            .feedback
            .values()
            .filter(|f| include_handled || !f.handled)
            .map(|f| (f.clone(), f.user.and_then(|u| st.users.get(&u)).map(|u| u.nickname.clone()).unwrap_or_default()))
            .collect();
        v.sort_by_key(|(f, _)| std::cmp::Reverse(f.id));
        Ok(v)
    }

    pub fn handle_feedback(&self, actor: Viewer, id: Id, note: &str) -> Result<()> {
        let me = actor.at_least(Level::Reviewer)?.id;
        self.mutate(|st, tx| {
            let mut f = st.feedback.get(&id).cloned().ok_or(Error::NotFound("反馈"))?;
            f.handled = true;
            f.handled_by = Some(me);
            f.handled_note = clean(note, 300);
            tx.put_feedback(&f)?;
            st.feedback.insert(id, f);
            Ok(())
        })
    }

    // ---------------------------------------------------------------- review audit trail

    /// Review decisions, newest first; `resource` narrows to one resource's history.
    pub fn review_log(&self, actor: Viewer, resource: Option<Id>, limit: usize) -> Result<Vec<ReviewView>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        Ok(st
            .reviews
            .iter()
            .rev()
            .filter(|e| resource.is_none_or(|r| e.resource == r))
            .take(limit)
            .map(|e| ReviewView {
                id: e.id,
                resource: e.resource,
                title: st.resources.get(&e.resource).map(|r| r.name.stem()).unwrap_or_default(),
                actor: st.users.get(&e.actor).map(|u| u.nickname.clone()).unwrap_or_default(),
                action: e.action.clone(),
                note: e.note.clone(),
                at: e.at,
            })
            .collect())
    }
}
