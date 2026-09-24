//! Personal access tokens: `Authorization: Bearer xmh_…` for scripts and AI agents (REST API
//! and MCP). A token acts as its owner with the owner's current role; only its hash is stored.

use serde::Serialize;

use super::accounts::sha;
use super::{Hub, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

pub const TOKEN_PREFIX: &str = "xmh_";
const MAX_TOKENS: usize = 10;
/// `last_used` is persisted at most this often per token.
const TOUCH_EVERY: i64 = 3600;

#[derive(Debug, Clone, Serialize)]
pub struct TokenView {
    pub id: Id,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub last_used: i64,
}

impl From<&ApiToken> for TokenView {
    fn from(t: &ApiToken) -> Self {
        TokenView { id: t.id, name: t.name.clone(), prefix: t.prefix.clone(), created_at: t.created_at, last_used: t.last_used }
    }
}

impl Hub {
    /// Creates a token; the secret is returned once and never stored.
    pub fn create_token(&self, viewer: Viewer, name: &str) -> Result<(String, TokenView)> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let name = clean(name, 40);
        let name = if name.is_empty() { "未命名".to_string() } else { name };
        self.mutate(|st, tx| {
            if st.tokens.values().filter(|t| t.user == me).count() >= MAX_TOKENS {
                return Err(bad("最多 10 个令牌，请先删除不用的"));
            }
            let secret = format!("{TOKEN_PREFIX}{}", hex::encode(rand::random::<[u8; 24]>()));
            let t = ApiToken {
                id: st.next_id(tx)?,
                user: me,
                name,
                hash: sha(&secret),
                prefix: secret[..TOKEN_PREFIX.len() + 6].to_string(),
                created_at: now(),
                last_used: 0,
            };
            tx.put_token(&t)?;
            st.token_by_hash.insert(t.hash, t.id);
            st.tokens.insert(t.id, t.clone());
            Ok((secret, TokenView::from(&t)))
        })
    }

    pub fn tokens(&self, viewer: Viewer) -> Result<Vec<TokenView>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<TokenView> = st.tokens.values().filter(|t| t.user == me).map(TokenView::from).collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.id));
        Ok(v)
    }

    pub fn revoke_token(&self, viewer: Viewer, id: Id) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let t = st.tokens.get(&id).filter(|t| t.user == me).cloned().ok_or(Error::NotFound("令牌"))?;
            tx.del_token(id)?;
            st.token_by_hash.remove(&t.hash);
            st.tokens.remove(&id);
            Ok(())
        })
    }

    /// The (non-banned) owner of a token secret.
    pub fn token_user(&self, secret: &str) -> Option<User> {
        if !secret.starts_with(TOKEN_PREFIX) {
            return None;
        }
        let hash = sha(secret);
        let (user, stale) = {
            let st = self.st.read();
            let t = st.token_by_hash.get(&hash).and_then(|id| st.tokens.get(id))?;
            let u = st.users.get(&t.user).filter(|u| !u.banned).cloned()?;
            (u, now() - t.last_used > TOUCH_EVERY)
        };
        if stale {
            let _ = self.mutate(|st, tx| {
                if let Some(id) = st.token_by_hash.get(&hash).copied() {
                    if let Some(t) = st.tokens.get_mut(&id) {
                        t.last_used = now();
                        tx.put_token(t)?;
                    }
                }
                Ok(())
            });
        }
        Some(user)
    }
}
