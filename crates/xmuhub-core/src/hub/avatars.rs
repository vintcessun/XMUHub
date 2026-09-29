//! Profile pictures. The browser crops the picture and sends it through the normal upload
//! pipeline (to GitHub like any file, never stored on this server); here it only becomes
//! the user's avatar. A new picture waits for a reviewer while the approved one stays up.
//! Replaced or rejected pictures are never deleted from storage (AGENTS.md §4).

use serde::Serialize;

use super::{Hub, State, Viewer};
use crate::error::{Error, Result, bad};
use crate::model::*;

/// The browser sends a 256×256 WebP / JPEG, a few tens of KB.
pub const AVATAR_MAX_BYTES: u64 = 400 * 1024;

/// A picture waiting for review, for the admin page.
#[derive(Debug, Clone, Serialize)]
pub struct PendingAvatar {
    pub user: Id,
    pub nickname: String,
    pub at: i64,
    pub pending: Vec<String>,
    pub current: Vec<String>,
}

impl Hub {
    /// Mirror URLs of a picture's blob (empty when there is none).
    pub(super) fn picture_urls(&self, st: &State, key: &str) -> Vec<String> {
        if key.is_empty() {
            return Vec::new();
        }
        st.blobs.get(key).and_then(|b| b.parts.first()).map(|p| self.storage.download_urls(&p.replicas)).unwrap_or_default()
    }

    /// The approved picture everyone sees.
    pub fn avatar_of(&self, user: Id) -> Vec<String> {
        let st = self.st.read();
        st.avatars.get(&user).map(|a| self.picture_urls(&st, &a.current)).unwrap_or_default()
    }

    /// (approved, waiting for review) pictures of a user, for themselves.
    pub fn own_avatar(&self, user: Id) -> (Vec<String>, Vec<String>) {
        let st = self.st.read();
        match st.avatars.get(&user) {
            Some(a) => (self.picture_urls(&st, &a.current), self.picture_urls(&st, &a.pending)),
            None => (Vec::new(), Vec::new()),
        }
    }

    /// Makes a finished upload the caller's new picture. Staff pictures need no review.
    pub fn set_avatar(&self, actor: Viewer, upload_id: Id) -> Result<()> {
        let me = actor.at_least(Level::Contributor)?.clone();
        self.mutate(|st, tx| {
            let mut up = st.uploads.get(&upload_id).cloned().ok_or(Error::NotFound("上传记录"))?;
            if up.user != me.id {
                return Err(Error::Forbidden);
            }
            if up.consumed {
                return Err(Error::Conflict("这个上传已经用过了".into()));
            }
            if !up.finished || !st.blobs.contains_key(&up.key) {
                return Err(bad("图片还没有上传完成"));
            }
            if up.size > AVATAR_MAX_BYTES || !up.mime.starts_with("image/") {
                return Err(bad("头像需要是 400 KB 以内的图片"));
            }
            up.consumed = true;
            tx.put_upload(&up)?;
            st.uploads.insert(up.id, up.clone());
            let mut a = st.avatars.get(&me.id).cloned().unwrap_or_else(|| Avatar { user: me.id, ..Default::default() });
            if actor.system() {
                a.current = up.key;
                a.pending.clear();
                a.reviewed_by = Some(me.id);
            } else {
                a.pending = up.key;
            }
            a.pending_at = now();
            tx.put_avatar(&a)?;
            st.avatars.insert(me.id, a);
            Ok(())
        })
    }

    /// Takes down the caller's own picture (and any waiting for review).
    pub fn clear_avatar(&self, actor: Viewer) -> Result<()> {
        let me = actor.at_least(Level::Contributor)?.clone();
        self.mutate(|st, tx| {
            if let Some(mut a) = st.avatars.get(&me.id).cloned() {
                a.current.clear();
                a.pending.clear();
                tx.put_avatar(&a)?;
                st.avatars.insert(me.id, a);
            }
            Ok(())
        })
    }

    pub fn pending_avatars(&self, actor: Viewer) -> Result<Vec<PendingAvatar>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut out: Vec<PendingAvatar> = st
            .avatars
            .values()
            .filter(|a| !a.pending.is_empty())
            .map(|a| PendingAvatar {
                user: a.user,
                nickname: st.users.get(&a.user).map(|u| u.nickname.clone()).unwrap_or_default(),
                at: a.pending_at,
                pending: self.picture_urls(&st, &a.pending),
                current: self.picture_urls(&st, &a.current),
            })
            .collect();
        out.sort_by_key(|p| p.at);
        Ok(out)
    }

    /// approve (the waiting picture goes up) / reject (it is dropped) / remove (the
    /// approved picture comes down, e.g. one found inappropriate later).
    pub fn review_avatar(&self, actor: Viewer, user: Id, action: &str) -> Result<()> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        self.mutate(|st, tx| {
            let mut a = st.avatars.get(&user).cloned().ok_or(Error::NotFound("头像"))?;
            if action == "approve" && user == me.id {
                return Err(bad("自己的头像要由其他审核员审核"));
            }
            match action {
                "approve" if !a.pending.is_empty() => a.current = std::mem::take(&mut a.pending),
                "reject" => a.pending.clear(),
                "remove" => a.current.clear(),
                "approve" => return Err(Error::Conflict("没有待审核的头像".into())),
                _ => return Err(bad("未知操作")),
            }
            a.reviewed_by = Some(me.id);
            tx.put_avatar(&a)?;
            st.avatars.insert(user, a);
            Ok(())
        })
    }
}
