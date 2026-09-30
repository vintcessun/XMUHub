//! 建议换个分类: anyone signed in can say a published file belongs in another course. The
//! suggestion waits for a reviewer other than its author, who moves the file there (through
//! the ordinary move, so names and counts follow) or turns it down with a reason; the author
//! hears back in 站内提醒.

use serde::Serialize;

use super::wants::Person;
use super::{Hub, State, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

const PENDING_PER_USER: usize = 20;
const PER_DAY: usize = 50;
const PENDING_MAX: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct MoveSuggestionView {
    pub id: Id,
    pub resource: Id,
    pub title: String,
    /// Where the file is now (it may have moved since) and where it should go, as paths.
    pub from: String,
    pub from_id: Id,
    pub to: String,
    pub to_id: Id,
    pub note: String,
    pub created_at: i64,
    pub status: String,
    pub review_note: String,
    pub by: Person,
    pub mine: bool,
}

impl Hub {
    fn move_view(&self, st: &State, viewer: Viewer, s: &MoveSuggestion) -> MoveSuggestionView {
        let r = st.resources.get(&s.resource);
        let now_at = r.map(|r| r.node).unwrap_or(s.from);
        MoveSuggestionView {
            id: s.id,
            resource: s.resource,
            title: r.map(|r| r.name.stem()).unwrap_or_default(),
            from: st.path_text(now_at),
            from_id: now_at,
            to: st.path_text(s.to),
            to_id: s.to,
            note: s.note.clone(),
            created_at: s.created_at,
            status: s.status.clone(),
            review_note: s.review_note.clone(),
            by: self.person(st, s.user),
            mine: viewer.user.is_some_and(|u| u.id == s.user),
        }
    }

    /// Suggests moving a published file to another course or level.
    pub fn suggest_move(&self, viewer: Viewer, resource: Id, to: Id, note: &str) -> Result<MoveSuggestionView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let note = clean(note, 200);
        let t = now();
        let from = {
            let st = self.st.read();
            let r = st.resources.get(&resource).filter(|r| r.status == Status::Published).ok_or(Error::NotFound("资料"))?;
            let target = st.resolve(to).ok_or(Error::NotFound("目标分类"))?;
            if target.kind == NodeKind::Section {
                return Err(bad("请选择具体的课程，不能直接放在栏目下"));
            }
            if target.id == r.node {
                return Err(bad("资料已经在这个分类里了"));
            }
            let own: Vec<&MoveSuggestion> = st.move_suggestions.values().filter(|s| s.user == me).collect();
            if own.iter().filter(|s| s.status == "pending").count() >= PENDING_PER_USER {
                return Err(Error::TooMany(format!("你已经有 {PENDING_PER_USER} 条分类建议在等审核，审核后再提")));
            }
            if own.iter().filter(|s| t - s.created_at < 86400).count() >= PER_DAY {
                return Err(Error::TooMany("今天提的分类建议已达上限".into()));
            }
            let pending = st.move_suggestions.values().filter(|s| s.status == "pending");
            if pending.clone().count() >= PENDING_MAX {
                return Err(Error::TooMany("待审核的分类建议太多了，请稍后再来".into()));
            }
            if pending.clone().any(|s| s.resource == resource) {
                return Err(Error::Conflict("已经有人建议给这份资料换分类了，正在等审核".into()));
            }
            r.node
        };
        let to = self.st.read().resolve(to).map(|n| n.id).unwrap_or(to);
        let s = self.mutate(|st, tx| {
            let s = MoveSuggestion {
                id: st.next_id(tx)?,
                resource,
                user: me,
                from,
                to,
                note,
                created_at: t,
                status: "pending".into(),
                reviewed_by: None,
                review_note: String::new(),
            };
            tx.put_move_suggestion(&s)?;
            st.move_suggestions.insert(s.id, s.clone());
            Ok(s)
        })?;
        Ok(self.move_view(&self.st.read(), viewer, &s))
    }

    /// Staff: the suggestions waiting for review, oldest first. Anyone else: their own, newest first.
    pub fn move_suggestions(&self, viewer: Viewer) -> Result<Vec<MoveSuggestionView>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<&MoveSuggestion> = if viewer.staff() {
            st.move_suggestions.values().filter(|s| s.status == "pending").collect()
        } else {
            st.move_suggestions.values().filter(|s| s.user == me).collect()
        };
        v.sort_by_key(|s| s.id);
        if !viewer.staff() {
            v.reverse();
        }
        Ok(v.into_iter().map(|s| self.move_view(&st, viewer, s)).collect())
    }

    /// A reviewer moves the file where suggested, or turns the suggestion down with a reason.
    /// Nobody reviews their own suggestion.
    pub fn review_move_suggestion(&self, viewer: Viewer, id: Id, approve: bool, note: &str) -> Result<()> {
        let me = viewer.at_least(Level::Reviewer)?.id;
        let reason = clean(note, 200);
        if !approve && reason.is_empty() {
            return Err(bad("请写明不采纳的原因"));
        }
        let s = self.st.read().move_suggestions.get(&id).cloned().ok_or(Error::NotFound("分类建议"))?;
        if s.status != "pending" {
            return Err(Error::Conflict("这条建议已经处理过了".into()));
        }
        if s.user == me {
            return Err(bad("不能审核自己提的建议"));
        }
        if approve {
            self.move_resources(viewer, &[s.resource], s.to)?;
        }
        self.mutate(|st, tx| {
            let mut s = st.move_suggestions.get(&id).cloned().ok_or(Error::NotFound("分类建议"))?;
            s.status = if approve { "approved" } else { "rejected" }.into();
            s.reviewed_by = Some(me);
            s.review_note = reason.clone();
            tx.put_move_suggestion(&s)?;
            let title = st.resources.get(&s.resource).map(|r| r.name.stem()).unwrap_or_default();
            let text = if approve {
                format!("你建议的分类调整已采纳：「{title}」已移到 {}", st.path_text(s.to))
            } else {
                format!("你对「{title}」的分类建议未被采纳：{reason}")
            };
            let (user, link) = (s.user, format!("/r/{}", s.resource));
            st.move_suggestions.insert(s.id, s);
            st.notify(tx, user, "move", None, text, link)
        })
    }
}
