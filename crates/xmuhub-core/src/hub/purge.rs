//! 封禁并清理: an admin bans an abusive account and takes back everything it put on the site in
//! one go. Like every other decision, nothing is erased: files are marked 未通过 / 已下架 (the
//! stored bytes stay, so any of them can be restored one by one), comments and replies are
//! marked deleted, posts and suggestions turned down with the reason.

use serde::Serialize;

use super::{Hub, Viewer, clean};
use crate::error::{Result, bad};
use crate::model::*;

/// What was taken back.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PurgeReport {
    pub rejected: usize,
    pub removed: usize,
    pub comments: usize,
    pub ratings: usize,
    pub wants: usize,
    pub want_replies: usize,
    pub series: usize,
    pub collections: usize,
    pub links: usize,
    pub tokens: usize,
    pub avatar: bool,
}

impl Hub {
    pub fn purge_user(&self, actor: Viewer, id: Id, reason: &str) -> Result<PurgeReport> {
        let me = actor.at_least(Level::Admin)?.id;
        let reason = clean(reason, 200);
        let reason = if reason.is_empty() { "账号因滥用被封禁，内容已清理".to_string() } else { reason };
        // The ban itself: same checks as the users tab (not oneself, not a peer), sessions end.
        self.update_user(actor, id, None, Some(true))?;
        let mut rep = self.mutate(|st, tx| {
            let mut rep = PurgeReport::default();
            let t = now();
            for c in st.comments.values_mut().filter(|c| c.user == id && c.deleted_by.is_none()) {
                c.deleted_by = Some(me);
                tx.put_comment(c)?;
                rep.comments += 1;
            }
            for (resource, by) in st.ratings.iter_mut() {
                if by.remove(&id).is_some() {
                    tx.del_rating(*resource, id)?;
                    rep.ratings += 1;
                }
            }
            for w in st.wants.values_mut().filter(|w| w.user == id && matches!(w.status, WantStatus::Pending | WantStatus::Open)) {
                w.status = WantStatus::Rejected;
                w.reviewed_by = Some(me);
                w.review_note = reason.clone();
                w.updated_at = t;
                tx.put_want(w)?;
                rep.wants += 1;
            }
            for r in st.want_replies.values_mut().filter(|r| r.user == id && r.deleted_by.is_none()) {
                r.deleted_by = Some(me);
                tx.put_want_reply(r)?;
                rep.want_replies += 1;
            }
            for s in st.series.values_mut().filter(|s| s.draft.as_ref().is_some_and(|d| d.by == id)) {
                s.draft = None;
                if s.status == "new" {
                    s.status = "rejected".into();
                }
                s.review_note = reason.clone();
                tx.put_series(s)?;
                rep.series += 1;
            }
            for c in st.collections.values_mut().filter(|c| c.user == id && matches!(c.status.as_str(), "public" | "pending")) {
                c.status = "private".into();
                tx.put_collection(c)?;
                rep.collections += 1;
            }
            for s in st.link_suggestions.values_mut().filter(|s| s.user == id && s.status == "pending") {
                s.status = "rejected".into();
                s.reviewed_by = Some(me);
                s.review_note = reason.clone();
                tx.put_link_suggestion(s)?;
                rep.links += 1;
            }
            let tokens: Vec<Id> = st.tokens.values().filter(|k| k.user == id).map(|k| k.id).collect();
            for k in tokens {
                if let Some(k) = st.tokens.remove(&k) {
                    st.token_by_hash.remove(&k.hash);
                    tx.del_token(k.id)?;
                    rep.tokens += 1;
                }
            }
            if let Some(a) = st.avatars.get_mut(&id)
                && (!a.current.is_empty() || !a.pending.is_empty())
            {
                a.current.clear();
                a.pending.clear();
                tx.put_avatar(a)?;
                rep.avatar = true;
            }
            Ok(rep)
        })?;
        // Files go through the ordinary review path (search index, counters, review log).
        let files: Vec<(Id, Status)> = {
            let st = self.st.read();
            st.resources.values().filter(|r| r.uploader == id && matches!(r.status, Status::Pending | Status::Published)).map(|r| (r.id, r.status)).collect()
        };
        for (rid, status) in files {
            let action = if status == Status::Pending { "reject" } else { "remove" };
            match self.review(actor, rid, action, &reason) {
                Ok(_) if action == "reject" => rep.rejected += 1,
                Ok(_) => rep.removed += 1,
                Err(e) => return Err(bad(format!("已封禁并清理了一部分，资料 #{rid} 没处理成功：{e}"))),
            }
        }
        Ok(rep)
    }
}
