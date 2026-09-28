//! 「待整理」: files whose uploader couldn't tell where they belong. They wait in one
//! node that no listing shows (tree, suggestions, search, stats) until a reviewer moves
//! each to its course; approving one in place is refused, so they are never public there.

use super::{Hub, INBOX_KEY, Viewer};
use crate::error::Result;
use crate::model::*;

/// Name (and file-name course segment) of the inbox node.
pub const INBOX_NAME: &str = "待整理";

impl Hub {
    /// The inbox node, created the first time someone needs it.
    pub fn inbox(&self, actor: Viewer) -> Result<Node> {
        let me = actor.at_least(Level::Contributor)?.clone();
        {
            let st = self.st.read();
            if let Some(n) = st.inbox.and_then(|id| st.nodes.get(&id)) {
                return Ok(n.clone());
            }
        }
        let n = self.mutate(|st, tx| {
            // Checked again under the write lock: two first uploads must not make two inboxes.
            if let Some(n) = st.inbox.and_then(|id| st.nodes.get(&id)) {
                return Ok(n.clone());
            }
            let n = Node {
                id: st.next_id(tx)?,
                parent: None,
                kind: NodeKind::Course,
                code: String::new(),
                name: INBOX_NAME.into(),
                label: INBOX_NAME.into(),
                aliases: Vec::new(),
                bucketed: false,
                sort: u32::MAX,
                status: NodeStatus::Active,
                created_by: me.id,
                created_at: now(),
            };
            st.put_node(tx, n.clone())?;
            tx.put_meta(INBOX_KEY, &n.id.to_le_bytes())?;
            st.inbox = Some(n.id);
            Ok(n)
        })?;
        self.reindex(&[n.id], &[]);
        Ok(n)
    }

    pub fn inbox_id(&self) -> Option<Id> {
        self.st.read().inbox
    }
}
