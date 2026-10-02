//! 缺资料的课程: searches that found nothing, counted, so the site can ask for what students
//! look for. Terms are whatever someone typed, so none is shown publicly until an admin marks
//! it 公开征集 (or hides it). Kept as one JSON map in the meta table (a few hundred short terms).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Hub, Viewer};
use crate::error::{Error, Result, bad};
use crate::model::*;

pub(super) const MISSING_KEY: &str = "missing_searches";
/// Terms kept; past this the least searched go.
const KEEP: usize = 2000;
const TERM_MAX: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingTerm {
    pub term: String,
    /// Distinct searches (one per address per term per day, counted by the caller).
    pub count: u32,
    pub first: i64,
    pub last: i64,
    /// "new" (only admins see it), "public" (listed on the 缺资料 page) or "hidden".
    pub status: String,
}

/// The form a term is counted under: trimmed, lower-case, single spaces, at most 30 characters.
pub fn missing_key(q: &str) -> Option<String> {
    let t: String = q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().chars().filter(|c| !c.is_control()).take(TERM_MAX).collect();
    (t.chars().count() >= 2).then_some(t)
}

impl Hub {
    fn save_missing(st: &super::State, tx: &crate::db::Tx) -> Result<()> {
        tx.put_meta(MISSING_KEY, &serde_json::to_vec(&st.missing).map_err(|e| Error::Internal(e.to_string()))?)
    }

    /// Counts a search that found nothing (`n` searches at once, for importing old logs).
    pub fn record_missing(&self, q: &str, n: u32) -> Result<()> {
        let Some(term) = missing_key(q) else { return Ok(()) };
        let t = now();
        self.mutate(|st, tx| {
            let e = st.missing.entry(term.clone()).or_insert_with(|| MissingTerm { term, count: 0, first: t, last: t, status: "new".into() });
            e.count = e.count.saturating_add(n);
            e.last = t;
            if st.missing.len() > KEEP {
                // Drop the least searched term that isn't public.
                if let Some(k) = st.missing.values().filter(|m| m.status != "public").min_by_key(|m| (m.count, m.last)).map(|m| m.term.clone()) {
                    st.missing.remove(&k);
                }
            }
            Self::save_missing(st, tx)
        })
    }

    /// Admins: adds counts from elsewhere (searches found in old access logs).
    pub fn import_missing(&self, viewer: Viewer, terms: &[(String, u32)]) -> Result<()> {
        viewer.at_least(Level::Admin)?;
        for (term, count) in terms.iter().take(KEEP) {
            self.record_missing(term, (*count).clamp(1, 10_000))?;
        }
        Ok(())
    }

    /// Admins: every counted term, most searched first.
    pub fn missing_terms(&self, viewer: Viewer) -> Result<Vec<MissingTerm>> {
        viewer.at_least(Level::Admin)?;
        let mut v: Vec<MissingTerm> = self.st.read().missing.values().cloned().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
        Ok(v)
    }

    /// The terms admins chose to show on the 缺资料 page, most searched first.
    pub fn public_missing(&self) -> Vec<MissingTerm> {
        let mut v: Vec<MissingTerm> = self.st.read().missing.values().filter(|m| m.status == "public").cloned().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
        v
    }

    /// Admins: "public" lists the term on the 缺资料 page, "hidden" keeps it off, "new" undoes.
    pub fn set_missing_status(&self, viewer: Viewer, term: &str, status: &str) -> Result<()> {
        viewer.at_least(Level::Admin)?;
        if !matches!(status, "new" | "public" | "hidden") {
            return Err(bad("状态不合法"));
        }
        let key = missing_key(term).ok_or(Error::NotFound("搜索词"))?;
        self.mutate(|st, tx| {
            st.missing.get_mut(&key).ok_or(Error::NotFound("搜索词"))?.status = status.into();
            Self::save_missing(st, tx)
        })
    }

    /// Admins: drops junk terms (garbled input) from the list; searching them again counts anew.
    pub fn delete_missing(&self, viewer: Viewer, terms: &[String]) -> Result<usize> {
        viewer.at_least(Level::Admin)?;
        let keys: Vec<String> = terms.iter().filter_map(|t| missing_key(t)).collect();
        self.mutate(|st, tx| {
            let n = keys.iter().filter(|k| st.missing.remove(*k).is_some()).count();
            Self::save_missing(st, tx)?;
            Ok(n)
        })
    }
}

/// Stored as a map from the counted form.
pub(super) type MissingMap = BTreeMap<String, MissingTerm>;
