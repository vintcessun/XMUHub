//! Storage backends. A resource never knows where its bytes live: blobs carry a list of
//! `Location`s per part and each backend knows how to reserve, confirm, serve and delete them.
//!
//! Bytes never pass through the XMUHub server in production: the browser uploads straight
//! to the backend's `UploadTarget`, and downloads are redirects to backend/mirror URLs.

pub mod github;
pub mod local;
pub mod mirrors;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::Location;

/// How the browser should send one part.
#[derive(Debug, Clone, Serialize)]
pub struct UploadTarget {
    pub url: String,
    pub method: &'static str,
    pub headers: Vec<(String, String)>,
}

/// What the browser reports back after sending a part.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Receipt {
    /// GitHub asset id returned by the upload.
    pub asset_id: Option<u64>,
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    fn name(&self) -> &'static str;

    /// Reserves a slot for one part and returns its (not yet filled) location.
    async fn reserve(&self, display_name: &str, size: u64, sha256: &str) -> Result<Location>;

    /// Signed instructions for sending the bytes of a reserved part.
    fn upload_target(&self, reserved: &Location, size: u64) -> Result<UploadTarget>;

    /// Verifies the bytes arrived intact and returns the final location.
    async fn confirm(&self, reserved: &Location, size: u64, sha256: &str, receipt: &Receipt) -> Result<Location>;

    /// Download URLs for a stored part, best first.
    fn download_urls(&self, loc: &Location) -> Vec<String>;

    async fn delete(&self, loc: &Location) -> Result<()>;
}

/// All configured backends; uploads go to the primary one.
#[derive(Clone)]
pub struct Storage {
    backends: Vec<Arc<dyn StorageBackend>>,
}

impl Storage {
    pub fn new(backends: Vec<Arc<dyn StorageBackend>>) -> Storage {
        assert!(!backends.is_empty(), "at least one storage backend");
        Storage { backends }
    }

    pub fn primary(&self) -> &Arc<dyn StorageBackend> {
        &self.backends[0]
    }

    pub fn for_location(&self, loc: &Location) -> Result<&Arc<dyn StorageBackend>> {
        self.backends
            .iter()
            .find(|b| b.name() == loc.backend())
            .ok_or_else(|| Error::Internal(format!("backend `{}` not configured", loc.backend())))
    }

    /// URLs across every replica of a part, best first.
    pub fn download_urls(&self, replicas: &[Location]) -> Vec<String> {
        replicas
            .iter()
            .filter_map(|l| self.for_location(l).ok().map(|b| b.download_urls(l)))
            .flatten()
            .collect()
    }
}
