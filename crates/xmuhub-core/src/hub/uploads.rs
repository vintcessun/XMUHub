//! Uploads: reserve storage slots, hand out upload targets, confirm parts, build blobs.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Hub, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;
use crate::storage::{Receipt, UploadTarget};

/// Unfinished uploads are abandoned after this long.
pub const UPLOAD_TTL: i64 = 24 * 3600;

#[derive(Debug, Clone, Deserialize)]
pub struct PartSpec {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadPlan {
    pub upload_id: Id,
    /// True when identical content is already stored: nothing to send.
    pub dedup: bool,
    pub parts: Vec<PartPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PartPlan {
    pub index: usize,
    pub size: u64,
    pub done: bool,
    pub target: Option<UploadTarget>,
}

fn check_hex64(s: &str) -> Result<()> {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(bad("sha256 必须是 64 位小写十六进制"))
    }
}

/// Content key of a file: its sha256, or for multi-part files `m` + sha256 of the
/// concatenated part hashes (so every part is verifiable on its own).
pub fn content_key(parts: &[PartSpec]) -> String {
    if parts.len() == 1 {
        parts[0].sha256.clone()
    } else {
        let joined: String = parts.iter().map(|p| p.sha256.as_str()).collect();
        format!("m{}", hex::encode(Sha256::digest(joined.as_bytes())))
    }
}

impl Hub {
    fn charge_quota(&self, t: &User, bytes: u64) -> Result<()> {
        let limit = match t.level {
            Level::Guest => return Err(Error::Unauthorized),
            Level::Contributor => self.limits.contributor_daily,
            Level::Trusted => self.limits.trusted_daily,
            Level::Reviewer | Level::Admin => return Ok(()),
        };
        let day = now() / 86400;
        let mut q = self.quota.lock();
        let e = q.entry(t.id).or_insert((day, 0, 0));
        if e.0 != day {
            *e = (day, 0, 0);
        }
        if e.1 + 1 > limit.0 || e.2 + bytes > limit.1 {
            return Err(Error::TooMany("今天的上传额度已用完，明天再来吧".into()));
        }
        e.1 += 1;
        e.2 += bytes;
        Ok(())
    }

    pub async fn begin_upload(&self, actor: Viewer<'_>, filename: &str, mime: &str, parts: Vec<PartSpec>) -> Result<UploadPlan> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let filename = clean(filename, 150);
        if filename.is_empty() {
            return Err(bad("文件名不能为空"));
        }
        if parts.is_empty() || parts.len() > 64 {
            return Err(bad("分片数量不合法"));
        }
        let mut size = 0u64;
        for p in &parts {
            check_hex64(&p.sha256)?;
            if p.size == 0 || p.size > self.limits.max_part {
                return Err(bad("分片大小不合法"));
            }
            size += p.size;
        }
        if size > self.limits.max_file {
            return Err(bad(format!("文件不能超过 {} MB", self.limits.max_file / 1024 / 1024)));
        }
        let key = content_key(&parts);
        let mime = if mime.is_empty() { "application/octet-stream".to_string() } else { clean(mime, 100) };

        let dedup = self.st.read().blobs.contains_key(&key);
        self.charge_quota(&me, if dedup { 0 } else { size })?;

        // Reserve storage slots before touching the database (network I/O, no locks held).
        let mut pending = Vec::with_capacity(parts.len());
        if !dedup {
            for p in &parts {
                let target = self.storage.primary().reserve(&filename, p.size, &p.sha256).await?;
                pending.push(PendingPart { size: p.size, sha256: p.sha256.clone(), target, done: false });
            }
        }
        let up = self.mutate(|st, tx| {
            let up = Upload {
                id: st.next_id(tx)?,
                user: me.id,
                filename: filename.clone(),
                mime,
                size,
                key,
                parts: pending,
                created_at: now(),
                finished: dedup,
                consumed: false,
            };
            tx.put_upload(&up)?;
            st.uploads.insert(up.id, up.clone());
            Ok(up)
        })?;
        self.plan(&up)
    }

    fn plan(&self, up: &Upload) -> Result<UploadPlan> {
        let mut parts = Vec::new();
        for (index, p) in up.parts.iter().enumerate() {
            let target = if p.done { None } else { Some(self.storage.for_location(&p.target)?.upload_target(&p.target, p.size)?) };
            parts.push(PartPlan { index, size: p.size, done: p.done, target });
        }
        Ok(UploadPlan { upload_id: up.id, dedup: up.finished && up.parts.is_empty(), parts })
    }

    /// Current plan of an upload (fresh tickets for parts still to send).
    pub fn upload_plan(&self, actor: Viewer, id: Id) -> Result<UploadPlan> {
        let me = actor.at_least(Level::Contributor)?;
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.user != me.id {
            return Err(Error::Forbidden);
        }
        self.plan(&up)
    }

    /// Moves a not-yet-sent part to a fresh storage slot. Used when a send failed midway:
    /// GitHub keeps a half-written asset name reserved, so the retry needs a new name.
    pub async fn renew_part(&self, actor: Viewer<'_>, id: Id, index: usize) -> Result<PartPlan> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.user != me.id {
            return Err(Error::Forbidden);
        }
        let part = up.parts.get(index).ok_or_else(|| bad("分片序号不合法"))?.clone();
        if part.done {
            return Err(Error::Conflict("这一卷已经上传完成".into()));
        }
        let loc = self.storage.primary().reserve(&up.filename, part.size, &part.sha256).await?;
        let target = self.storage.primary().upload_target(&loc, part.size)?;
        self.mutate(|st, tx| {
            let mut up = st.uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
            up.parts[index].target = loc;
            tx.put_upload(&up)?;
            st.uploads.insert(id, up);
            Ok(())
        })?;
        Ok(PartPlan { index, size: part.size, done: false, target: Some(target) })
    }

    /// Verifies one part with its backend; the last confirmed part creates the blob.
    pub async fn confirm_part(&self, actor: Viewer<'_>, id: Id, index: usize, receipt: Receipt) -> Result<bool> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let up = self.st.read().uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
        if up.user != me.id {
            return Err(Error::Forbidden);
        }
        let part = up.parts.get(index).ok_or_else(|| bad("分片序号不合法"))?.clone();
        if part.done {
            return Ok(up.finished);
        }
        let loc = self.storage.for_location(&part.target)?.confirm(&part.target, part.size, &part.sha256, &receipt).await?;
        self.mutate(|st, tx| {
            let mut up = st.uploads.get(&id).cloned().ok_or(Error::NotFound("上传记录"))?;
            up.parts[index].target = loc;
            up.parts[index].done = true;
            if up.parts.iter().all(|p| p.done) && !up.finished {
                up.finished = true;
                if !st.blobs.contains_key(&up.key) {
                    let b = Blob {
                        key: up.key.clone(),
                        size: up.size,
                        parts: up
                            .parts
                            .iter()
                            .map(|p| Part { size: p.size, sha256: p.sha256.clone(), replicas: vec![p.target.clone()] })
                            .collect(),
                        created_at: now(),
                    };
                    tx.put_blob(&b)?;
                    st.blobs.insert(b.key.clone(), b);
                }
            }
            tx.put_upload(&up)?;
            let finished = up.finished;
            st.uploads.insert(id, up);
            Ok(finished)
        })
    }

    /// Drops stale uploads; returns orphaned storage locations for the caller to delete.
    pub fn collect_garbage(&self) -> Result<Vec<Location>> {
        let cutoff = now() - UPLOAD_TTL;
        self.mutate(|st, tx| {
            let stale: Vec<Upload> = st.uploads.values().filter(|u| u.created_at < cutoff).cloned().collect();
            let mut garbage = Vec::new();
            for u in stale {
                st.uploads.remove(&u.id);
                tx.del_upload(u.id)?;
                if u.finished {
                    // If nothing ended up using the blob, drop it.
                    garbage.extend(Self::release_blob(st, tx, &u.key)?);
                } else {
                    // Parts sent for an upload that never finished are not in any blob.
                    garbage.extend(u.parts.into_iter().filter(|p| p.done).map(|p| p.target));
                }
            }
            Ok(garbage)
        })
    }

}
