//! 站外资源: outside sources worth knowing about (other students' repos, netdisk collections,
//! accounts that share materials). Everyone sees the list. Signed-in students suggest links;
//! a reviewer other than the one who suggested it adds it or turns it down; staff can also
//! edit and remove entries. Only the link is kept — nothing is fetched from it.

use serde::Serialize;

use super::wants::Person;
use super::{Hub, State, Viewer, clean};
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

/// Suggestions one person may have waiting at once, and make in a day.
const PENDING_PER_USER: usize = 3;
const PER_DAY: usize = 5;
/// Waiting suggestions site-wide, so the queue can't be flooded.
const PENDING_MAX: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct LinkSuggestionView {
    pub id: Id,
    pub title: String,
    pub url: String,
    pub note: String,
    pub created_at: i64,
    pub status: String,
    pub review_note: String,
    pub by: Person,
    /// The viewer suggested it (a reviewer can't approve their own).
    pub mine: bool,
}

impl Hub {
    fn suggestion_view(&self, st: &State, viewer: Viewer, s: &LinkSuggestion) -> LinkSuggestionView {
        LinkSuggestionView {
            id: s.id,
            title: s.title.clone(),
            url: s.url.clone(),
            note: s.note.clone(),
            created_at: s.created_at,
            status: s.status.clone(),
            review_note: s.review_note.clone(),
            by: self.person(st, s.user),
            mine: viewer.user.is_some_and(|u| u.id == s.user),
        }
    }

    /// Suggests a link for the list; it waits for a reviewer.
    pub fn suggest_link(&self, viewer: Viewer, title: &str, url: &str, note: &str) -> Result<LinkSuggestionView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let (title, url, note) = checked(title, url, note)?;
        let t = now();
        {
            let st = self.st.read();
            let own: Vec<&LinkSuggestion> = st.link_suggestions.values().filter(|s| s.user == me).collect();
            if own.iter().filter(|s| s.status == "pending").count() >= PENDING_PER_USER {
                return Err(Error::TooMany(format!("你已经有 {PENDING_PER_USER} 个推荐在等审核，审核后再推荐")));
            }
            if own.iter().filter(|s| t - s.created_at < 86400).count() >= PER_DAY {
                return Err(Error::TooMany("今天推荐的已达上限".into()));
            }
            if st.link_suggestions.values().filter(|s| s.status == "pending").count() >= PENDING_MAX {
                return Err(Error::TooMany("待审核的推荐太多了，请稍后再来".into()));
            }
            let same = |u: &str| u.trim_end_matches('/').eq_ignore_ascii_case(url.trim_end_matches('/'));
            if st.links.values().any(|l| same(&l.url)) {
                return Err(Error::Conflict("列表里已经有这个链接了".into()));
            }
            if st.link_suggestions.values().any(|s| s.status == "pending" && same(&s.url)) {
                return Err(Error::Conflict("这个链接已经有人推荐了，正在等审核".into()));
            }
        }
        let s = self.mutate(|st, tx| {
            let s = LinkSuggestion {
                id: st.next_id(tx)?,
                user: me,
                title,
                url,
                note,
                created_at: t,
                status: "pending".into(),
                reviewed_by: None,
                review_note: String::new(),
                link: None,
            };
            tx.put_link_suggestion(&s)?;
            st.link_suggestions.insert(s.id, s.clone());
            Ok(s)
        })?;
        Ok(self.suggestion_view(&self.st.read(), viewer, &s))
    }

    /// Staff: the suggestions waiting for review, oldest first. Anyone else: their own, newest first.
    pub fn link_suggestions(&self, viewer: Viewer) -> Result<Vec<LinkSuggestionView>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<&LinkSuggestion> = if viewer.staff() {
            st.link_suggestions.values().filter(|s| s.status == "pending").collect()
        } else {
            st.link_suggestions.values().filter(|s| s.user == me).collect()
        };
        v.sort_by_key(|s| s.id);
        if !viewer.staff() {
            v.reverse();
        }
        Ok(v.into_iter().map(|s| self.suggestion_view(&st, viewer, s)).collect())
    }

    /// A reviewer adds a suggestion to the list (title / note as given, possibly tidied by the
    /// reviewer) or turns it down with a reason. Nobody reviews their own suggestion.
    pub fn review_link_suggestion(&self, viewer: Viewer, id: Id, approve: bool, note: &str, edit: Option<(&str, &str, &str, u32)>) -> Result<Option<Link>> {
        let me = viewer.at_least(Level::Reviewer)?.id;
        let reason = clean(note, 200);
        if !approve && reason.is_empty() {
            return Err(bad("请写明不采纳的原因"));
        }
        let fixed = match edit {
            Some((title, url, note, sort)) => Some((checked(title, url, note)?, sort)),
            None => None,
        };
        self.mutate(|st, tx| {
            let mut s = st.link_suggestions.get(&id).cloned().ok_or(Error::NotFound("推荐"))?;
            if s.status != "pending" {
                return Err(Error::Conflict("这个推荐已经处理过了".into()));
            }
            if s.user == me {
                return Err(bad("自己推荐的链接要由其他审核员审核"));
            }
            let mut added = None;
            if approve {
                let ((title, url, note), sort) = fixed.unwrap_or(((s.title.clone(), s.url.clone(), s.note.clone()), 100));
                let l = Link { id: st.next_id(tx)?, title, url, note, sort, created_by: s.user, created_at: now() };
                tx.put_link(&l)?;
                st.links.insert(l.id, l.clone());
                s.link = Some(l.id);
                added = Some(l);
            }
            s.status = if approve { "approved" } else { "rejected" }.into();
            s.reviewed_by = Some(me);
            s.review_note = if approve { String::new() } else { reason.clone() };
            tx.put_link_suggestion(&s)?;
            let text = if approve { format!("你推荐的「{}」已加入站外资源", s.title) } else { format!("你推荐的「{}」没有被采纳：{reason}", s.title) };
            let user = s.user;
            st.link_suggestions.insert(id, s);
            st.notify(tx, user, "link", None, text, "/links".into())?;
            Ok(added)
        })
    }

    /// Everything on the list, in display order.
    pub fn links(&self) -> Vec<Link> {
        let st = self.st.read();
        let mut v: Vec<Link> = st.links.values().cloned().collect();
        v.sort_by_key(|l| (l.sort, l.id));
        v
    }

    /// Straight onto the list: only the owner's import account. Everyone else, staff too,
    /// suggests a link for another reviewer.
    pub fn add_link(&self, actor: Viewer, title: &str, url: &str, note: &str, sort: u32) -> Result<Link> {
        let me = actor.at_least(Level::Reviewer)?.id;
        if !actor.system() {
            return Err(bad("新链接请在「推荐一个」里提交，由其他审核员审核"));
        }
        let (title, url, note) = checked(title, url, note)?;
        self.mutate(|st, tx| {
            let l = Link { id: st.next_id(tx)?, title, url, note, sort, created_by: me, created_at: now() };
            tx.put_link(&l)?;
            st.links.insert(l.id, l.clone());
            Ok(l)
        })
    }

    /// Staff fix an entry's name, note and order; a different address is a new link to suggest.
    pub fn update_link(&self, actor: Viewer, id: Id, title: &str, url: &str, note: &str, sort: u32) -> Result<Link> {
        actor.at_least(Level::Reviewer)?;
        let (title, url, note) = checked(title, url, note)?;
        self.mutate(|st, tx| {
            let mut l = st.links.get(&id).cloned().ok_or(Error::NotFound("链接"))?;
            if l.url != url && !actor.system() {
                return Err(bad("换链接地址请重新推荐，由其他审核员审核"));
            }
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
