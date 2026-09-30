//! How much is waiting in each review queue, for the numbers on the review page's tabs.

use std::collections::BTreeMap;

use super::{Hub, Viewer};
use crate::error::Result;
use crate::model::*;

impl Hub {
    /// Pending items per review tab (keyed like the tabs); complaints and feedback for admins only.
    pub fn review_counts(&self, viewer: Viewer) -> Result<BTreeMap<&'static str, usize>> {
        viewer.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let waiting = |r: &&Resource| r.status == Status::Pending || (r.status == Status::Published && r.needs_review);
        let mut m = BTreeMap::from([
            ("queue", st.resources.values().filter(waiting).count()),
            ("uncertain", st.resources.values().filter(waiting).filter(|r| r.uncertain).count()),
            ("changes", st.resource_change_requests.values().filter(|q| q.status == "pending").count()),
            ("links", st.link_suggestions.values().filter(|s| s.status == "pending").count()),
            ("moves", st.move_suggestions.values().filter(|s| s.status == "pending").count()),
            ("collections", st.collections.values().filter(|c| c.status == "pending").count()),
            ("series", st.series.values().filter(|s| s.draft.is_some()).count()),
            ("avatars", st.avatars.values().filter(|a| !a.pending.is_empty()).count()),
            ("nodes", st.nodes.values().filter(|n| n.status == NodeStatus::Pending).count()),
        ]);
        if viewer.user.is_some_and(|u| u.level == Level::Admin) {
            m.insert("reports", st.reports.values().filter(|r| !r.handled).count());
            m.insert("feedback", st.feedback.values().filter(|f| !f.handled).count());
        }
        Ok(m)
    }
}
