//! 站外资源: outside sources worth knowing about (other students' repos, netdisk collections,
//! accounts that share materials). Staff keep the list; everyone sees it. Only the link is
//! kept — nothing is fetched from it.

use super::{Hub, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

/// Checks and tidies a submitted link: a title, an http(s) address, an optional note.
fn checked(title: &str, url: &str, note: &str) -> Result<(String, String, String)> {
    let title = clean(title, 40);
    let url = url.trim().to_string();
    let note = clean(note, 200);
    if title.is_empty() {
        return Err(bad("请填写名称"));
    }
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 400
        && url.split_once("://").is_some_and(|(_, rest)| rest.split('/').next().is_some_and(|host| host.contains('.')))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '<' || c == '>');
    if !ok {
        return Err(bad("链接需要是 http:// 或 https:// 开头的完整网址"));
    }
    Ok((title, url, note))
}

impl Hub {
    /// Everything on the list, in display order.
    pub fn links(&self) -> Vec<Link> {
        let st = self.st.read();
        let mut v: Vec<Link> = st.links.values().cloned().collect();
        v.sort_by_key(|l| (l.sort, l.id));
        v
    }

    pub fn add_link(&self, actor: Viewer, title: &str, url: &str, note: &str, sort: u32) -> Result<Link> {
        let me = actor.at_least(Level::Reviewer)?.id;
        let (title, url, note) = checked(title, url, note)?;
        self.mutate(|st, tx| {
            let l = Link { id: st.next_id(tx)?, title, url, note, sort, created_by: me, created_at: now() };
            tx.put_link(&l)?;
            st.links.insert(l.id, l.clone());
            Ok(l)
        })
    }

    pub fn update_link(&self, actor: Viewer, id: Id, title: &str, url: &str, note: &str, sort: u32) -> Result<Link> {
        actor.at_least(Level::Reviewer)?;
        let (title, url, note) = checked(title, url, note)?;
        self.mutate(|st, tx| {
            let mut l = st.links.get(&id).cloned().ok_or(Error::NotFound("链接"))?;
            l.title = title;
            l.url = url;
            l.note = note;
            l.sort = sort;
            tx.put_link(&l)?;
            st.links.insert(id, l.clone());
            Ok(l)
        })
    }

    /// Takes a link off the list (only the list entry; nothing else is involved).
    pub fn delete_link(&self, actor: Viewer, id: Id) -> Result<()> {
        actor.at_least(Level::Reviewer)?;
        self.mutate(|st, tx| {
            st.links.remove(&id).ok_or(Error::NotFound("链接"))?;
            tx.del_link(id)
        })
    }
}
