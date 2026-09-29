//! 合集: files of one course that belong together, in a set order (第1讲…第12讲, 2019–2024
//! 期末…). The course page shows them as one row, the file page as 「第 3 / 12 份」 with the
//! neighbours, like a video 合集.
//!
//! Anyone signed in may propose a new 合集 or a change to one (including 解散: no files).
//! A proposal waits in `draft` until a reviewer who didn't write it approves it; until then
//! visitors keep seeing the approved version. A file is in at most one 合集.

use serde::Serialize;

use super::wants::Person;
use super::{Hub, State, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

const TITLE_MAX: usize = 40;
const ITEMS_MAX: usize = 300;
const DRAFTS_PER_USER: usize = 20;
const DRAFTS_MAX: usize = 300;

/// One file of a 合集.
#[derive(Debug, Clone, Serialize)]
pub struct SeriesItem {
    pub id: Id,
    pub title: String,
    pub ext: String,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesDraftView {
    pub by: Person,
    pub mine: bool,
    pub title: String,
    pub items: Vec<SeriesItem>,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesView {
    pub id: Id,
    pub node: Id,
    pub node_name: String,
    /// "new" (waiting for its first approval), "public", "closed" (解散) or "rejected".
    pub status: String,
    pub title: String,
    /// The approved files that are public and still in this course, in order.
    pub items: Vec<SeriesItem>,
    pub updated_at: i64,
    /// The change waiting for review: for its author and staff only.
    pub draft: Option<SeriesDraftView>,
    /// Why the viewer's last proposal was turned down.
    pub review_note: String,
}

impl Hub {
    fn series_item(st: &State, id: Id) -> Option<SeriesItem> {
        st.resources.get(&id).map(|r| SeriesItem { id, title: r.name.stem(), ext: r.ext.clone(), status: r.status.as_str() })
    }

    fn series_view(&self, st: &State, viewer: Viewer, s: &Series) -> SeriesView {
        let me = viewer.id();
        let live = s
            .items
            .iter()
            .filter(|id| st.resources.get(id).is_some_and(|r| r.status == Status::Published && r.node == s.node))
            .filter_map(|id| Self::series_item(st, *id))
            .collect();
        let draft = s.draft.as_ref().filter(|d| viewer.staff() || me == Some(d.by)).map(|d| SeriesDraftView {
            by: self.person(st, d.by),
            mine: me == Some(d.by),
            title: d.title.clone(),
            items: d.items.iter().filter_map(|id| Self::series_item(st, *id)).collect(),
            at: d.at,
        });
        SeriesView {
            id: s.id,
            node: s.node,
            node_name: st.nodes.get(&s.node).map(|n| n.name.clone()).unwrap_or_default(),
            status: s.status.clone(),
            title: s.title.clone(),
            items: live,
            updated_at: s.updated_at,
            draft,
            review_note: if viewer.staff() || me == Some(s.created_by) { s.review_note.clone() } else { String::new() },
        }
    }

    /// The live 合集 of a course with at least two public files, oldest first.
    pub fn node_series(&self, viewer: Viewer, node: Id) -> Vec<SeriesView> {
        let st = self.st.read();
        let mut v: Vec<&Series> = st.series.values().filter(|s| s.node == node && s.status == "public").collect();
        v.sort_by_key(|s| s.id);
        v.into_iter().map(|s| self.series_view(&st, viewer, s)).filter(|s| s.items.len() >= 2).collect()
    }

    /// The live 合集 a file is in, if any.
    pub fn series_of(&self, viewer: Viewer, resource: Id) -> Option<SeriesView> {
        let st = self.st.read();
        let s = st.series.values().find(|s| s.status == "public" && s.items.contains(&resource))?;
        Some(self.series_view(&st, viewer, s)).filter(|v| v.items.iter().any(|i| i.id == resource))
    }

    /// One 合集: live ones for everyone; the author of a waiting proposal and staff see it too.
    pub fn series(&self, viewer: Viewer, id: Id) -> Result<SeriesView> {
        let st = self.st.read();
        let s = st.series.get(&id).ok_or(Error::NotFound("合集"))?;
        let v = self.series_view(&st, viewer, s);
        if s.status != "public" && v.draft.is_none() && !viewer.staff() && viewer.id() != Some(s.created_by) {
            return Err(Error::NotFound("合集"));
        }
        Ok(v)
    }

    /// Proposes a new 合集 (`id` None) or a change to one. No files (for an existing one) means
    /// 解散. The proposal waits for a reviewer other than its author.
    pub fn propose_series(&self, viewer: Viewer, id: Option<Id>, node: Id, title: &str, items: &[Id]) -> Result<SeriesView> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let title = clean(title, TITLE_MAX);
        let mut list: Vec<Id> = Vec::with_capacity(items.len().min(ITEMS_MAX));
        for i in items {
            if !list.contains(i) {
                list.push(*i);
            }
        }
        if list.len() > ITEMS_MAX {
            return Err(bad(format!("一个合集最多 {ITEMS_MAX} 份资料")));
        }
        let closing = list.is_empty() && id.is_some();
        if !closing {
            if list.len() < 2 {
                return Err(bad("合集至少要有两份资料"));
            }
            if title.is_empty() {
                return Err(bad("请给合集起个名字"));
            }
        }
        let s = self.mutate(|st, tx| {
            let mut s = match id {
                Some(id) => {
                    let s = st.series.get(&id).cloned().ok_or(Error::NotFound("合集"))?;
                    if s.status != "public" && s.created_by != me {
                        return Err(Error::NotFound("合集"));
                    }
                    if s.draft.as_ref().is_some_and(|d| d.by != me) {
                        return Err(Error::Conflict("这个合集已经有人提交了修改，等审核完再改".into()));
                    }
                    s
                }
                None => {
                    let n = st.nodes.get(&node).ok_or(Error::NotFound("分类"))?;
                    if n.kind == NodeKind::Section {
                        return Err(bad("合集要建在课程里"));
                    }
                    let t = now();
                    Series { id: 0, node, title: String::new(), items: Vec::new(), created_by: me, created_at: t, updated_at: t, status: "new".into(), draft: None, reviewed_by: None, review_note: String::new() }
                }
            };
            for rid in &list {
                let r = st.resources.get(rid).ok_or(Error::NotFound("资料"))?;
                if r.node != s.node {
                    return Err(bad(format!("「{}」不在这门课里", r.name.stem())));
                }
                let ok = r.status == Status::Published || (r.status == Status::Pending && (r.uploader == me || viewer.staff()));
                if !ok {
                    return Err(bad(format!("「{}」没有公开，不能放进合集", r.name.stem())));
                }
                if let Some(o) = st.series.values().find(|o| {
                    o.id != s.id && ((o.status == "public" && o.items.contains(rid)) || o.draft.as_ref().is_some_and(|d| d.items.contains(rid)))
                }) {
                    let name = if o.status == "public" { o.title.as_str() } else { o.draft.as_ref().map_or("", |d| d.title.as_str()) };
                    return Err(bad(format!("「{}」已经在合集「{name}」里了（一份资料只能在一个合集里）", r.name.stem())));
                }
            }
            if s.draft.is_none() {
                let mine = st.series.values().filter(|o| o.draft.as_ref().is_some_and(|d| d.by == me)).count();
                if mine >= DRAFTS_PER_USER {
                    return Err(Error::TooMany(format!("你已经有 {DRAFTS_PER_USER} 个合集在等审核了，等审核完再提交")));
                }
                if st.series.values().filter(|o| o.draft.is_some()).count() >= DRAFTS_MAX {
                    return Err(Error::TooMany("等待审核的合集太多了，请稍后再试".into()));
                }
            }
            if id.is_none() {
                s.id = st.next_id(tx)?;
            }
            let title = if closing { s.title.clone() } else { title };
            if s.status == "public" && s.title == title && s.items == list {
                s.draft = None;
            } else {
                s.draft = Some(SeriesDraft { by: me, title, items: list, at: now() });
            }
            if viewer.exempt()
                && let Some(d) = s.draft.take()
            {
                Self::apply_series(&mut s, d, me);
            }
            tx.put_series(&s)?;
            st.series.insert(s.id, s.clone());
            Ok(s)
        })?;
        Ok(self.series_view(&self.st.read(), viewer, &s))
    }

    fn apply_series(s: &mut Series, d: SeriesDraft, by: Id) {
        s.status = if d.items.is_empty() { "closed" } else { "public" }.into();
        s.title = d.title;
        s.items = d.items;
        s.updated_at = now();
        s.reviewed_by = Some(by);
        s.review_note.clear();
    }

    /// Staff: proposals waiting for review, oldest first.
    pub fn pending_series(&self, viewer: Viewer) -> Result<Vec<SeriesView>> {
        viewer.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<&Series> = st.series.values().filter(|s| s.draft.is_some()).collect();
        v.sort_by_key(|s| s.draft.as_ref().map_or(0, |d| d.at));
        Ok(v.into_iter().map(|s| self.series_view(&st, viewer, s)).collect())
    }

    /// A reviewer other than the proposal's author approves it, or turns it down with a reason.
    pub fn review_series(&self, viewer: Viewer, id: Id, approve: bool, note: &str) -> Result<()> {
        let me = viewer.at_least(Level::Reviewer)?.id;
        let reason = clean(note, 200);
        if !approve && reason.is_empty() {
            return Err(bad("请写明不通过的原因"));
        }
        self.mutate(|st, tx| {
            let mut s = st.series.get(&id).cloned().ok_or(Error::NotFound("合集"))?;
            let d = s.draft.take().ok_or_else(|| Error::Conflict("这个合集没有等待审核的修改".into()))?;
            if d.by == me && !viewer.exempt() {
                return Err(bad("自己提交的合集要由其他审核员审核"));
            }
            let (author, title) = (d.by, d.title.clone());
            let text = if approve {
                // Files taken down or moved since, or claimed by another 合集 meanwhile, drop out.
                let mut d = d;
                d.items.retain(|rid| {
                    st.resources.get(rid).is_some_and(|r| r.node == s.node && matches!(r.status, Status::Published | Status::Pending))
                        && !st.series.values().any(|o| o.id != id && o.status == "public" && o.items.contains(rid))
                });
                if !d.items.is_empty() && d.items.len() < 2 {
                    return Err(Error::Conflict("合集里能用的资料不到两份了，请驳回".into()));
                }
                let closing = d.items.is_empty();
                Self::apply_series(&mut s, d, me);
                if closing { format!("合集「{title}」已按你的申请解散") } else { format!("你提交的合集「{title}」已通过审核") }
            } else {
                if s.status == "new" {
                    s.status = "rejected".into();
                }
                s.reviewed_by = Some(me);
                s.review_note = reason.clone();
                format!("你提交的合集「{title}」没有通过审核：{reason}")
            };
            tx.put_series(&s)?;
            let link = match s.items.first() {
                Some(r) if s.status == "public" => format!("/r/{r}"),
                _ => format!("/n/{}", s.node),
            };
            st.series.insert(id, s);
            if author == me {
                return Ok(());
            }
            st.notify(tx, author, "series", None, text, link)
        })
    }
}
