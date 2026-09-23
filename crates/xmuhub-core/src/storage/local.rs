//! Disk backend for development and tests. The server itself receives and serves the
//! bytes (`/api/local/...`), which is exactly what production avoids — never make it primary there.

use std::path::PathBuf;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{Receipt, StorageBackend, UploadTarget};
use crate::error::{Error, Result, bad};
use crate::model::{Location, now};
use crate::ticket::{self, Ticket};

pub struct LocalBackend {
    pub dir: PathBuf,
    pub secret: Vec<u8>,
}

impl LocalBackend {
    pub fn path_of(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') || name.starts_with('.') {
            return Err(bad("bad local file name"));
        }
        Ok(self.dir.join(name))
    }
}

#[async_trait]
impl StorageBackend for LocalBackend {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn reserve(&self, _display: &str, _size: u64, sha256: &str) -> Result<Location> {
        tokio::fs::create_dir_all(&self.dir).await?;
        Ok(Location::Local { path: sha256.to_string() })
    }

    fn upload_target(&self, reserved: &Location, size: u64) -> Result<UploadTarget> {
        let Location::Local { path } = reserved else { return Err(bad("not a local location")) };
        let t = ticket::sign(&self.secret, &Ticket { u: path.clone(), s: size, e: now() + 6 * 3600 });
        Ok(UploadTarget { url: format!("/api/local/upload?t={t}"), method: "PUT", headers: vec![] })
    }

    async fn confirm(&self, reserved: &Location, size: u64, sha256: &str, _r: &Receipt) -> Result<Location> {
        let Location::Local { path } = reserved else { return Err(bad("not a local location")) };
        let mut f = tokio::fs::File::open(self.path_of(path)?)
            .await
            .map_err(|_| Error::Conflict("文件还没有上传完成".into()))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut total = 0u64;
        loop {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            total += n as u64;
            h.update(&buf[..n]);
        }
        if total != size || hex::encode(h.finalize()) != sha256 {
            return Err(Error::Conflict("上传的文件内容与声明不符".into()));
        }
        Ok(reserved.clone())
    }

    fn download_urls(&self, loc: &Location) -> Vec<String> {
        match loc {
            Location::Local { path } => vec![format!("/api/local/file/{path}")],
            _ => vec![],
        }
    }

    async fn delete(&self, loc: &Location) -> Result<()> {
        if let Location::Local { path } = loc {
            let _ = tokio::fs::remove_file(self.path_of(path)?).await;
        }
        Ok(())
    }
}
