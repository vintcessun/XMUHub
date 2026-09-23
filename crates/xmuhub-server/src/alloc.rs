//! mimalloc, tuned to hand freed memory back to the OS promptly: the host has little RAM
//! to spare and a long-running server should not sit on its high-water mark.

use libmimalloc_sys as ffi;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Option indices of the bundled mimalloc v3 (`mi_option_e` in mimalloc.h); the Rust
// bindings don't export names for these. Only applied when the runtime reports v3.
// Sanity anchor: the bindings define eager_commit_delay = 14 and use_numa_nodes = 16.
const MI_OPTION_PURGE_DELAY: ffi::mi_option_t = 15;
const MI_OPTION_ARENA_PURGE_MULT: ffi::mi_option_t = 24;
const MI_OPTION_ALLOW_THP: ffi::mi_option_t = 43;

/// Call first thing in `main`. Returns (version, purge_delay before, purge_delay after).
pub fn tune() -> (i32, i64, i64) {
    unsafe {
        let version = ffi::mi_version();
        let before = ffi::mi_option_get(MI_OPTION_PURGE_DELAY) as i64;
        if version / 10000 == 3 {
            // Purge freed pages immediately instead of after the default 10 ms × arena multiplier.
            ffi::mi_option_set(MI_OPTION_PURGE_DELAY, 0);
            ffi::mi_option_set(MI_OPTION_ARENA_PURGE_MULT, 1);
            // Transparent huge pages make RSS jump in 2 MiB steps and are hard to give back.
            ffi::mi_option_set(MI_OPTION_ALLOW_THP, 0);
        }
        (version, before, ffi::mi_option_get(MI_OPTION_PURGE_DELAY) as i64)
    }
}

/// Forces mimalloc to return all unused memory it is holding. Cheap; run periodically.
pub fn collect() {
    unsafe { ffi::mi_collect(true) }
}

/// Resident set size in bytes (Linux), for the admin status page.
pub fn rss_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}
