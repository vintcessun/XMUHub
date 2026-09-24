//! First-page thumbnails for resource lists. They are made by a GitHub Actions job (see the
//! server's `thumbs` module) — the files never pass through our server — and stored as
//! release assets; here we only track which blobs have one.

use serde::Serialize;

use super::Hub;
use crate::error::Result;
use crate::model::*;

/// Retry a failed thumbnail this many times (network hiccups), then give up.
const MAX_TRIES: u8 = 3;
/// Marks files that can't have a thumbnail (unsupported format), so they aren't retried.
pub const NEVER: u8 = 200;
/// Largest file the job downloads to make a thumbnail.
const MAX_BYTES: u64 = 120 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ThumbJob {
    pub key: String,
    /// Lower-case extension of (one of) the resources using this blob.
    pub ext: String,
    /// Direct GitHub URL of each part, in order.
    pub urls: Vec<String>,
    pub size: u64,
}

impl Hub {
    /// Blobs of live resources still without a thumbnail, newest first.
    pub fn thumb_backlog(&self, limit: usize, max_bytes: u64) -> Vec<ThumbJob> {
        let st = self.st.read();
        let mut rs: Vec<&Resource> = st.resources.values().filter(|r| r.status != Status::Rejected).collect();
        rs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let mut seen = std::collections::HashSet::new();
        let (mut out, mut bytes) = (Vec::new(), 0u64);
        for r in rs {
            if out.len() >= limit || !seen.insert(r.blob.as_str()) {
                continue;
            }
            if st.thumbs.get(&r.blob).is_some_and(|t| t.loc.is_some() || t.tries >= MAX_TRIES) {
                continue;
            }
            let Some(b) = st.blobs.get(&r.blob) else { continue };
            if b.size > MAX_BYTES || bytes + b.size > max_bytes {
                continue;
            }
            let urls: Option<Vec<String>> = b
                .parts
                .iter()
                .map(|p| self.storage.download_urls(&p.replicas).into_iter().find(|u| u.starts_with("https://github.com/")))
                .collect();
            let Some(urls) = urls else { continue };
            bytes += b.size;
            out.push(ThumbJob { key: b.key.clone(), ext: r.ext.clone(), urls, size: b.size });
        }
        out
    }

    /// Records a finished thumbnail (`Some`) or a failed attempt (`None`; `permanent` for
    /// formats that can never have one).
    pub fn set_thumb(&self, key: &str, loc: Option<Location>, permanent: bool) -> Result<()> {
        self.mutate(|st, tx| {
            let prev = st.thumbs.get(key).map(|t| t.tries).unwrap_or(0);
            let t = Thumb {
                key: key.to_string(),
                tries: if loc.is_some() { prev } else if permanent { NEVER } else { prev.saturating_add(1) },
                loc,
                at: now(),
            };
            tx.put_thumb(&t)?;
            st.thumbs.insert(t.key.clone(), t);
            Ok(())
        })
    }

    /// Where a blob's thumbnail is stored, if it has one.
    pub fn thumb_of(&self, key: &str) -> Option<Location> {
        self.st.read().thumbs.get(key).and_then(|t| t.loc.clone())
    }

    /// (with thumbnail, without one yet, can't have one) — for the admin status page.
    pub fn thumb_counts(&self) -> (usize, usize, usize) {
        let st = self.st.read();
        let keys: std::collections::HashSet<&str> = st.resources.values().filter(|r| r.status != Status::Rejected).map(|r| r.blob.as_str()).collect();
        let (mut ok, mut todo, mut never) = (0, 0, 0);
        for k in keys {
            match st.thumbs.get(k) {
                Some(t) if t.loc.is_some() => ok += 1,
                Some(t) if t.tries >= MAX_TRIES => never += 1,
                _ => todo += 1,
            }
        }
        (ok, todo, never)
    }
}
