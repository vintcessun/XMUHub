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
