//! 收藏夹: named lists of files a user keeps. Private by default; the owner may ask to share
//! one, and it becomes public once a reviewer (not the owner) approves its title and note.
//! Changing a shared list's title or note sends it back to review. Only public files are listed
//! to others; the owner sees which of theirs were taken down.

use serde::Serialize;

use super::wants::Person;
use super::{Hub, State, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

const LISTS_PER_USER: usize = 20;
const ITEMS_PER_LIST: usize = 500;
const TITLE_MAX: usize = 40;
const NOTE_MAX: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct CollectionView {
    pub id: Id,
    pub title: String,
    pub note: String,
    pub status: String,
    /// Why sharing was turned down (owner and staff only).
    pub review_note: String,
    pub owner: Person,
    /// Files listed to this viewer.
    pub count: usize,
    pub updated_at: i64,
    pub mine: bool,
}

/// A file on a list: its record, or `None` when it is no longer public.
pub type CollectionItem = (Id, Option<(Resource, Node)>);

impl Hub {
    fn collection_view(&self, st: &State, viewer: Viewer, c: &Collection) -> CollectionView {
        let mine = viewer.id() == Some(c.user);
        let public = |id: &Id| st.resources.get(id).is_some_and(|r| r.status == Status::Published);
        CollectionView {
            id: c.id,
            title: c.title.clone(),
            note: c.note.clone(),
            status: c.status.clone(),
            review_note: if mine || viewer.staff() { c.review_note.clone() } else { String::new() },
            owner: self.person(st, c.user),
            count: if mine { c.items.len() } else { c.items.iter().filter(|id| public(id)).count() },
            updated_at: c.updated_at,
            mine,
        }
    }

    fn own_collection(st: &State, me: Id, id: Id) -> Result<&Collection> {
        let c = st.collections.get(&id).ok_or(Error::NotFound("收藏夹"))?;
        if c.user != me {
            return Err(Error::Forbidden);
        }
        Ok(c)
    }

    fn checked_text(title: &str, note: &str) -> Result<(String, String)> {
        let title = clean(title, TITLE_MAX);
        if title.is_empty() {
            return Err(bad("请给收藏夹起个名字"));
        }
        Ok((title, clean(note, NOTE_MAX)))
    }

    /// The viewer's own lists, most recently changed first.
    pub fn my_collections(&self, viewer: Viewer) -> Result<Vec<CollectionView>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<&Collection> = st.collections.values().filter(|c| c.user == me).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        Ok(v.into_iter().map(|c| self.collection_view(&st, viewer, c)).collect())
    }

    /// Shared lists everyone can see, most recently changed first.
    pub fn public_collections(&self, viewer: Viewer, limit: usize) -> Vec<CollectionView> {
        let st = self.st.read();
        let mut v: Vec<&Collection> = st.collections.values().filter(|c| c.status == "public").collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        v.into_iter().take(limit).map(|c| self.collection_view(&st, viewer, c)).collect()
    }

    /// One list with its files: for its owner, staff, or anyone once it's public.
    pub fn collection(&self, viewer: Viewer, id: Id) -> Result<(CollectionView, Vec<CollectionItem>)> {
        let st = self.st.read();
        let c = st.collections.get(&id).ok_or(Error::NotFound("收藏夹"))?;
        let mine = viewer.id() == Some(c.user);
        if !(mine || viewer.staff() || c.status == "public") {
            return Err(Error::NotFound("收藏夹"));
        }
        let items = c
            .items
            .iter()
            .map(|rid| {
                let hit = st.resources.get(rid).filter(|r| r.status == Status::Published).and_then(|r| st.nodes.get(&r.node).map(|n| (r.clone(), n.clone())));
                (*rid, hit)
            })
            .filter(|(_, hit)| mine || hit.is_some())
            .collect();
        Ok((self.collection_view(&st, viewer, c), items))
    }

    pub fn create_collection(&self, viewer: Viewer, title: &str, note: &str) -> Result<CollectionView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let (title, note) = Self::checked_text(title, note)?;
        let c = self.mutate(|st, tx| {
            if st.collections.values().filter(|c| c.user == me).count() >= LISTS_PER_USER {
                return Err(Error::TooMany(format!("最多建 {LISTS_PER_USER} 个收藏夹")));
            }
            let t = now();
            let c = Collection { id: st.next_id(tx)?, user: me, title, note, items: Vec::new(), created_at: t, updated_at: t, status: "private".into(), reviewed_by: None, review_note: String::new() };
            tx.put_collection(&c)?;
            st.collections.insert(c.id, c.clone());
            Ok(c)
        })?;
        Ok(self.collection_view(&self.st.read(), viewer, &c))
    }

    /// Renames a list or changes its note. A shared (or waiting) list goes back to review.
    pub fn update_collection(&self, viewer: Viewer, id: Id, title: &str, note: &str) -> Result<CollectionView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let (title, note) = Self::checked_text(title, note)?;
        let c = self.mutate(|st, tx| {
            let mut c = Self::own_collection(st, me, id)?.clone();
            if c.title == title && c.note == note {
                return Ok(c);
            }
            c.title = title;
            c.note = note;
            c.updated_at = now();
            c.status = match c.status.as_str() {
                "public" | "pending" => "pending".into(),
                _ => "private".into(),
            };
            c.review_note.clear();
            tx.put_collection(&c)?;
            st.collections.insert(id, c.clone());
            Ok(c)
        })?;
        Ok(self.collection_view(&self.st.read(), viewer, &c))
    }

    /// Deletes a list (only the list: the files stay where they are).
    pub fn delete_collection(&self, viewer: Viewer, id: Id) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            Self::own_collection(st, me, id)?;
            st.collections.remove(&id);
            tx.del_collection(id)
        })
    }

    /// Puts a public file on one of the viewer's lists, or takes it off.
    pub fn collect(&self, viewer: Viewer, id: Id, resource: Id, on: bool) -> Result<bool> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let mut c = Self::own_collection(st, me, id)?.clone();
            let has = c.items.contains(&resource);
            if on == has {
                return Ok(on);
            }
            if on {
                if !st.resources.get(&resource).is_some_and(|r| r.status == Status::Published) {
                    return Err(Error::NotFound("资料"));
                }
                if c.items.len() >= ITEMS_PER_LIST {
                    return Err(Error::TooMany(format!("一个收藏夹最多放 {ITEMS_PER_LIST} 份资料")));
                }
                c.items.push(resource);
            } else {
                c.items.retain(|x| *x != resource);
            }
            c.updated_at = now();
            tx.put_collection(&c)?;
            st.collections.insert(id, c);
            Ok(on)
        })
    }

    /// Which of the viewer's lists hold a file.
    pub fn collections_with(&self, viewer: Viewer, resource: Id) -> Result<Vec<Id>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        Ok(st.collections.values().filter(|c| c.user == me && c.items.contains(&resource)).map(|c| c.id).collect())
    }

    /// Asks to share a list (it waits for a reviewer), or makes it private again.
    pub fn share_collection(&self, viewer: Viewer, id: Id, on: bool) -> Result<CollectionView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let c = self.mutate(|st, tx| {
            let mut c = Self::own_collection(st, me, id)?.clone();
            if on {
                if c.status == "public" || c.status == "pending" {
                    return Ok(c);
                }
                if c.items.is_empty() {
                    return Err(bad("先放几份资料再分享"));
                }
                if st.collections.values().filter(|x| x.status == "pending").count() >= 200 {
                    return Err(Error::TooMany("待审核的收藏夹太多了，请稍后再试".into()));
                }
                // Admins share straight away; everyone else waits for a reviewer.
                c.status = if viewer.exempt() { "public" } else { "pending" }.into();
            } else {
                c.status = "private".into();
            }
            c.review_note.clear();
            tx.put_collection(&c)?;
            st.collections.insert(id, c.clone());
            Ok(c)
        })?;
        Ok(self.collection_view(&self.st.read(), viewer, &c))
    }

    /// Staff: lists waiting to be shared, oldest first.
    pub fn pending_collections(&self, viewer: Viewer) -> Result<Vec<CollectionView>> {
        viewer.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<&Collection> = st.collections.values().filter(|c| c.status == "pending").collect();
        v.sort_by_key(|c| c.updated_at);
        Ok(v.into_iter().map(|c| self.collection_view(&st, viewer, c)).collect())
    }

    /// A reviewer other than the owner lets a list be shared, or turns it down with a reason.
    pub fn review_collection(&self, viewer: Viewer, id: Id, approve: bool, note: &str) -> Result<()> {
        let me = viewer.at_least(Level::Reviewer)?.id;
        let reason = clean(note, 200);
        if !approve && reason.is_empty() {
            return Err(bad("请写明不通过的原因"));
        }
        self.mutate(|st, tx| {
            let mut c = st.collections.get(&id).cloned().ok_or(Error::NotFound("收藏夹"))?;
            if c.status != "pending" {
                return Err(Error::Conflict("这个收藏夹不在等待审核".into()));
            }
            if c.user == me && !viewer.exempt() {
                return Err(bad("自己的收藏夹要由其他审核员审核"));
            }
            c.status = if approve { "public" } else { "rejected" }.into();
            c.reviewed_by = Some(me);
            c.review_note = if approve { String::new() } else { reason.clone() };
            tx.put_collection(&c)?;
            let text = if approve {
                format!("你的收藏夹「{}」已公开分享", c.title)
            } else {
                format!("你的收藏夹「{}」没有通过分享审核：{reason}", c.title)
            };
            let (user, link) = (c.user, format!("/collections?id={}", c.id));
            st.collections.insert(id, c);
            st.notify(tx, user, "collection", None, text, link)
        })
    }
}
