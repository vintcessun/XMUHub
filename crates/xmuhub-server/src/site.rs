//! The site this server runs: `site/<name>/site.json` plus files that replace or add to
//! `web/` (logo, pictures, whole pages). See `site/README.md` and `docs/fork.md`.
//!
//! Pages and scripts under `web/` say `{{site.name}}`, `{{site.school}}` … instead of any
//! site's own words; [`Site::fill`] puts the values in when the static files are loaded.
//! A fork only edits its own `site/<name>/`, so merging upstream never conflicts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteConfig {
    /// 站名.
    pub name: String,
    /// Shown under the name in the header (a few characters).
    pub subtitle: String,
    /// The school's full name and short name.
    pub school: String,
    pub school_short: String,
    /// `<meta name="description">` of every page.
    pub description: String,
    /// The line under the name on the home page.
    pub slogan: String,
    /// The first line of the footer.
    pub footer: String,
    /// The public host name, e.g. `hub.example.edu` (MCP instructions, examples on pages).
    pub domain: String,
    /// The source repository linked from the pages (a fork puts its own).
    pub repo: String,
    /// Short id of the MCP server (`claude mcp add … <mcp_name> …`).
    pub mcp_name: String,
    /// A chat group or other contact shown next to 意见反馈; empty for none.
    #[serde(default)]
    pub community: String,
    /// The school's email domains; addresses there count as 认证.
    #[serde(default)]
    pub verified_domains: Vec<String>,
    /// The badge on verified accounts.
    pub verified_label: String,
    /// An example for the 站外资源 name field.
    pub link_example: String,
    /// The site-wide notice until an admin sets one.
    pub announcement: String,
    /// Anything else pages may use as `{{site.extra.<key>}}`.
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

pub struct Site {
    pub dir: PathBuf,
    pub config: SiteConfig,
    vars: BTreeMap<String, String>,
}

static SITE: OnceLock<Site> = OnceLock::new();

/// The site loaded at start-up.
pub fn get() -> &'static Site {
    SITE.get().expect("site loaded at start-up")
}

/// Characters a value may not contain: it lands inside HTML attributes and JS strings unescaped.
const FORBIDDEN: &[char] = &['<', '>', '"', '\'', '`', '\\', '\n', '\r', '{', '}'];

impl Site {
    /// Finds the site folder: `XMUHUB_SITE_DIR`; else `_site/` inside the web folder (where
    /// deploy puts it); else `site/<XMUHUB_SITE or xmu>/` (a checkout).
    pub fn locate(web_dir: &Path) -> PathBuf {
        if let Ok(d) = std::env::var("XMUHUB_SITE_DIR")
            && !d.trim().is_empty()
        {
            return PathBuf::from(d.trim());
        }
        let deployed = web_dir.join("_site");
        if deployed.join("site.json").is_file() {
            return deployed;
        }
        let name = std::env::var("XMUHUB_SITE").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "xmu".into());
        web_dir.parent().unwrap_or(Path::new(".")).join("site").join(name)
    }

    pub fn load(dir: &Path) -> anyhow::Result<&'static Site> {
        let path = dir.join("site.json");
        let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let config: SiteConfig = serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let mut vars = BTreeMap::new();
        let domains: Vec<String> = config.verified_domains.iter().map(|d| format!("@{}", d.trim_start_matches('@'))).collect();
        for (k, v) in [
            ("name", &config.name),
            ("subtitle", &config.subtitle),
            ("school", &config.school),
            ("school_short", &config.school_short),
            ("description", &config.description),
            ("slogan", &config.slogan),
            ("footer", &config.footer),
            ("domain", &config.domain),
            ("repo", &config.repo),
            ("mcp_name", &config.mcp_name),
            ("community", &config.community),
            ("verified_label", &config.verified_label),
            ("link_example", &config.link_example),
            ("verified_emails", &domains.join(" / ")),
        ] {
            vars.insert(k.to_string(), v.clone());
        }
        for (k, v) in &config.extra {
            vars.insert(format!("extra.{k}"), v.clone());
        }
        for (k, v) in &vars {
            if let Some(c) = v.chars().find(|c| FORBIDDEN.contains(c)) {
                anyhow::bail!("{}: `{k}` may not contain {c:?}", path.display());
            }
        }
        xmuhub_core::site::init(xmuhub_core::site::SiteInfo {
            name: config.name.clone(),
            verified_domains: config.verified_domains.clone(),
            announcement: config.announcement.clone(),
        });
        let _ = SITE.set(Site { dir: dir.to_path_buf(), config, vars });
        tracing::info!(site = %dir.display(), name = %get().config.name, "site loaded");
        Ok(get())
    }

    /// Replaces every `{{site.<key>}}` in a text file. An unknown key is an error, so a typo
    /// can't reach visitors as a literal placeholder.
    pub fn fill(&self, file: &str, text: &str) -> anyhow::Result<String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find("{{site.") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            let end = after.find("}}").ok_or_else(|| anyhow::anyhow!("{file}: unclosed {{{{site."))?;
            let key = &after["site.".len()..end];
            let value = self.vars.get(key).ok_or_else(|| anyhow::anyhow!("{file}: unknown placeholder {{{{site.{key}}}}} (see site/README.md)"))?;
            out.push_str(value);
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// Files whose text may hold placeholders.
pub fn is_template(path: &str) -> bool {
    matches!(path.rsplit('.').next(), Some("html" | "js" | "mjs" | "css" | "json" | "txt" | "svg" | "webmanifest"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        let vars = BTreeMap::from([("name".to_string(), "测试书阁".to_string()), ("domain".to_string(), "hub.invalid".to_string())]);
        let config = serde_json::from_str::<SiteConfig>(include_str!("../../../site/example/site.json")).unwrap();
        Site { dir: PathBuf::new(), config, vars }
    }

    #[test]
    fn fills_known_placeholders_and_rejects_unknown() {
        let s = site();
        assert_eq!(s.fill("a.html", "<b>{{site.name}}</b> https://{{site.domain}}/r/1").unwrap(), "<b>测试书阁</b> https://hub.invalid/r/1");
        assert_eq!(s.fill("a.js", "no placeholders {{x}}").unwrap(), "no placeholders {{x}}");
        assert!(s.fill("a.html", "{{site.nmae}}").is_err(), "a typo is caught at start-up");
        assert!(s.fill("a.html", "{{site.name").is_err());
    }

    #[test]
    fn templates_are_text_files() {
        assert!(is_template("index.html") && is_template("assets/app.js") && is_template("assets/app.css"));
        assert!(!is_template("assets/site/logo.png") && !is_template("vendor/x/y.wasm"));
    }
}
