//! Content-hash versions for the static site, so browsers and Cloudflare can keep scripts,
//! styles, fonts and images for a year and still never mix old and new files after a deploy.
//!
//! At start-up every local reference in the pages, scripts and styles (`/assets/…` in quotes
//! or `url(…)`, and relative module imports `from '../app.js'` / `import('./preview.js')`)
//! gets `?v=<key>` appended. A file's key covers its own bytes and the bytes of everything it
//! references, directly or not (so a page script's key changes when `app.js` does). Files
//! that import each other are fine: the key is over the set of files reachable from it.
//! No build step: `web/` stays as written; the rewriting happens in memory.

use std::collections::{BTreeSet, HashMap};

use regex::Regex;
use sha2::{Digest, Sha256};

/// `<meta name=…>` put into every page with the version of `assets/app.js`. The soft-navigation
/// router compares it: a page from a newer deploy is opened with a full load instead of
/// running a second copy of the app module next to the old one.
pub const VERSION_META: &str = "xmuhub-version";

pub struct Versioned {
    /// Path → bytes, references rewritten.
    pub files: HashMap<String, Vec<u8>>,
    /// Path → key (for every file; `?v=` must match it for a file to be cached for good).
    pub keys: HashMap<String, String>,
}

/// A reference found in a text file: where the path ends in the text, and the file it names.
struct Ref {
    end: usize,
    target: String,
}

fn is_text(path: &str) -> bool {
    path.ends_with(".html") || path.ends_with(".js") || path.ends_with(".mjs") || path.ends_with(".css")
}

/// Resolves `./x.js` / `../x.js` against the directory of `from`.
fn resolve(from: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in rel.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

fn find_refs(path: &str, text: &str, exists: &dyn Fn(&str) -> bool) -> Vec<Ref> {
    static ABS: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"["'(]/((?:assets|vendor)/[A-Za-z0-9_./-]+\.[A-Za-z0-9]+)(.)"#).unwrap());
    static IMPORT: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"(?:\bfrom\s*|\bimport\s*\(\s*)["'](\.\.?/[A-Za-z0-9_./-]+\.m?js)(.)"#).unwrap());
    let mut out = Vec::new();
    for c in ABS.captures_iter(text) {
        let (m, after) = (c.get(1).unwrap(), c.get(2).unwrap().as_str());
        // Already versioned, or a longer path this pattern only partly matched.
        if !matches!(after, "\"" | "'" | ")") || !exists(m.as_str()) {
            continue;
        }
        out.push(Ref { end: m.end(), target: m.as_str().to_string() });
    }
    if !path.ends_with(".html") && !path.ends_with(".css") {
        for c in IMPORT.captures_iter(text) {
            let (m, after) = (c.get(1).unwrap(), c.get(2).unwrap().as_str());
            let Some(target) = resolve(path, m.as_str()).filter(|t| exists(t)) else { continue };
            if matches!(after, "\"" | "'") {
                out.push(Ref { end: m.end(), target });
            }
        }
    }
    out
}

fn hex_sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn version(raw: HashMap<String, Vec<u8>>) -> Versioned {
    let exists = |p: &str| raw.contains_key(p);
    let mut refs: HashMap<String, Vec<Ref>> = HashMap::new();
    for (path, bytes) in &raw {
        if is_text(path)
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            refs.insert(path.clone(), find_refs(path, text, &exists));
        }
    }
    let own: HashMap<&str, String> = raw.iter().map(|(p, b)| (p.as_str(), hex_sha(b))).collect();
    // key = hash over the own-hashes of every file reachable from this one (itself included).
    let mut keys = HashMap::new();
    for path in raw.keys() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack = vec![path.as_str()];
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            for r in refs.get(p).into_iter().flatten() {
                stack.push(r.target.as_str());
            }
        }
        let mut h = Sha256::new();
        for p in &seen {
            h.update(p.as_bytes());
            h.update(own[p].as_bytes());
        }
        keys.insert(path.clone(), hex::encode(h.finalize())[..12].to_string());
    }
    let app_version = keys.get("assets/app.js").cloned().unwrap_or_default();
    let mut files = HashMap::new();
    for (path, bytes) in raw {
        let Some(found) = refs.get(&path).filter(|r| !r.is_empty() || path.ends_with(".html")) else {
            files.insert(path, bytes);
            continue;
        };
        let text = String::from_utf8(bytes).expect("checked above");
        let mut out = String::with_capacity(text.len() + found.len() * 16 + 64);
        let mut at = 0;
        let mut sorted: Vec<&Ref> = found.iter().collect();
        sorted.sort_by_key(|r| r.end);
        for r in sorted {
            out.push_str(&text[at..r.end]);
            out.push_str("?v=");
            out.push_str(&keys[&r.target]);
            at = r.end;
        }
        out.push_str(&text[at..]);
        if path.ends_with(".html")
            && let Some(i) = out.find("<head>")
        {
            out.insert_str(i + "<head>".len(), &format!("\n<meta name=\"{VERSION_META}\" content=\"{app_version}\">"));
        }
        files.insert(path, out.into_bytes());
    }
    Versioned { files, keys }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(files: &[(&str, &str)]) -> Versioned {
        version(files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect())
    }

    fn text(v: &Versioned, p: &str) -> String {
        String::from_utf8(v.files[p].clone()).unwrap()
    }

    #[test]
    fn references_get_the_target_key() {
        let v = site(&[
            ("index.html", "<head><link rel=\"stylesheet\" href=\"/assets/app.css\"></head><script type=\"module\" src=\"/assets/pages/home.js\"></script><img src=\"/assets/missing.png\">"),
            ("assets/app.css", "body{background:url('/assets/bg.jpg')}"),
            ("assets/bg.jpg", "jpg"),
            ("assets/app.js", "const m = await import('./preview.js'); const logo = \"/assets/bg.jpg\";"),
            ("assets/preview.js", "import { api } from './app.js';"),
            ("assets/pages/home.js", "import { api } from '../app.js';"),
        ]);
        let html = text(&v, "index.html");
        assert!(html.contains(&format!("/assets/app.css?v={}", v.keys["assets/app.css"])));
        assert!(html.contains(&format!("/assets/pages/home.js?v={}", v.keys["assets/pages/home.js"])));
        assert!(html.contains("/assets/missing.png\""), "unknown files are left alone");
        assert!(html.contains(&format!("<meta name=\"{VERSION_META}\" content=\"{}\">", v.keys["assets/app.js"])));
        assert!(text(&v, "assets/app.css").contains(&format!("url('/assets/bg.jpg?v={}')", v.keys["assets/bg.jpg"])));
        assert!(text(&v, "assets/pages/home.js").contains(&format!("from '../app.js?v={}'", v.keys["assets/app.js"])));
        assert!(text(&v, "assets/app.js").contains(&format!("import('./preview.js?v={}')", v.keys["assets/preview.js"])));
        assert!(text(&v, "assets/preview.js").contains(&format!("from './app.js?v={}'", v.keys["assets/app.js"])));
    }

    #[test]
    fn keys_follow_what_a_file_depends_on() {
        let base = [
            ("assets/app.js", "import('./preview.js')"),
            ("assets/preview.js", "import { a } from './app.js';"),
            ("assets/pages/home.js", "import { a } from '../app.js';"),
            ("assets/app.css", "body{}"),
        ];
        let before = site(&base);
        let mut changed = base;
        changed[1] = ("assets/preview.js", "import { a } from './app.js'; // new");
        let after = site(&changed);
        // preview.js is reachable from the page and the app (and they from each other): all change.
        for p in ["assets/app.js", "assets/preview.js", "assets/pages/home.js"] {
            assert_ne!(before.keys[p], after.keys[p], "{p}");
        }
        assert_eq!(before.keys["assets/app.css"], after.keys["assets/app.css"], "unrelated files keep their key");
    }

    #[test]
    fn already_versioned_references_are_kept() {
        let v = site(&[("a.html", "<script src=\"/assets/x.js?v=abc\"></script>"), ("assets/x.js", "1")]);
        assert!(text(&v, "a.html").contains("/assets/x.js?v=abc\""));
    }
}
