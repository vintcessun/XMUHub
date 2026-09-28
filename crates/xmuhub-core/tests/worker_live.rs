//! Live check of the Cloudflare upload Worker, end to end with the real storage code:
//! reserve a slot (in a throwaway `XMUHub-uptest-*` repo, never the real store), send the
//! bytes through the Worker the way the upload page does, then verify them the way the
//! server does before a file is accepted.
//!
//! Ignored by default (it needs the storage account and the network). Run with:
//!   GH_STORE_USER=… GH_STORE_TOKEN=… UPLOAD_TICKET_SECRET=… UPLOAD_WORKER_URL=https://upload.vintces.icu \
//!   cargo test -p xmuhub-core --test worker_live -- --ignored --nocapture

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::db::Db;
use xmuhub_core::storage::github::{GitHubBackend, GitHubConfig};
use xmuhub_core::storage::mirrors::Mirrors;
use xmuhub_core::storage::{Receipt, StorageBackend};

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} is required for this test"))
}

#[tokio::test]
#[ignore]
async fn upload_through_worker() {
    let worker = env("UPLOAD_WORKER_URL");
    let dir = std::env::temp_dir().join(format!("xmuhub-worker-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    // GitHub rejects API calls without a User-Agent (the server sets one the same way).
    let http = reqwest::Client::builder().user_agent("XMUHub-worker-live-test").build().unwrap();
    let gh = GitHubBackend::new(
        GitHubConfig {
            owner: env("GH_STORE_USER"),
            token: env("GH_STORE_TOKEN"),
            repo_prefix: "XMUHub-uptest".into(),
            assets_per_release: 900,
            releases_per_repo: 50,
            worker_url: worker.clone(),
            ticket_secret: env("UPLOAD_TICKET_SECRET").into_bytes(),
            backup_repo: String::new(),
        },
        http.clone(),
        db,
        Arc::new(Mirrors::new(Vec::new())),
    );

    // ~6 MB of non-repeating bytes, so nothing along the way can shortcut the transfer.
    let mut content = Vec::with_capacity(6 << 20);
    let mut x: u64 = std::process::id() as u64 ^ 0x9E37_79B9_7F4A_7C15;
    while content.len() < 6 << 20 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        content.extend_from_slice(&x.to_le_bytes());
    }
    let size = content.len() as u64;
    let sha = hex::encode(Sha256::digest(&content));

    let slot = gh.reserve("worker-live-test.bin", size, &sha).await.unwrap();
    let target = gh.upload_target(&slot, size).unwrap();
    assert!(target.url.starts_with(&worker), "upload goes to the Worker: {}", target.url);

    // As the upload page's XHR sends it (cross-origin, so no X-XMUHub header).
    let started = std::time::Instant::now();
    let mut req = http
        .post(&target.url)
        .header("Origin", "https://xmu.vintces.icu")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36")
        .header("Content-Type", "application/octet-stream");
    for (k, v) in &target.headers {
        req = req.header(k, v);
    }
    let resp = req.body(content).send().await.unwrap();
    let status = resp.status();
    let cors = resp.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body: serde_json::Value = resp.json().await.unwrap();
    let secs = started.elapsed().as_secs_f64();
    println!("worker: HTTP {status} in {secs:.1}s ({:.2} MB/s), CORS {cors:?}", size as f64 / secs / 1048576.0);
    assert_eq!(status.as_u16(), 201, "{body}");
    assert_eq!(cors.as_deref(), Some("https://xmu.vintces.icu"));

    // The server's acceptance check: the asset exists where reserved, complete, same sha256.
    let id = body["id"].as_u64().expect("asset id");
    let stored = gh.confirm(&slot, size, &sha, &Receipt { asset_id: Some(id) }).await.unwrap();
    println!("verified by the server-side check: {stored:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
