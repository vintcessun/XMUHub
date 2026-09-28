//! Signed, expiring upload tickets: `base64url(json) "." base64url(hmac_sha256)`.
//! The Cloudflare upload Worker verifies the same format with the same secret.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{Error, Result};
use crate::model::now;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    /// Destination the bytes are forwarded to.
    pub u: String,
    /// Exact byte count the upload must carry.
    pub s: u64,
    /// Expiry, unix seconds.
    pub e: i64,
}

fn mac(secret: &[u8], payload: &[u8]) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(secret).expect("hmac accepts any key length");
    m.update(payload);
    m
}

pub fn sign(secret: &[u8], t: &Ticket) -> String {
    let payload = B64.encode(serde_json::to_vec(t).expect("ticket json"));
    let sig = B64.encode(mac(secret, payload.as_bytes()).finalize().into_bytes());
    format!("{payload}.{sig}")
}

pub fn verify(secret: &[u8], s: &str) -> Result<Ticket> {
    let (payload, sig) = s.split_once('.').ok_or(Error::Forbidden)?;
    let sig = B64.decode(sig).map_err(|_| Error::Forbidden)?;
    mac(secret, payload.as_bytes()).verify_slice(&sig).map_err(|_| Error::Forbidden)?;
    let json = B64.decode(payload).map_err(|_| Error::Forbidden)?;
    let t: Ticket = serde_json::from_slice(&json).map_err(|_| Error::Forbidden)?;
    if t.e < now() {
        return Err(Error::Forbidden);
    }
    Ok(t)
}

/// A receipt for something submitted without an account (feedback): whoever holds it can
/// look that item up later. Derived from the secret, so nothing is stored and it can't be
/// guessed from the id.
pub fn receipt(secret: &[u8], kind: &str, id: u64) -> String {
    let tag = mac(secret, format!("{kind}:{id}").as_bytes()).finalize().into_bytes();
    hex::encode(&tag[..12])
}

/// Whether `given` is the receipt for `kind` `id` (constant-time).
pub fn check_receipt(secret: &[u8], kind: &str, id: u64, given: &str) -> bool {
    let want = receipt(secret, kind, id);
    want.len() == given.len() && want.bytes().zip(given.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn receipts_verify_only_for_their_item() {
        let r = receipt(b"secret", "feedback", 42);
        assert!(check_receipt(b"secret", "feedback", 42, &r));
        assert!(!check_receipt(b"secret", "feedback", 43, &r));
        assert!(!check_receipt(b"other", "feedback", 42, &r));
        assert!(!check_receipt(b"secret", "feedback", 42, ""));
    }
}
