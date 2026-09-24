//! A loose per-IP request limit, so one runaway script can't hog the server. Normal use
//! (even a whole dorm behind one NAT address) stays far below it.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use parking_lot::Mutex;
use serde_json::json;

/// Requests per second per IP (also the burst size). `XMUHUB_RATE_LIMIT=0` turns it off.
fn rate() -> f64 {
    static R: OnceLock<f64> = OnceLock::new();
    *R.get_or_init(|| std::env::var("XMUHUB_RATE_LIMIT").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(200.0))
}

/// ip → (tokens left, last refill)
static BUCKETS: OnceLock<Mutex<HashMap<String, (f64, Instant)>>> = OnceLock::new();

fn allow(ip: &str) -> bool {
    let rate = rate();
    if rate <= 0.0 || ip == "local" {
        return true;
    }
    let now = Instant::now();
    let mut map = BUCKETS.get_or_init(Default::default).lock();
    if map.len() > 50_000 {
        // Forget addresses that have been quiet long enough to be full again.
        map.retain(|_, (_, at)| now.duration_since(*at).as_secs_f64() < 1.0);
    }
    let (tokens, at) = map.entry(ip.to_string()).or_insert((rate, now));
    *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(rate);
    *at = now;
    if *tokens >= 1.0 {
        *tokens -= 1.0;
        true
    } else {
        false
    }
}

pub async fn guard(req: Request, next: Next) -> Response {
    if allow(&crate::api::client_ip(req.headers())) {
        return next.run(req).await;
    }
    let mut res = (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "请求太频繁，请稍后再试" }))).into_response();
    res.headers_mut().insert("retry-after", axum::http::HeaderValue::from_static("1"));
    res
}
