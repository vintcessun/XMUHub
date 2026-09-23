//! Configuration from environment variables (systemd `EnvironmentFile=` in production).

use std::net::SocketAddr;
use std::path::PathBuf;

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn var_or(k: &str, d: &str) -> String {
    var(k).unwrap_or_else(|| d.to_string())
}

pub const DEFAULT_MIRRORS: &[&str] = &[
    "https://gh-proxy.com",
    "https://ghfast.top",
    "https://ghproxy.net",
    "https://gh.llkk.cc",
    "https://ghproxy.cc",
    "https://gh.ddlc.top",
    "https://github.moeyy.xyz",
    "https://gh-proxy.org",
    "https://hk.gh-proxy.com",
    "https://cdn.gh-proxy.com",
];

pub enum StorageKind {
    Local,
    GitHub,
}

pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    pub storage: StorageKind,
    pub gh_owner: String,
    pub gh_token: String,
    pub gh_repo_prefix: String,
    pub gh_backup_repo: String,
    /// Optional proxy for server → GitHub API calls (small requests only).
    pub gh_api_proxy: Option<String>,
    pub worker_url: String,
    pub ticket_secret: Vec<u8>,
    pub mirrors: Vec<String>,
    pub db_cache_mb: usize,
    /// Cap on bytes the server relays to GitHub per day (only when no Worker is used).
    pub relay_daily_bytes: u64,
    pub relay_concurrency: usize,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        let storage = match var_or("XMUHUB_STORAGE", "local").as_str() {
            "github" => StorageKind::GitHub,
            "local" => StorageKind::Local,
            other => anyhow::bail!("XMUHUB_STORAGE must be `github` or `local`, got `{other}`"),
        };
        let secret = var("UPLOAD_TICKET_SECRET").unwrap_or_default();
        if matches!(storage, StorageKind::GitHub) {
            for k in ["GH_STORE_USER", "GH_STORE_TOKEN", "UPLOAD_TICKET_SECRET"] {
                if var(k).is_none() {
                    anyhow::bail!("{k} is required when XMUHUB_STORAGE=github");
                }
            }
        }
        Ok(Config {
            bind: var_or("XMUHUB_BIND", "127.0.0.1:8089").parse()?,
            data_dir: var_or("XMUHUB_DATA", "data").into(),
            web_dir: var_or("XMUHUB_WEB", "web").into(),
            storage,
            gh_owner: var_or("GH_STORE_USER", ""),
            gh_token: var_or("GH_STORE_TOKEN", ""),
            gh_repo_prefix: var_or("GH_REPO_PREFIX", "XMUHub-store"),
            gh_backup_repo: var_or("GH_BACKUP_REPO", "XMUHub-backup"),
            gh_api_proxy: var("GH_API_PROXY"),
            worker_url: var_or("UPLOAD_WORKER_URL", ""),
            // Local dev still signs its tickets; a random per-process secret is fine there.
            ticket_secret: if secret.is_empty() { rand_secret() } else { secret.into_bytes() },
            mirrors: var("MIRRORS")
                .map(|m| m.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_else(|| DEFAULT_MIRRORS.iter().map(|s| s.to_string()).collect()),
            db_cache_mb: var_or("XMUHUB_DB_CACHE_MB", "32").parse()?,
            relay_daily_bytes: var_or("RELAY_DAILY_MB", "5120").parse::<u64>()? * 1024 * 1024,
            relay_concurrency: var_or("RELAY_CONCURRENCY", "4").parse()?,
        })
    }
}

fn rand_secret() -> Vec<u8> {
    use std::hash::{BuildHasher, RandomState};
    (0..4).flat_map(|i| RandomState::new().hash_one(i).to_le_bytes()).collect()
}
