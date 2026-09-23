//! Static site. Files under `web/` are read once at startup, compressed (brotli + gzip)
//! and served from memory with ETags. Edit files and restart (deploy does) to update.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

pub struct Asset {
    mime: &'static str,
    raw: Bytes,
    br: Option<Bytes>,
    gz: Option<Bytes>,
    etag: String,
    /// Hashed-by-query assets and HTML differ in cacheability.
    immutable: bool,
}

pub struct Site {
    files: HashMap<String, Asset>,
}

fn compressible(mime: &str) -> bool {
    mime.starts_with("text/") || mime.contains("javascript") || mime.contains("json") || mime.contains("svg") || mime.contains("xml")
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

impl Site {
    pub fn load(dir: &Path) -> anyhow::Result<Site> {
        let mut files = HashMap::new();
        let mut total = 0usize;
        walk(dir, dir, &mut |rel, raw| {
            let mime = mime_of(&rel);
            let (br, gz) = if compressible(mime) && raw.len() > 512 {
                let mut br = Vec::new();
                {
                    let mut w = brotli::CompressorWriter::new(&mut br, 4096, 11, 22);
                    w.write_all(&raw)?;
                }
                let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                gz.write_all(&raw)?;
                (Some(Bytes::from(br)), Some(Bytes::from(gz.finish()?)))
            } else {
                (None, None)
            };
            total += raw.len();
            let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&raw))[..16]);
            let immutable = rel.starts_with("vendor/");
            files.insert(rel, Asset { mime, raw: Bytes::from(raw), br, gz, etag, immutable });
            Ok(())
        })?;
        tracing::info!(files = files.len(), bytes = total, "static site loaded");
        Ok(Site { files })
    }

    pub fn get(&self, path: &str) -> Option<&Asset> {
        self.files.get(path)
    }

    pub fn respond(self: &Arc<Self>, path: &str, req: &HeaderMap, status: StatusCode) -> Response {
        let Some(a) = self.files.get(path) else {
            return match self.files.get("404.html") {
                Some(_) if path != "404.html" => self.respond("404.html", req, StatusCode::NOT_FOUND),
                _ => (StatusCode::NOT_FOUND, "not found").into_response(),
            };
        };
        let cache = if a.immutable {
            "public, max-age=31536000, immutable"
        } else if a.mime.starts_with("text/html") {
            "no-cache"
        } else {
            "public, max-age=600, stale-while-revalidate=86400"
        };
        if status == StatusCode::OK && req.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(a.etag.as_str()) {
            return Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, &a.etag)
                .header(header::CACHE_CONTROL, cache)
                .body(Body::empty())
                .unwrap();
        }
        let accept = req.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).unwrap_or("");
        let (body, enc) = match (&a.br, &a.gz) {
            (Some(br), _) if accept.contains("br") => (br.clone(), Some("br")),
            (_, Some(gz)) if accept.contains("gzip") => (gz.clone(), Some("gzip")),
            _ => (a.raw.clone(), None),
        };
        let mut b = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, a.mime)
            .header(header::ETAG, &a.etag)
            .header(header::CACHE_CONTROL, cache)
            .header(header::VARY, "Accept-Encoding")
            .header("X-Content-Type-Options", "nosniff");
        if let Some(e) = enc {
            b = b.header(header::CONTENT_ENCODING, HeaderValue::from_static(e));
        }
        b.body(Body::from(body)).unwrap()
    }
}

fn walk(root: &Path, dir: &Path, f: &mut dyn FnMut(String, Vec<u8>) -> anyhow::Result<()>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            walk(root, &p, f)?;
        } else {
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            if rel.starts_with('.') {
                continue;
            }
            f(rel, std::fs::read(&p)?)?;
        }
    }
    Ok(())
}
