//! Read-only backend for files referenced inside other public GitHub repositories
//! (`Location::GitHubRepo`). Nothing is uploaded; downloads go through the mirrors to
//! `github.com/{owner}/{repo}/raw/{commit}/{path}`, pinned to a commit so content can't change.

use std::sync::Arc;

use async_trait::async_trait;

use super::mirrors::Mirrors;
use super::{Receipt, StorageBackend, UploadTarget};
use crate::error::{Result, bad};
use crate::model::Location;

pub struct RepoRefBackend {
    pub mirrors: Arc<Mirrors>,
}

/// Percent-encodes a repository path, keeping `/` separators.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() * 3);
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn raw_url(owner: &str, repo: &str, commit: &str, path: &str) -> String {
    format!("https://github.com/{owner}/{repo}/raw/{commit}/{}", encode_path(path))
}

#[async_trait]
impl StorageBackend for RepoRefBackend {
    fn name(&self) -> &'static str {
        "github-repo"
    }

    async fn reserve(&self, _display: &str, _size: u64, _sha256: &str) -> Result<Location> {
        Err(bad("引用存储不支持上传"))
    }

    fn upload_target(&self, _reserved: &Location, _size: u64) -> Result<UploadTarget> {
        Err(bad("引用存储不支持上传"))
    }

    async fn confirm(&self, _reserved: &Location, _size: u64, _sha256: &str, _receipt: &Receipt) -> Result<Location> {
        Err(bad("引用存储不支持上传"))
    }

    fn download_urls(&self, loc: &Location) -> Vec<String> {
        match loc {
            Location::GitHubRepo { owner, repo, commit, path } => self.mirrors.wrap(&raw_url(owner, repo, commit, path)),
            _ => vec![],
        }
    }

    async fn delete(&self, _loc: &Location) -> Result<()> {
        // Someone else's repository: never touched.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_chinese_paths() {
        assert_eq!(super::encode_path("大一上/微积分 I.pdf"), "%E5%A4%A7%E4%B8%80%E4%B8%8A/%E5%BE%AE%E7%A7%AF%E5%88%86%20I.pdf");
    }
}
