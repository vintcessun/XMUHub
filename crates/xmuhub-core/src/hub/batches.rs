//! Review batches. A reviewer on the 待审资料 page is handed up to `BATCH_SIZE` random files
//! nobody else holds, finishes them before getting more, and gives back whatever is left
//! when they leave (the page stops its heartbeat; after `CLAIM_TTL` the files are free
//! again). 「不懂」 hands one file back and keeps it out of that reviewer's later batches.
//! Batches live in memory only: a restart just returns every file to the pool.
//!
//! Reviewers can also ask a file's uploader a question. The file then waits outside the
//! batches until the uploader answers; the answer shows up next to it for whoever gets it.

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use serde::Serialize;

use super::{Hub, State, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

/// Files handed out at a time.
pub const BATCH_SIZE: usize = 50;
/// Seconds without a heartbeat after which a reviewer's files go back to the pool.
const CLAIM_TTL: i64 = 180;

/// Locks are always taken in the order seen → claims → skipped.
#[derive(Default)]
pub(super) struct Batches {
    /// resource → reviewer holding it
    claims: Mutex<HashMap<Id, Id>>,
    /// reviewer → last heartbeat
    seen: Mutex<HashMap<Id, i64>>,
    /// reviewer → files they passed on (「不懂」)
    skipped: Mutex<HashMap<Id, HashSet<Id>>>,
}

/// A question about a file, as the uploader and reviewers see it.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionView {
    pub id: Id,
    pub resource: Id,
    pub text: String,
    pub asker: String,
    pub asked_at: i64,
    pub answer: String,
    pub answered_at: i64,
}

impl Hub {
    /// Waiting for a reviewer, and something a batch hands out: not an unsorted 「待整理」 /
    /// unverified file (those have their own tab) and not waiting on the uploader's answer.
    fn batchable(st: &State, r: &Resource) -> bool {
        let queued = r.status == Status::Pending || (r.status == Status::Published && r.needs_review);
        let asked = st.questions.values().any(|q| q.resource == r.id && q.answer.is_empty());
        queued && !r.uncertain && st.inbox != Some(r.node) && !asked
    }

    /// The caller's current batch, topped up with a fresh one when it is used up and `take`
    /// is set. `within` narrows new files to one section / college / group (「只审自己学院」).
    /// Every call counts as the page's heartbeat.
    pub fn review_batch(&self, actor: Viewer, within: Option<Id>, take: bool) -> Result<Vec<(Resource, Node)>> {
        let me = actor.at_least(Level::Reviewer)?.id;
        let now = now();
        let st = self.st.read();
        let mut seen = self.batches.seen.lock();
        seen.insert(me, now);
        seen.retain(|_, t| now - *t < CLAIM_TTL);
        let mut claims = self.batches.claims.lock();
        // Files reviewed since, or whose holder left, are no longer held.
        claims.retain(|id, who| seen.contains_key(who) && st.resources.get(id).is_some_and(|r| Self::batchable(&st, r)));
        let mut mine: Vec<Id> = claims.iter().filter(|(_, who)| **who == me).map(|(id, _)| *id).collect();
        if mine.is_empty() && take {
            let skipped = self.batches.skipped.lock();
            let passed = skipped.get(&me);
            let mut pool: Vec<(u64, Id)> = st
                .resources
                .values()
                .filter(|r| Self::batchable(&st, r) && !claims.contains_key(&r.id) && !passed.is_some_and(|p| p.contains(&r.id)))
                // Nobody reviews their own uploads.
                .filter(|r| r.uploader != me)
                .filter(|r| within.is_none_or(|w| r.node == w || st.ancestors(r.node).contains(&w)))
                .map(|r| (rand::random::<u64>(), r.id))
                .collect();
            pool.sort_unstable();
            for (_, id) in pool.into_iter().take(BATCH_SIZE) {
                claims.insert(id, me);
                mine.push(id);
            }
        }
        let mut out: Vec<(Resource, Node)> = mine
            .iter()
            .filter_map(|id| {
                let r = st.resources.get(id)?;
                Some((r.clone(), st.nodes.get(&r.node)?.clone()))
            })
            .collect();
        out.sort_by_key(|(r, _)| (r.created_at, r.id));
        Ok(out)
    }

    /// How many files are free to hand out (for the page's 「池子里还有 N 份」).
    pub fn review_pool_size(&self, within: Option<Id>) -> usize {
        let st = self.st.read();
        let now = now();
        let seen = self.batches.seen.lock();
        let claims = self.batches.claims.lock();
        st.resources
            .values()
            .filter(|r| Self::batchable(&st, r))
            .filter(|r| !claims.get(&r.id).is_some_and(|who| seen.get(who).is_some_and(|t| now - t < CLAIM_TTL)))
            .filter(|r| within.is_none_or(|w| r.node == w || st.ancestors(r.node).contains(&w)))
            .count()
    }

    /// Gives back everything the caller holds (they left the page).
    pub fn release_batch(&self, actor: Viewer) -> Result<()> {
        let me = actor.at_least(Level::Reviewer)?.id;
        // Same lock order as review_batch (seen, then claims).
        self.batches.seen.lock().remove(&me);
        self.batches.claims.lock().retain(|_, who| *who != me);
        Ok(())
    }

    /// 「不懂」: hands one file back for someone else and keeps it out of the caller's batches.
    pub fn skip_in_batch(&self, actor: Viewer, id: Id) -> Result<()> {
        let me = actor.at_least(Level::Reviewer)?.id;
        let mut claims = self.batches.claims.lock();
        if claims.get(&id) == Some(&me) {
            claims.remove(&id);
        }
        self.batches.skipped.lock().entry(me).or_default().insert(id);
        Ok(())
    }

    // ---------------------------------------------------------------- questions

    fn question_view(st: &State, q: &Question) -> QuestionView {
        QuestionView {
            id: q.id,
            resource: q.resource,
            text: q.text.clone(),
            asker: st.users.get(&q.asker).map(|u| u.nickname.clone()).unwrap_or_default(),
            asked_at: q.asked_at,
            answer: q.answer.clone(),
            answered_at: q.answered_at,
        }
    }

    /// A reviewer asks the uploader about a file under review (「问上传者」).
    pub fn ask_uploader(&self, actor: Viewer, resource: Id, text: &str) -> Result<QuestionView> {
        let me = actor.at_least(Level::Reviewer)?.id;
        let text = clean(text, 300);
        if text.chars().count() < 2 {
            return Err(bad("请写下想问的问题"));
        }
        let q = self.mutate(|st, tx| {
            let r = st.resources.get(&resource).ok_or(Error::NotFound("资料"))?;
            if !matches!(r.status, Status::Pending | Status::Published) {
                return Err(bad("这份资料不在审核中"));
            }
            if st.questions.values().any(|q| q.resource == resource && q.answer.is_empty()) {
                return Err(Error::Conflict("已经问过了，正在等上传者回答".into()));
            }
            let (uploader, name) = (r.uploader, r.name.stem());
            let q = Question { id: st.next_id(tx)?, resource, asker: me, text, asked_at: now(), answer: String::new(), answered_at: 0 };
            tx.put_question(&q)?;
            st.questions.insert(q.id, q.clone());
            st.notify(tx, uploader, "question", None, format!("审核员对你上传的「{name}」有个问题，回答后才能继续审核"), format!("/r/{resource}"))?;
            Ok(Self::question_view(st, &q))
        })?;
        // It waits for the answer outside any batch.
        self.batches.claims.lock().remove(&resource);
        Ok(q)
    }

    /// The uploader answers a question about their file; it goes back to the review pool.
    pub fn answer_question(&self, actor: Viewer, id: Id, text: &str) -> Result<QuestionView> {
        let me = actor.at_least(Level::Contributor)?.id;
        let text = clean(text, 500);
        if text.is_empty() {
            return Err(bad("请写下回答"));
        }
        self.mutate(|st, tx| {
            let mut q = st.questions.get(&id).cloned().ok_or(Error::NotFound("问题"))?;
            if st.resources.get(&q.resource).map(|r| r.uploader) != Some(me) {
                return Err(Error::Forbidden);
            }
            q.answer = text;
            q.answered_at = now();
            tx.put_question(&q)?;
            st.questions.insert(q.id, q.clone());
            Ok(Self::question_view(st, &q))
        })
    }

    /// Questions about a file, oldest first, for its uploader and for staff.
    pub fn questions_about(&self, viewer: Viewer, resource: Id) -> Vec<QuestionView> {
        let st = self.st.read();
        let allowed = viewer.staff() || st.resources.get(&resource).is_some_and(|r| viewer.id() == Some(r.uploader));
        if !allowed {
            return Vec::new();
        }
        let mut v: Vec<QuestionView> = st.questions.values().filter(|q| q.resource == resource).map(|q| Self::question_view(&st, q)).collect();
        v.sort_by_key(|q| q.id);
        v
    }

    /// Unanswered questions about the caller's uploads, with the file names (for 「我的」).
    pub fn my_open_questions(&self, actor: Viewer) -> Result<Vec<(QuestionView, String)>> {
        let me = actor.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<(QuestionView, String)> = st
            .questions
            .values()
            .filter(|q| q.answer.is_empty())
            .filter_map(|q| {
                let r = st.resources.get(&q.resource).filter(|r| r.uploader == me)?;
                Some((Self::question_view(&st, q), r.name.stem()))
            })
            .collect();
        v.sort_by_key(|(q, _)| q.id);
        Ok(v)
    }
}
