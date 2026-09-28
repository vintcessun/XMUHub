//! Cloudflare Turnstile: the human check that sign-ups from uncommon mail domains (see
//! `xmuhub_core::hub::needs_captcha`) must pass before a code is mailed. The page renders
//! the widget with the site key; the token it yields is checked here with the secret.

use std::time::Duration;

use serde_json::{Value, json};

const VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

pub struct Turnstile {
    pub sitekey: String,
    secret: String,
    http: reqwest::Client,
}

impl Turnstile {
    pub fn new(sitekey: String, secret: String, http: reqwest::Client) -> Turnstile {
        Turnstile { sitekey, secret, http }
    }

    /// Whether `token` (from the widget on the page) is a check that just passed.
    /// Any failure to ask Cloudflare counts as not passed.
    pub async fn verify(&self, token: &str, ip: &str) -> bool {
        if token.is_empty() || token.len() > 2048 {
            return false;
        }
        let body = json!({ "secret": self.secret, "response": token, "remoteip": ip });
        let res = self
            .http
            .post(VERIFY_URL)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        let bytes = match res {
            Ok(r) => r.bytes().await,
            Err(e) => {
                tracing::warn!("turnstile verify: {e}");
                return false;
            }
        };
        bytes.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["success"].as_bool()).unwrap_or(false)
    }
}
