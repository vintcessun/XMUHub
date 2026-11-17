//! Account-scoped download markers, queried directly from redb rather than cached at boot.

use super::{Hub, State, Viewer};
use crate::error::{Error, Result};
use crate::model::{Id, Level, Status, User, now};

fn account<'a>(st: &'a State, viewer: Viewer<'_>) -> Result<&'a User> {
    let supplied = viewer.at_least(Level::Contributor)?;
    let user = st.users.get(&supplied.id).ok_or(Error::Unauthorized)?;
    if supplied.banned || user.banned || user.level < Level::Contributor {
        return Err(Error::Forbidden);
    }
    Ok(user)
}

impl Hub {
    /// Marks an accessible resource for the signed-in account (not a download counter).
    /// Repeating the operation updates the timestamp, never creates a duplicate marker.
    pub fn record_download(&self, viewer: Viewer<'_>, resource: Id) -> Result<()> {
        self.mutate(|st, tx| {
            let user = account(st, viewer)?;
            let current = Viewer { user: Some(user) };
            let r = st.resources.get(&resource)
                .filter(|r| current.can_see(r) && r.status != Status::Rejected && st.nodes.contains_key(&r.node))
                .ok_or(Error::NotFound("资料"))?;
            if !st.blobs.contains_key(&r.blob) {
                return Err(Error::NotFound("文件"));
            }
            tx.put_download(user.id, r.id, now())
        })
    }

    /// Returns only this account's resource ids. Guests and banned accounts are denied;
    /// administrators have no special access to anyone else's history.
    pub fn downloaded_resources(&self, viewer: Viewer<'_>) -> Result<Vec<Id>> {
        let st = self.st.read();
        let user = account(&st, viewer)?;
        self.db.downloaded_resources(user.id)
    }
}
