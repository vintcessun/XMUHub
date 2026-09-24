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
    /// Time to download the whole probe file.
    pub latency_ms: u64,
    /// Measured throughput, KiB/s.
    pub speed_kbps: u64,
    pub checked_at: i64,
    pub error: String,
    /// Sends `Access-Control-Allow-Origin`, so pages can fetch through it (previews).
    pub cors: bool,
}

pub struct Mirrors {
    candidates: Vec<String>,
    /// Healthy prefixes, fastest first.
    ranked: RwLock<Vec<String>>,
    stats: RwLock<Vec<MirrorStat>>,
    /// Healthy prefixes that allow cross-origin reads, fastest first.
    cors: RwLock<Vec<String>>,
}

impl Mirrors {
    pub fn new(candidates: Vec<String>) -> Mirrors {
        let candidates: Vec<String> = candidates.into_iter().map(|c| c.trim_end_matches('/').to_string()).collect();
        Mirrors { ranked: RwLock::new(candidates.clone()), candidates, stats: RwLock::new(Vec::new()), cors: RwLock::new(Vec::new()) }
    }

    /// `url` rewritten through each healthy mirror, then the original.
    pub fn wrap(&self, url: &str) -> Vec<String> {
        let mut out: Vec<String> = self.ranked.read().iter().map(|p| format!("{p}/{url}")).collect();
        out.push(url.to_string());
        out
    }

    /// Of `urls` (as produced by `wrap`), the ones a page can fetch cross-origin.
    pub fn cors_urls(&self, urls: &[String]) -> Vec<String> {
        let cors = self.cors.read();
        cors.iter().filter_map(|p| urls.iter().find(|u| u.starts_with(p.as_str()) && u[p.len()..].starts_with('/'))).cloned().collect()
    }

    pub fn stats(&self) -> Vec<MirrorStat> {
        self.stats.read().clone()
    }

    /// Downloads the probe file (`expected_len` bytes) through every candidate and ranks
    /// healthy mirrors by total transfer time — throughput, not just time to first byte.
    pub async fn probe(&self, client: &reqwest::Client, probe_url: &str, expected_len: usize) {
        let checks = self.candidates.iter().map(|prefix| async move {
            let start = Instant::now();
            let res = client
                .get(format!("{prefix}/{probe_url}"))
                .header(reqwest::header::ORIGIN, "https://xmuhub.invalid")
                .timeout(Duration::from_secs(20))
                .send()
                .await;
            let cors = res.as_ref().is_ok_and(|r| r.headers().contains_key(reqwest::header::ACCESS_CONTROL_ALLOW_ORIGIN));
            let (ok, error) = match res {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    // A mirror that answers with an HTML page instead of the file is broken.
                    Ok(b) if b.len() == expected_len => (true, String::new()),
                    Ok(b) => (false, format!("unexpected body ({} bytes)", b.len())),
                    Err(e) => (false, e.to_string()),
                },
                Ok(r) => (false, format!("HTTP {}", r.status())),
                Err(e) => (false, e.to_string()),
            };
            let ms = start.elapsed().as_millis().max(1) as u64;
            MirrorStat {
                prefix: prefix.clone(),
                ok,
                latency_ms: ms,
                speed_kbps: if ok { expected_len as u64 * 1000 / 1024 / ms } else { 0 },
                checked_at: crate::model::now(),
                error,
                cors: cors && ok,
            }
        });
        let mut stats = futures_util::future::join_all(checks).await;
        stats.sort_by_key(|s| (!s.ok, s.latency_ms));
        let ranked: Vec<String> = stats.iter().filter(|s| s.ok).map(|s| s.prefix.clone()).collect();
        tracing::info!(healthy = ranked.len(), total = stats.len(), "mirror probe finished");
        // If every probe failed the problem is more likely on our side (probe file, network)
        // than all mirrors dying at once; keep the previous ranking rather than going direct-only.
        if !ranked.is_empty() {
            *self.ranked.write() = ranked;
            *self.cors.write() = stats.iter().filter(|s| s.cors).map(|s| s.prefix.clone()).collect();
        }
        *self.stats.write() = stats;
    }
}
