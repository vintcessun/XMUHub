//! Streaming upload relay: browser → this server → uploads.github.com.
//!
//! Used when the Cloudflare Worker is unreachable from mainland China (workers.dev is
//! DNS-poisoned). Bytes are forwarded as they arrive — nothing is written to disk and only
//! a few socket buffers are held in memory. Accepts exactly what a signed ticket allows.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

use xmuhub_core::model::now;
use xmuhub_core::ticket;

use crate::api::App;

pub struct Relay {
    pub http: reqwest::Client,
    pub secret: Vec<u8>,
    pub owner: String,
    pub token: String,
    pub slots: Semaphore,
    pub daily_cap: u64,
    /// (day, bytes relayed that day)
    pub used: Mutex<(i64, u64)>,
}

impl Relay {
    pub fn new(secret: Vec<u8>, owner: String, token: String, concurrency: usize, daily_cap: u64) -> anyhow::Result<Relay> {
        let http = reqwest::Client::builder()
            .user_agent("XMUHub-relay/0.1")
            .connect_timeout(Duration::from_secs(15))
            // No proxy: the host reaches uploads.github.com directly and proxy traffic is metered.
            .no_proxy()
            .build()?;
        Ok(Relay { http, secret, owner, token, slots: Semaphore::new(concurrency), daily_cap, used: Mutex::new((0, 0)) })
    }

    /// Reserves `bytes` of today's budget; returns false when the cap would be exceeded.
    fn charge(&self, bytes: u64) -> bool {
        let day = now() / 86400;
        let mut u = self.used.lock();
        if u.0 != day {
            *u = (day, 0);
        }
        if u.1 + bytes > self.daily_cap {
            return false;
        }
        u.1 += bytes;
        true
    }

    fn refund(&self, bytes: u64) {
        let mut u = self.used.lock();
        u.1 = u.1.saturating_sub(bytes);
    }

    pub fn used_today(&self) -> u64 {
        let u = self.used.lock();
        if u.0 == now() / 86400 { u.1 } else { 0 }
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, axum::Json(json!({ "error": msg.into() }))).into_response()
}

#[derive(Deserialize)]
pub struct TicketQ {
    t: String,
}

pub async fn upload(State(app): State<std::sync::Arc<App>>, Query(q): Query<TicketQ>, headers: HeaderMap, body: Body) -> Response {
    let Some(relay) = app.relay.as_ref() else { return err(StatusCode::NOT_FOUND, "中转未启用") };
    let Ok(t) = ticket::verify(&relay.secret, &q.t) else {
        return err(StatusCode::FORBIDDEN, "上传凭证无效或已过期，请刷新页面重试");
    };
    if !t.u.starts_with(&format!("https://uploads.github.com/repos/{}/", relay.owner)) {
        return err(StatusCode::FORBIDDEN, "bad destination");
    }
    let len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if len != Some(t.s) {
        return err(StatusCode::BAD_REQUEST, format!("文件大小不符（{len:?} ≠ {}）", t.s));
    }
    let Ok(_slot) = relay.slots.try_acquire() else {
        return err(StatusCode::TOO_MANY_REQUESTS, "当前上传人数较多，请稍后重试");
    };
    if !relay.charge(t.s) {
        return err(StatusCode::TOO_MANY_REQUESTS, "今天全站的上传流量已用完，请明天再来");
    }

    let expected = t.s;
    let mut seen = 0u64;
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        seen += chunk.len() as u64;
        if seen > expected {
            return Err(std::io::Error::other("body larger than ticket"));
        }
        Ok::<_, std::io::Error>(chunk)
    });

    let res = relay
        .http
        .post(&t.u)
        .bearer_auth(&relay.token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        // Explicit length keeps hyper from falling back to chunked encoding, which GitHub rejects.
        .header(header::CONTENT_LENGTH, t.s)
        .timeout(Duration::from_secs(30 * 60))
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await;

    match res {
        Ok(r) if r.status().is_success() => {
            let text = r.text().await.unwrap_or_default();
            (StatusCode::CREATED, [(header::CONTENT_TYPE, "application/json")], text).into_response()
        }
        Ok(r) => {
            relay.refund(t.s);
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            tracing::warn!(%status, "relay: github rejected upload: {msg}");
            let code = if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY { StatusCode::CONFLICT } else { StatusCode::BAD_GATEWAY };
            err(code, format!("GitHub 返回 {status}：{msg}"))
        }
        Err(e) => {
            relay.refund(t.s);
            tracing::warn!("relay: {e}");
            err(StatusCode::BAD_GATEWAY, "转存到 GitHub 失败，请重试")
        }
    }
}
