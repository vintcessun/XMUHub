//! Static site. Files under `web/` are read once at startup, compressed (brotli + gzip)
//! and served from memory with ETags. Edit files and restart (deploy does) to update.
//! Also builds the Content-Security-Policy (with the hashes of the pages' inline scripts)
//! and the middleware that puts the security headers on every response.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use bytes::Bytes;
use sha2::{Digest, Sha256};

pub struct Asset {
    mime: &'static str,
    raw: Bytes,
    br: Option<Bytes>,
    gz: Option<Bytes>,
    etag: String,
    /// Content key (see versioning.rs): a request whose `?v=` matches it may be cached for good.
    key: String,
    /// Third-party libraries live in versioned folders (`vendor/<name>-<version>/`): always final.
    vendor: bool,
}

pub struct Site {
    files: HashMap<String, Asset>,
    csp: HeaderValue,
}

/// Origins the pages load code or frames from besides our own:
/// Cloudflare Turnstile (login) and Microsoft's viewer for legacy .doc / .ppt previews.
const TURNSTILE: &str = "https://challenges.cloudflare.com";
const OFFICE_VIEWER: &str = "https://view.officeapps.live.com";

/// The Content-Security-Policy for every response. Scripts come only from this site (plus
/// Turnstile) and the pages' own fixed inline snippets, allowed by hash. Material files are
/// fetched by the browser straight from many GitHub mirrors and uploads go to the upload
/// Worker, so images / media / fetch may use any https origin. Previews build pages from
/// blob: and data: URLs (pdf.js, docx-preview, epub.js), and wasm (libarchive, hash-wasm)
/// needs 'wasm-unsafe-eval'. Speculation rules come from a file ([`SPECULATION_RULES`]): with hashes
/// in script-src, Chrome ignores 'inline-speculation-rules', so inline rules would be blocked.
fn build_csp(script_hashes: &BTreeSet<String>) -> String {
    let hashes: String = script_hashes.iter().map(|h| format!(" 'sha256-{h}'")).collect();
    [
        "default-src 'self'".to_string(),
        format!("script-src 'self' 'wasm-unsafe-eval' {TURNSTILE}{hashes}"),
        "style-src 'self' 'unsafe-inline' blob:".to_string(),
        "img-src 'self' data: blob: https:".to_string(),
        "font-src 'self' data: blob:".to_string(),
        "connect-src 'self' https:".to_string(),
        "media-src 'self' blob: https:".to_string(),
        "worker-src 'self' blob:".to_string(),
        format!("frame-src 'self' blob: {TURNSTILE} {OFFICE_VIEWER}"),
        "object-src 'none'".to_string(),
        // 'self', not 'none': epub.js puts a same-origin <base> in its srcdoc chapter frames.
        "base-uri 'self'".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'none'".to_string(),
    ]
    .join("; ")
}

/// Base64 SHA-256 of the body of every inline `<script>` (one without `src`) in an HTML page.
fn inline_script_hashes(html: &[u8], out: &mut BTreeSet<String>) {
    let lower = html.to_ascii_lowercase();
    let find = |from: usize, pat: &[u8]| lower[from..].windows(pat.len()).position(|w| w == pat).map(|p| p + from);
    let mut at = 0;
    while let Some(open) = find(at, b"<script") {
        let Some(tag_end) = find(open, b">") else { break };
        at = tag_end + 1;
        let attrs = &lower[open + b"<script".len()..tag_end];
        if attrs.first().is_some_and(|c| !c.is_ascii_whitespace()) {
            continue; // not a <script> tag, e.g. <scripts>
        }
        let Some(close) = find(at, b"</script") else { break };
        let has_src = attrs.split(|c| c.is_ascii_whitespace()).any(|a| a == b"src" || a.starts_with(b"src="));
        if !has_src {
            out.insert(base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&html[at..close])));
        }
        at = close;
    }
}

fn compressible(mime: &str) -> bool {
    mime.starts_with("text/") || mime.contains("javascript") || mime.contains("json") || mime.contains("svg") || mime.contains("xml")
}

/// Prefetch rules for the pages that still open with a full load (upload, account), sent
/// with every page in a `Speculation-Rules` header (Chrome/Edge fetch them on hover).
const SPECULATION_RULES: &str = "assets/speculation-rules.json";

fn mime_of(path: &str) -> &'static str {
    if path == SPECULATION_RULES {
        return "application/speculationrules+json";
    }
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
        let mut raw_files = HashMap::new();
        walk(dir, dir, &mut |rel, raw| {
            raw_files.insert(rel, raw);
            Ok(())
        })?;
        let crate::versioning::Versioned { files: versioned, keys } = crate::versioning::version(raw_files);
        let mut files = HashMap::new();
        let mut total = 0usize;
        let mut script_hashes = BTreeSet::new();
        for (rel, raw) in versioned {
            let mime = mime_of(&rel);
            if rel.ends_with(".html") {
                inline_script_hashes(&raw, &mut script_hashes);
            }
            let (br, gz) = if compressible(mime) && raw.len() > 512 {
                let mut br = Vec::new();
                {
                    // Max quality for our own small files; big vendor libraries (pdf.js, SheetJS…)
                    // at quality 11 would add seconds to every start-up for a few % of size.
                    let quality = if raw.len() > 128 * 1024 { 6 } else { 11 };
                    let mut w = brotli::CompressorWriter::new(&mut br, 4096, quality, 22);
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
            let key = keys.get(&rel).cloned().unwrap_or_default();
            let vendor = rel.starts_with("vendor/");
            files.insert(rel, Asset { mime, raw: Bytes::from(raw), br, gz, etag, key, vendor });
        }
        tracing::info!(files = files.len(), bytes = total, inline_scripts = script_hashes.len(), "static site loaded");
        let csp = HeaderValue::from_str(&build_csp(&script_hashes))?;
        Ok(Site { files, csp })
    }

    /// The Content-Security-Policy header value, for [`security_headers`].
    pub fn csp(&self) -> HeaderValue {
        self.csp.clone()
    }

    pub fn get(&self, path: &str) -> Option<&Asset> {
        self.files.get(path)
    }

    /// `version`: the request's `?v=`. Pages link every script, style, font and image with
    /// its content key, so a request carrying the current key can be kept by browsers and
    /// Cloudflare for a year (a new deploy changes the key, hence the URL). Anything else —
    /// pages themselves, unversioned or outdated keys — revalidates every time (a 304 costs
    /// ~200 bytes), so old and new files never mix.
    pub fn respond(self: &Arc<Self>, path: &str, version: Option<&str>, req: &HeaderMap, status: StatusCode) -> Response {
        let Some(a) = self.files.get(path) else {
            return match self.files.get("404.html") {
                Some(_) if path != "404.html" => self.respond("404.html", None, req, StatusCode::NOT_FOUND),
                _ => (StatusCode::NOT_FOUND, "not found").into_response(),
            };
        };
        let fresh = a.vendor || (status == StatusCode::OK && !a.key.is_empty() && version == Some(a.key.as_str()));
        let cache = if fresh { "public, max-age=31536000, immutable" } else { "no-cache" };
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
            .header(header::VARY, "Accept-Encoding");
        if let Some(e) = enc {
            b = b.header(header::CONTENT_ENCODING, HeaderValue::from_static(e));
        }
        if a.mime.starts_with("text/html") && self.files.contains_key(SPECULATION_RULES) {
            b = b.header("Speculation-Rules", format!("\"/{SPECULATION_RULES}\""));
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

/// Middleware: security headers on every response (pages, static files, JSON, errors).
pub async fn security_headers(State(csp): State<HeaderValue>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, csp);
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("strict-origin-when-cross-origin"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(s.as_bytes()))
    }

    #[test]
    fn hashes_only_inline_scripts() {
        let html = "<head><SCRIPT>a()</SCRIPT><script type=\"module\" src=\"/x.js\"></script>\n\
                    <script>\nb()\n</script><scripts>x</scripts><script>a()</script></head>";
        let mut out = BTreeSet::new();
        inline_script_hashes(html.as_bytes(), &mut out);
        assert_eq!(out, BTreeSet::from([hash("a()"), hash("\nb()\n")]));
    }

    #[test]
    fn csp_lists_hashes() {
        let csp = build_csp(&BTreeSet::from(["abc=".to_string()]));
        assert!(csp.contains("script-src 'self' 'wasm-unsafe-eval' https://challenges.cloudflare.com 'sha256-abc='"));
        assert!(csp.contains("frame-ancestors 'none'"));
    }
}
