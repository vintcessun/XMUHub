//! ghproxy-style download mirrors (`{prefix}/https://github.com/...`).
//! Public mirrors come and go, so they are probed periodically and ranked by latency;
//! direct GitHub is always kept as the last resort.

use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MirrorStat {
    pub prefix: String,
    pub ok: bool,
    pub latency_ms: u64,
    pub checked_at: i64,
    pub error: String,
}

pub struct Mirrors {
    candidates: Vec<String>,
    /// Healthy prefixes, fastest first.
    ranked: RwLock<Vec<String>>,
    stats: RwLock<Vec<MirrorStat>>,
}

impl Mirrors {
    pub fn new(candidates: Vec<String>) -> Mirrors {
        let candidates: Vec<String> = candidates.into_iter().map(|c| c.trim_end_matches('/').to_string()).collect();
        Mirrors { ranked: RwLock::new(candidates.clone()), candidates, stats: RwLock::new(Vec::new()) }
    }

    /// `url` rewritten through each healthy mirror, then the original.
    pub fn wrap(&self, url: &str) -> Vec<String> {
        let mut out: Vec<String> = self.ranked.read().iter().map(|p| format!("{p}/{url}")).collect();
        out.push(url.to_string());
        out
    }

    pub fn stats(&self) -> Vec<MirrorStat> {
        self.stats.read().clone()
    }

    /// Fetches the first byte of `probe_url` through every candidate and re-ranks.
    pub async fn probe(&self, client: &reqwest::Client, probe_url: &str) {
        let checks = self.candidates.iter().map(|prefix| async move {
            let start = Instant::now();
            let res = client
                .get(format!("{prefix}/{probe_url}"))
                .header("Range", "bytes=0-0")
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            let (ok, error) = match res {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    // A mirror that answers with an HTML page instead of the file is broken.
                    Ok(b) if b.len() <= 16 => (true, String::new()),
                    Ok(b) => (false, format!("unexpected body ({} bytes)", b.len())),
                    Err(e) => (false, e.to_string()),
                },
                Ok(r) => (false, format!("HTTP {}", r.status())),
                Err(e) => (false, e.to_string()),
            };
            MirrorStat {
                prefix: prefix.clone(),
                ok,
                latency_ms: start.elapsed().as_millis() as u64,
                checked_at: crate::model::now(),
                error,
            }
        });
        let mut stats = futures_util::future::join_all(checks).await;
        stats.sort_by_key(|s| (!s.ok, s.latency_ms));
        let ranked: Vec<String> = stats.iter().filter(|s| s.ok).map(|s| s.prefix.clone()).collect();
        tracing::info!(healthy = ranked.len(), total = stats.len(), "mirror probe finished");
        *self.ranked.write() = ranked;
        *self.stats.write() = stats;
    }
}
