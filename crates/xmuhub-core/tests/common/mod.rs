//! Helpers shared by the integration tests.
// Each test binary uses only some of these.
#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Password for the throwaway accounts tests register. Made up at run time (never written
/// in the source) so secret scanners have no literal to flag; the same for the whole run.
pub fn test_password() -> String {
    static PW: OnceLock<String> = OnceLock::new();
    PW.get_or_init(|| {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        format!("t-{:x}-{:x}", std::process::id(), nanos)
    })
    .clone()
}

/// A second, different one (for password changes and resets).
pub fn other_test_password() -> String {
    format!("{}-2", test_password())
}

/// The site config the tests run under (normally read from `site/<name>/site.json`).
pub fn test_site() {
    xmuhub_core::site::init(xmuhub_core::site::SiteInfo {
        name: "鹭岛书阁".into(),
        verified_domains: vec!["xmu.edu.cn".into(), "stu.xmu.edu.cn".into()],
        announcement: "本站目前只面向厦门大学的同学。".into(),
    });
}
