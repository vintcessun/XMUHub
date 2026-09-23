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
/// Mirrors are ranked by how fast they deliver this file, so it must be big enough to
/// measure throughput yet cheap to fetch every few minutes through each mirror.
const PROBE_NAME: &str = "probe-256k.bin";
pub const PROBE_SIZE: usize = 256 * 1024;

/// One file of a scanned repository (git blob sha1, size in bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoFile {
    pub path: String,
    pub size: u64,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoScan {
    pub owner: String,
    pub repo: String,
    pub branch: String,
    /// Commit every imported file is pinned to.
    pub commit: String,
    /// SPDX id, empty when unknown.
    pub license: String,
    pub files: Vec<RepoFile>,
}

/// `https://github.com/owner/repo[/tree/branch/...]` or `owner/repo` → (owner, repo, branch).
pub fn parse_repo_url(s: &str) -> Result<(String, String, Option<String>)> {
    let s = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let rest = rest.strip_prefix("github.com/").or_else(|| rest.strip_prefix("www.github.com/")).unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    match parts.as_slice() {
        [o, r] if ok(o) && ok(r) => Ok((o.to_string(), r.to_string(), None)),
        [o, r, "tree", b, ..] if ok(o) && ok(r) => Ok((o.to_string(), r.to_string(), Some(b.to_string()))),
        _ => Err(bad("请输入 GitHub 仓库地址，例如 https://github.com/owner/repo")),
    }
}

pub struct GitHubConfig {
    pub owner: String,
    pub token: String,
    pub repo_prefix: String,
    pub assets_per_release: u32,
    pub releases_per_repo: u32,
    /// Where browsers send part bytes: the Cloudflare Worker's base URL, or empty to use
    /// the XMUHub server's own streaming relay (`/api/relay/upload`).
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

    pub async fn ensure_repo(&self, name: &str, private: bool) -> Result<()> {
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
        let good = rel.assets.iter().find(|a| a.name == PROBE_NAME && a.state == "uploaded" && a.size == PROBE_SIZE as u64);
        if good.is_none() {
            // Replace a missing, half-written or wrong-sized probe.
            if let Some(bad) = rel.assets.iter().find(|a| a.name == PROBE_NAME) {
                self.delete_asset(&repo, bad.id).await?;
            }
            // Deterministic, incompressible-looking bytes so mirrors can't shortcut the transfer.
            let mut data = Vec::with_capacity(PROBE_SIZE);
            let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
            while data.len() < PROBE_SIZE {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                data.extend_from_slice(&x.to_le_bytes());
            }
            data.truncate(PROBE_SIZE);
            self.upload_small(&repo, rel.id, PROBE_NAME, data, "application/octet-stream").await?;
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

    pub fn owner(&self) -> &str {
        &self.cfg.owner
    }

    pub fn token(&self) -> &str {
        &self.cfg.token
    }

    // ------------------------------------------------------------ generic API helpers

    /// GET an API path (relative to api.github.com) as JSON; `None` on 404.
    pub async fn api_get(&self, path: &str) -> Result<Option<serde_json::Value>> {
        let (st, v) = self.call::<serde_json::Value>(self.req(reqwest::Method::GET, &format!("{API}{path}"))).await?;
        if st == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !st.is_success() {
            return Err(Error::Upstream(format!("GET {path}: HTTP {st}")));
        }
        Ok(v)
    }

    /// Reads a file from one of the storage account's repos (contents API); `None` if absent.
    pub async fn read_file(&self, repo: &str, path: &str) -> Result<Option<(Vec<u8>, String)>> {
        use base64::Engine;
        let Some(v) = self.api_get(&format!("/repos/{}/{repo}/contents/{}", self.cfg.owner, super::repo_ref::encode_path(path))).await? else {
            return Ok(None);
        };
        let b64: String = v["content"].as_str().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = if b64.is_empty() {
            // Files over 1 MB come without inline content; fetch the blob instead.
            let sha = v["sha"].as_str().unwrap_or("");
            let blob = self.api_get(&format!("/repos/{}/{repo}/git/blobs/{sha}", self.cfg.owner)).await?.unwrap_or_default();
            let b: String = blob["content"].as_str().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| Error::Upstream(e.to_string()))?
        } else {
            base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| Error::Upstream(e.to_string()))?
        };
        Ok(Some((bytes, v["sha"].as_str().unwrap_or("").to_string())))
    }

    /// Creates or replaces a file in one of the storage account's repos (one commit).
    pub async fn write_file(&self, repo: &str, path: &str, bytes: &[u8], message: &str) -> Result<()> {
        use base64::Engine;
        let existing = self.read_file(repo, path).await?;
        if existing.as_ref().is_some_and(|(b, _)| b == bytes) {
            return Ok(());
        }
        let mut body = json!({ "message": message, "content": base64::engine::general_purpose::STANDARD.encode(bytes) });
        if let Some((_, sha)) = existing {
            body["sha"] = json!(sha);
        }
        let url = format!("{API}/repos/{}/{repo}/contents/{}", self.cfg.owner, super::repo_ref::encode_path(path));
        let (st, _) = self.call::<serde_json::Value>(self.req(reqwest::Method::PUT, &url).json(&body)).await?;
        if !st.is_success() {
            return Err(Error::Upstream(format!("write {repo}/{path}: HTTP {st}")));
        }
        Ok(())
    }

    /// Triggers a `workflow_dispatch` run.
    pub async fn dispatch(&self, repo: &str, workflow: &str, inputs: serde_json::Value) -> Result<()> {
        let url = format!("{API}/repos/{}/{repo}/actions/workflows/{workflow}/dispatches", self.cfg.owner);
        let (st, _) = self
            .call::<serde_json::Value>(self.req(reqwest::Method::POST, &url).json(&json!({ "ref": "main", "inputs": inputs })))
            .await?;
        if !st.is_success() {
            return Err(Error::Upstream(format!("dispatch {repo}/{workflow}: HTTP {st}")));
        }
        Ok(())
    }

    /// Assets of a release in one of our repos, by tag: (release id, [(asset id, name, size, digest)]).
    pub async fn release_assets(&self, repo: &str, tag: &str) -> Result<Option<(u64, Vec<(u64, String, u64, String)>)>> {
        let Some(rel) = self.api_get(&format!("/repos/{}/{repo}/releases/tags/{tag}", self.cfg.owner)).await? else {
            return Ok(None);
        };
        let rid = rel["id"].as_u64().unwrap_or(0);
        let mut out = Vec::new();
        for page in 1..=20 {
            let v = self
                .api_get(&format!("/repos/{}/{repo}/releases/{rid}/assets?per_page=100&page={page}", self.cfg.owner))
                .await?
                .unwrap_or_default();
            let arr = v.as_array().cloned().unwrap_or_default();
            if arr.is_empty() {
                break;
            }
            for a in arr {
                if a["state"] == "uploaded" {
                    out.push((
                        a["id"].as_u64().unwrap_or(0),
                        a["name"].as_str().unwrap_or("").to_string(),
                        a["size"].as_u64().unwrap_or(0),
                        a["digest"].as_str().unwrap_or("").to_string(),
                    ));
                }
            }
        }
        Ok(Some((rid, out)))
    }

    pub fn owner_login(&self) -> &str {
        &self.cfg.owner
    }

    // ------------------------------------------------------------ repository import

    /// Lists every file of a public repository at the head of `branch` (or the default branch).
    /// Only API calls — no file contents are downloaded.
    pub async fn scan_repo(&self, owner: &str, repo: &str, branch: Option<&str>) -> Result<RepoScan> {
        let info = self.api_get(&format!("/repos/{owner}/{repo}")).await?.ok_or(Error::NotFound("GitHub 仓库"))?;
        if info["private"].as_bool().unwrap_or(false) {
            return Err(bad("只能导入公开仓库"));
        }
        let branch = branch.map(str::to_string).unwrap_or_else(|| info["default_branch"].as_str().unwrap_or("main").to_string());
        let commit = self
            .api_get(&format!("/repos/{owner}/{repo}/commits/{}", super::repo_ref::encode_path(&branch)))
            .await?
            .ok_or(Error::NotFound("分支"))?;
        let sha = commit["sha"].as_str().unwrap_or("").to_string();
        let tree = commit["commit"]["tree"]["sha"].as_str().unwrap_or("").to_string();
        let mut files = Vec::new();
        self.walk_tree(owner, repo, &tree, "", &mut files, 0).await?;
        Ok(RepoScan {
            owner: info["owner"]["login"].as_str().unwrap_or(owner).to_string(),
            repo: info["name"].as_str().unwrap_or(repo).to_string(),
            branch,
            commit: sha,
            license: info["license"]["spdx_id"].as_str().filter(|l| *l != "NOASSERTION").unwrap_or("").to_string(),
            files,
        })
    }

    fn walk_tree<'a>(
        &'a self,
        owner: &'a str,
        repo: &'a str,
        sha: &'a str,
        prefix: &'a str,
        out: &'a mut Vec<RepoFile>,
        depth: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if depth > 12 {
                return Ok(());
            }
            let t = self.api_get(&format!("/repos/{owner}/{repo}/git/trees/{sha}?recursive=1")).await?.unwrap_or_default();
            if !t["truncated"].as_bool().unwrap_or(false) {
                for e in t["tree"].as_array().into_iter().flatten().filter(|e| e["type"] == "blob") {
                    out.push(RepoFile {
                        path: format!("{prefix}{}", e["path"].as_str().unwrap_or("")),
                        size: e["size"].as_u64().unwrap_or(0),
                        sha: e["sha"].as_str().unwrap_or("").to_string(),
                    });
                }
                return Ok(());
            }
            // Too big for one recursive listing: descend one level at a time.
            let t = self.api_get(&format!("/repos/{owner}/{repo}/git/trees/{sha}")).await?.unwrap_or_default();
            for e in t["tree"].as_array().cloned().unwrap_or_default() {
                let path = format!("{prefix}{}", e["path"].as_str().unwrap_or(""));
                if e["type"] == "tree" {
                    self.walk_tree(owner, repo, e["sha"].as_str().unwrap_or(""), &format!("{path}/"), out, depth + 1).await?;
                } else if e["type"] == "blob" {
                    out.push(RepoFile { path, size: e["size"].as_u64().unwrap_or(0), sha: e["sha"].as_str().unwrap_or("").to_string() });
                }
            }
            Ok(())
        })
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

        // Readable name first (it becomes the saved filename when a mirror serves the file),
        // then a hash + nonce suffix so names never collide within a bucket.
        let short = sha256.get(..8).ok_or_else(|| bad("bad sha256"))?;
        let suffix = format!("{short}{}", hex::encode(rand::random::<[u8; 2]>()));
        let ascii = ascii_filename(display_name);
        let name = match ascii.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}-{suffix}.{ext}"),
            None => format!("{ascii}-{suffix}"),
        };
        Ok(Location::GitHub {
            owner: self.cfg.owner.clone(),
            repo: self.repo_name(cur.repo_no),
            release_id: cur.release_id,
            tag: Self::tag(cur.release_no),
            asset_id: 0,
            name,
        })
    }

    fn upload_target(&self, reserved: &Location, size: u64) -> Result<UploadTarget> {
        let Location::GitHub { owner, repo, release_id, name, .. } = reserved else {
            return Err(bad("not a github location"));
        };
        let dest = format!("{UPLOADS}/repos/{owner}/{repo}/releases/{release_id}/assets?name={name}");
        let t = ticket::sign(&self.cfg.ticket_secret, &Ticket { u: dest, s: size, e: now() + 6 * 3600 });
        let url = if self.cfg.worker_url.is_empty() {
            format!("/api/relay/upload?t={t}")
        } else {
            format!("{}/upload?t={t}", self.cfg.worker_url.trim_end_matches('/'))
        };
        Ok(UploadTarget { url, method: "POST", headers: vec![] })
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
