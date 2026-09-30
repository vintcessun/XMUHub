//! 公告 in 站内提醒: admins post notices that sit at the top of the 提醒 page and reach every
//! account as an unread reminder — nothing pops up. Kept as one JSON list in the meta table
//! (a handful of short texts), so no table of their own.

use serde::{Deserialize, Serialize};

use super::{Hub, Viewer};
use crate::error::{Error, Result, bad};
use crate::model::*;

pub(super) const BULLETINS_KEY: &str = "bulletins";
const TEXT_MAX: usize = 1000;
/// How much of the text the reminder in each account's list shows.
const PREVIEW: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bulletin {
    pub id: Id,
    pub text: String,
    pub at: i64,
}

impl Hub {
    /// Every 公告, newest first.
    pub fn bulletins(&self) -> Vec<Bulletin> {
        self.st.read().bulletins.iter().rev().cloned().collect()
    }

    /// Admins post a 公告; every account that can sign in gets a reminder pointing at it.
    pub fn post_bulletin(&self, viewer: Viewer, text_in: &str) -> Result<Bulletin> {
        viewer.at_least(Level::Admin)?;
        let text: String = text_in.trim().chars().filter(|c| *c == '\n' || !c.is_control()).take(TEXT_MAX).collect();
        if text.is_empty() {
            return Err(bad("公告内容不能为空"));
        }
        self.mutate(|st, tx| {
            let b = Bulletin { id: st.next_id(tx)?, text: text.clone(), at: now() };
            let mut list = st.bulletins.clone();
            list.push(b.clone());
            tx.put_meta(BULLETINS_KEY, &serde_json::to_vec(&list).map_err(|e| Error::Internal(e.to_string()))?)?;
            st.bulletins = list;
            let first_line = text.lines().next().unwrap_or_default();
            let mut preview: String = first_line.chars().take(PREVIEW).collect();
            if preview.chars().count() < text.chars().count() {
                preview.push('…');
            }
            let users: Vec<Id> = st.users.values().filter(|u| !u.banned && u.email != super::accounts::SYSTEM_EMAIL).map(|u| u.id).collect();
            for u in users {
                st.notify(tx, u, "bulletin", None, format!("公告：{preview}"), format!("/notices#b{}", b.id))?;
            }
            Ok(b)
        })
    }

    /// Admins take a 公告 down (reminders already sent stay until they expire).
    pub fn delete_bulletin(&self, viewer: Viewer, id: Id) -> Result<()> {
        viewer.at_least(Level::Admin)?;
        self.mutate(|st, tx| {
            let list: Vec<Bulletin> = st.bulletins.iter().filter(|b| b.id != id).cloned().collect();
            if list.len() == st.bulletins.len() {
                return Err(Error::NotFound("公告"));
            }
            tx.put_meta(BULLETINS_KEY, &serde_json::to_vec(&list).map_err(|e| Error::Internal(e.to_string()))?)?;
            st.bulletins = list;
            Ok(())
        })
    }
}
