//! GitHub Releases backend.
//!
//! Layout: public repos `{prefix}-001`, `{prefix}-002`, … owned by the storage account;
//! each release (`b0001`, `b0002`, …) is a bucket of at most `assets_per_release` assets.
//! Browsers upload through the Cloudflare Worker (which holds the token), so the XMUHub
//! server only makes small API calls: create buckets, confirm assets, delete assets.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::mirrors::Mirrors;
use super::{Receipt, StorageBackend, UploadTarget};
use crate::db::Db;
use crate::error::{Error, Result, bad};
use crate::model::{Location, now};
use crate::text::ascii_filename;
use crate::ticket::{self, Ticket};

const API: &str = "https://api.github.com";
const UPLOADS: &str = "https://uploads.github.com";
const CURSOR_KEY: &str = "github.cursor";
const PROBE_TAG: &str = "probe";
const PROBE_NAME: &str = "probe.txt";

pub struct GitHubConfig {
    pub owner: String,
    pub token: String,
    pub repo_prefix: String,
    pub assets_per_release: u32,
    pub releases_per_repo: u32,
    /// Base URL of the upload Worker, e.g. `https://xmuhub-upload.example.workers.dev`.
    pub worker_url: String,
    /// Shared with the Worker to sign upload tickets.
    pub ticket_secret: Vec<u8>,
    pub backup_repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cursor {
    repo_no: u32,
    release_no: u32,
    release_id: u64,
    count: u32,
}

#[derive(Deserialize)]
struct ReleaseJson {
    id: u64,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    id: u64,
    name: String,
    size: u64,
    state: String,
    #[serde(default)]
    digest: Option<String>,
    browser_download_url: String,
    #[serde(default)]
    created_at: String,
}

pub struct GitHubBackend {
    cfg: GitHubConfig,
    http: reqwest::Client,
    db: Arc<Db>,
    mirrors: Arc<Mirrors>,
    reserve_lock: tokio::sync::Mutex<()>,
}

impl GitHubBackend {
    pub fn new(cfg: GitHubConfig, http: reqwest::Client, db: Arc<Db>, mirrors: Arc<Mirrors>) -> GitHubBackend {
        GitHubBackend { cfg, http, db, mirrors, reserve_lock: tokio::sync::Mutex::new(()) }
    }

    fn repo_name(&self, no: u32) -> String {
        format!("{}-{no:03}", self.cfg.repo_prefix)
    }

    fn tag(no: u32) -> String {
        format!("b{no:04}")
    }

    fn req(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .bearer_auth(&self.cfg.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    async fn call<T: for<'de> Deserialize<'de>>(&self, rb: reqwest::RequestBuilder) -> Result<(reqwest::StatusCode, Option<T>)> {
        let res = rb.send().await?;
        let status = res.status();
        if status.is_success() {
            let body = res.text().await?;
            if body.is_empty() {
                return Ok((status, None));
            }
            let v = serde_json::from_str(&body).map_err(|e| Error::Upstream(format!("github json: {e}")))?;
            Ok((status, Some(v)))
        } else {
            Ok((status, None))
        }
    }

    async fn ensure_repo(&self, name: &str, private: bool) -> Result<()> {
        let url = format!("{API}/repos/{}/{name}", self.cfg.owner);
        let (st, _) = self.call::<serde_json::Value>(self.req(reqwest::Method::GET, &url)).await?;
        if st.is_success() {
            return Ok(());
        }
        let body = json!({
            "name": name,
            "description": "XMUHub storage — managed automatically, do not edit by hand",
            "private": private,
            "auto_init": true,
            "has_issues": false,
            "has_projects": false,
            "has_wiki": false,
        });
        let (st, _) = self
            .call::<serde_json::Value>(self.req(reqwest::Method::POST, &format!("{API}/user/repos")).json(&body))
            .await?;
        if !st.is_success() {
            return Err(Error::Upstream(format!("create repo {name}: HTTP {st}")));
        }
        tracing::info!(repo = name, "created storage repo");
        // A fresh auto_init repo needs a moment before releases can target its branch.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(())
    }

    /// Returns the release for `tag`, creating it if needed.
    async fn ensure_release(&self, repo: &str, tag: &str) -> Result<ReleaseJson> {
        let base = format!("{API}/repos/{}/{repo}/releases", self.cfg.owner);
        let (st, rel) = self.call::<ReleaseJson>(self.req(reqwest::Method::GET, &format!("{base}/tags/{tag}"))).await?;
        if st.is_success() {
            return rel.ok_or_else(|| Error::Upstream("empty release".into()));
        }
        let body = json!({ "tag_name": tag, "name": tag, "body": "XMUHub storage bucket", "make_latest": "false" });
        let (st, rel) = self.call::<ReleaseJson>(self.req(reqwest::Method::POST, &base).json(&body)).await?;
        match rel {
            Some(r) if st.is_success() => Ok(r),
            _ => Err(Error::Upstream(format!("create release {repo}/{tag}: HTTP {st}"))),
        }
    }

    fn load_cursor(&self) -> Result<Option<Cursor>> {
        use redb::ReadableDatabase;
        let txn = self.db.inner.begin_read()?;
        let t = txn.open_table(crate::db::META)?;
        Ok(match t.get(CURSOR_KEY)? {
            Some(v) => Some(serde_json::from_slice(v.value()).map_err(|e| Error::Internal(e.to_string()))?),
            None => None,
        })
    }

    fn save_cursor(&self, c: &Cursor) -> Result<()> {
        let bytes = serde_json::to_vec(c).map_err(|e| Error::Internal(e.to_string()))?;
        self.db.write(|tx| tx.put_meta(CURSOR_KEY, &bytes))
    }

    /// Uploads a small file directly from the server (probe file, metadata backups).
    async fn upload_small(&self, repo: &str, release_id: u64, name: &str, bytes: Vec<u8>, ctype: &str) -> Result<AssetJson> {
        let url = format!("{UPLOADS}/repos/{}/{repo}/releases/{release_id}/assets?name={name}", self.cfg.owner);
        let (st, a) = self
            .call::<AssetJson>(self.req(reqwest::Method::POST, &url).header("Content-Type", ctype).body(bytes))
            .await?;
        a.filter(|_| st.is_success()).ok_or_else(|| Error::Upstream(format!("upload {name}: HTTP {st}")))
    }

    /// Makes sure the tiny probe asset used for mirror health checks exists; returns its URL.
    pub async fn ensure_probe(&self) -> Result<String> {
        let repo = self.repo_name(1);
        self.ensure_repo(&repo, false).await?;
        let rel = self.ensure_release(&repo, PROBE_TAG).await?;
        if !rel.assets.iter().any(|a| a.name == PROBE_NAME && a.state == "uploaded") {
            self.upload_small(&repo, rel.id, PROBE_NAME, b"xmuhub-probe\n".to_vec(), "text/plain").await?;
        }
        Ok(format!("https://github.com/{}/{repo}/releases/download/{PROBE_TAG}/{PROBE_NAME}", self.cfg.owner))
    }

    /// Stores a metadata dump in the private backup repo and keeps the newest `keep`.
    pub async fn backup(&self, name: &str, bytes: Vec<u8>, keep: usize) -> Result<()> {
        let repo = self.cfg.backup_repo.clone();
        self.ensure_repo(&repo, true).await?;
        let rel = self.ensure_release(&repo, "backups").await?;
        if let Some(old) = rel.assets.iter().find(|a| a.name == name) {
            self.delete_asset(&repo, old.id).await?;
        }
        self.upload_small(&repo, rel.id, name, bytes, "application/gzip").await?;
        let mut assets = rel.assets;
        assets.retain(|a| a.name != name);
        assets.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        for a in assets.iter().skip(keep.saturating_sub(1)) {
            self.delete_asset(&repo, a.id).await?;
        }
        Ok(())
    }

    async fn delete_asset(&self, repo: &str, id: u64) -> Result<()> {
        let url = format!("{API}/repos/{}/{repo}/releases/assets/{id}", self.cfg.owner);
        let (st, _) = self.call::<serde_json::Value>(self.req(reqwest::Method::DELETE, &url)).await?;
        if st.is_success() || st == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(Error::Upstream(format!("delete asset {id}: HTTP {st}")))
        }
    }

    fn github_url(&self, repo: &str, tag: &str, name: &str) -> String {
        format!("https://github.com/{}/{repo}/releases/download/{tag}/{name}", self.cfg.owner)
    }
}

#[async_trait]
impl StorageBackend for GitHubBackend {
    fn name(&self) -> &'static str {
        "github"
    }

    async fn reserve(&self, display_name: &str, _size: u64, sha256: &str) -> Result<Location> {
        let _g = self.reserve_lock.lock().await;
        let mut cur = match self.load_cursor()? {
            Some(c) => c,
            None => {
                let repo = self.repo_name(1);
                self.ensure_repo(&repo, false).await?;
                let rel = self.ensure_release(&repo, &Self::tag(1)).await?;
                Cursor { repo_no: 1, release_no: 1, release_id: rel.id, count: rel.assets.len() as u32 }
            }
        };
        if cur.count >= self.cfg.assets_per_release {
            let (mut repo_no, mut release_no) = (cur.repo_no, cur.release_no + 1);
            if release_no > self.cfg.releases_per_repo {
                repo_no += 1;
                release_no = 1;
            }
            let repo = self.repo_name(repo_no);
            self.ensure_repo(&repo, false).await?;
            let rel = self.ensure_release(&repo, &Self::tag(release_no)).await?;
            cur = Cursor { repo_no, release_no, release_id: rel.id, count: rel.assets.len() as u32 };
        }
        cur.count += 1;
        self.save_cursor(&cur)?;

        let nonce = hex::encode(rand::random::<[u8; 3]>());
        let short = sha256.get(..12).ok_or_else(|| bad("bad sha256"))?;
        Ok(Location::GitHub {
            owner: self.cfg.owner.clone(),
            repo: self.repo_name(cur.repo_no),
            release_id: cur.release_id,
            tag: Self::tag(cur.release_no),
            asset_id: 0,
            name: format!("{short}-{nonce}-{}", ascii_filename(display_name)),
        })
    }

    fn upload_target(&self, reserved: &Location, size: u64) -> Result<UploadTarget> {
        let Location::GitHub { owner, repo, release_id, name, .. } = reserved else {
            return Err(bad("not a github location"));
        };
        let dest = format!("{UPLOADS}/repos/{owner}/{repo}/releases/{release_id}/assets?name={name}");
        let t = ticket::sign(&self.cfg.ticket_secret, &Ticket { u: dest, s: size, e: now() + 6 * 3600 });
        Ok(UploadTarget {
            url: format!("{}/upload?t={t}", self.cfg.worker_url.trim_end_matches('/')),
            method: "POST",
            headers: vec![],
        })
    }

    async fn confirm(&self, reserved: &Location, size: u64, sha256: &str, receipt: &Receipt) -> Result<Location> {
        let Location::GitHub { owner, repo, release_id, tag, name, .. } = reserved else {
            return Err(bad("not a github location"));
        };
        let id = receipt.asset_id.ok_or_else(|| bad("missing asset_id"))?;
        let url = format!("{API}/repos/{owner}/{repo}/releases/assets/{id}");
        let (st, a) = self.call::<AssetJson>(self.req(reqwest::Method::GET, &url)).await?;
        let a = a.filter(|_| st.is_success()).ok_or_else(|| Error::Conflict(format!("GitHub 上找不到该文件（HTTP {st}）")))?;
        let belongs = a.browser_download_url.ends_with(&format!("/releases/download/{tag}/{name}"));
        if !belongs || a.name != *name {
            return Err(Error::Conflict("上传回执与预留位置不符".into()));
        }
        if a.state != "uploaded" || a.size != size {
            return Err(Error::Conflict("文件没有完整上传，请重试".into()));
        }
        match a.digest.as_deref() {
            Some(d) if d != format!("sha256:{sha256}") => {
                let _ = self.delete_asset(repo, id).await;
                return Err(Error::Conflict("文件校验失败（sha256 不一致），请重新上传".into()));
            }
            None => tracing::warn!(asset = id, "github returned no digest; accepting size match"),
            _ => {}
        }
        Ok(Location::GitHub {
            owner: owner.clone(),
            repo: repo.clone(),
            release_id: *release_id,
            tag: tag.clone(),
            asset_id: id,
            name: name.clone(),
        })
    }

    fn download_urls(&self, loc: &Location) -> Vec<String> {
        match loc {
            Location::GitHub { repo, tag, name, .. } => self.mirrors.wrap(&self.github_url(repo, tag, name)),
            _ => vec![],
        }
    }

    async fn delete(&self, loc: &Location) -> Result<()> {
        match loc {
            Location::GitHub { repo, asset_id, .. } if *asset_id != 0 => self.delete_asset(repo, *asset_id).await,
            _ => Ok(()),
        }
    }
}
