//! What the core needs to know about the site it runs (set once at start-up from the site's
//! `site.json`, see `site/README.md`): its name, the school's email domains, the default
//! announcement. Everything site-specific lives in `site/<name>/`, never in the code, so a
//! fork changes its own folder and can keep merging upstream without conflicts.

use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct SiteInfo {
    /// The site's name (nobody but staff may take it as a nickname).
    pub name: String,
    /// The school's email domains, e.g. `school.edu`: an address at one of them or any of its
    /// subdomains counts as verified (「认证」) and never needs a human check to sign up.
    pub verified_domains: Vec<String>,
    /// The site-wide notice shown until an admin sets one.
    pub announcement: String,
}

static SITE: OnceLock<SiteInfo> = OnceLock::new();

/// Sets the site once, at start-up; later calls are ignored.
pub fn init(info: SiteInfo) {
    let _ = SITE.set(SiteInfo {
        verified_domains: info.verified_domains.iter().map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase()).filter(|d| !d.is_empty()).collect(),
        ..info
    });
}

pub fn get() -> &'static SiteInfo {
    static EMPTY: OnceLock<SiteInfo> = OnceLock::new();
    SITE.get().unwrap_or_else(|| EMPTY.get_or_init(SiteInfo::default))
}

/// Whether `domain` (lower case) is one of the school's domains or a subdomain of one.
pub fn is_school_domain(domain: &str) -> bool {
    get().verified_domains.iter().any(|d| domain == d || domain.strip_suffix(d.as_str()).is_some_and(|rest| rest.ends_with('.')))
}

/// Whether an (already normalized) address is at the school's domains.
pub fn is_school_email(email: &str) -> bool {
    email.rsplit_once('@').is_some_and(|(_, domain)| is_school_domain(domain))
}
