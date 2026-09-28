//! Resource metadata, generated file names, review, reports and downloads.

use serde::{Deserialize, Serialize};

use super::{Hub, State, Viewer, clean};
use crate::db::Tx;
use crate::error::{Error, Result, bad};
use crate::model::*;

/// What an uploader chooses; the file name is generated from it (不自己起名，只做选择).
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceInput {
    pub node: Id,
    /// Course segment override; defaults to the node's label.
    #[serde(default)]
    pub course: String,
    #[serde(default)]
    pub time: String,
    pub type_word: String,
    /// Tag code ("T1"…); derived from the type word when omitted.
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub paper: String,
    #[serde(default)]
    pub with_answer: bool,
    #[serde(default)]
    pub extra: String,
    #[serde(default)]
    pub note: String,
    /// Public subtitle describing the content; `None` keeps the current / automatic one.
    #[serde(default)]
    pub subtitle: Option<String>,
}

/// Fields only staff (and the importer) may set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdminExtras {
    pub status: Option<String>,
    #[serde(default)]
    pub original_name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub uncertain: bool,
    /// Keep this exact version number instead of auto-numbering duplicates.
    pub version: Option<u16>,
    /// Verbatim type word outside the closed set (archive names).
    #[serde(default)]
    pub free_type: bool,
    /// Keep a second resource with byte-identical content in the same node (archive imports
    /// where the same file was filed under two names). Storage still holds one copy.
    #[serde(default)]
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadPlan {
    pub filename: String,
    pub size: u64,
    pub mime: String,
    pub parts: Vec<DownloadPart>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadPart {
    pub size: u64,
    pub sha256: String,
    pub urls: Vec<String>,
}

pub const PAPERS: &[&str] = &["", "A卷", "B卷", "C卷"];

fn seg(s: &str, max: usize) -> String {
    clean(s, max).chars().filter(|c| !"_/\\()（）【】[] 　·".contains(*c)).collect()
}

/// "2023-2024秋", "2025春", "2023", "2019-2023", "202406" or empty.
fn valid_time(t: &str) -> bool {
    if t.is_empty() {
        return true;
    }
    let body = t.trim_end_matches(['秋', '春', '暑']);
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    match body.split_once('-') {
        Some((a, b)) => digits(a, 4) && digits(b, 4),
        None => digits(body, 4) || (digits(body, 6) && body.len() == t.len()),
    }
}

impl Hub {
    pub(super) fn build_name(st: &State, input: &ResourceInput, extras: &AdminExtras, staff: bool, exclude: Option<Id>) -> Result<(NameParts, Tag)> {
        let node = st.resolve(input.node).ok_or(Error::NotFound("分类"))?;
        if node.kind == NodeKind::Section {
            return Err(bad("请选择具体的课程或分类"));
        }
        let course = if input.course.trim().is_empty() { node.label.clone() } else { seg(&input.course, 40) };
        if course.is_empty() {
            return Err(bad("请填写课程名"));
        }
        let time = seg(&input.time, 20);
        if !staff && !valid_time(&time) {
            return Err(bad("时间格式应为 2023-2024秋 / 2025春 / 2023 / 202406"));
        }
        let type_word = seg(&input.type_word, 12);
        let implied = type_word_tag(&type_word);
        if implied.is_none() && !(staff && extras.free_type && !type_word.is_empty()) {
            return Err(bad("请选择资料类型"));
        }
        let paper = seg(&input.paper, 6);
        if !staff && !PAPERS.contains(&paper.as_str()) {
            return Err(bad("卷别不合法"));
        }
        let tag = match input.tag.as_str() {
            "" => implied.unwrap_or(Tag::Slides),
            code => Tag::parse(code).ok_or_else(|| bad("未知的资料标签"))?,
        };
        let mut name = NameParts {
            course,
            time,
            type_word,
            paper,
            with_answer: input.with_answer,
            extra: seg(&input.extra, 20),
            version: 1,
        };
        // Same 课程+时间+类型 in the same node → _v2, _v3 … (分类规则 §4.4).
        let base = name.base();
        let taken: Vec<u16> = st
            .by_node
            .get(&node.id)
            .into_iter()
            .flatten()
            .filter(|id| Some(**id) != exclude)
            .map(|id| &st.resources[id])
            .filter(|r| r.name.base() == base && !matches!(r.status, Status::Rejected))
            .map(|r| r.name.version)
            .collect();
        name.version = match extras.version.filter(|_| staff) {
            Some(v) => v.max(1),
            None if taken.is_empty() => 1,
            None => taken.iter().max().copied().unwrap_or(1) + 1,
        };
        if name.stem().chars().count() > 80 {
            return Err(bad("生成的文件名超过 80 字，请精简补充说明"));
        }
        Ok((name, tag))
    }

    /// Previews the generated file name without saving anything.
    pub fn preview_name(&self, input: &ResourceInput, ext: &str) -> Result<String> {
        let st = self.st.read();
        let (name, _) = Self::build_name(&st, input, &AdminExtras::default(), false, None)?;
        Ok(if ext.is_empty() { name.stem() } else { format!("{}.{}", name.stem(), ext) })
    }

    pub fn create_resource(&self, actor: Viewer, upload_id: Id, input: ResourceInput, extras: AdminExtras) -> Result<Resource> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let staff = me.level >= Level::Reviewer;
        let status_override = match extras.status.as_deref() {
            Some(s) if staff => Some(Status::parse(s).ok_or_else(|| bad("未知状态"))?),
            _ => None,
        };
        let r = self.mutate(|st, tx| {
            let mut up = st.uploads.get(&upload_id).cloned().ok_or(Error::NotFound("上传记录"))?;
            if up.user != me.id {
                return Err(Error::Forbidden);
            }
            if !up.finished {
                return Err(Error::Conflict("文件还没有上传完成".into()));
            }
            if up.consumed {
                return Err(Error::Conflict("这个上传已经提交过了".into()));
            }
            let (name, tag) = Self::build_name(st, &input, &extras, staff, None)?;
            let node = st.resolve(input.node).map(|n| n.id).ok_or(Error::NotFound("分类"))?;
            let dup = st.by_node.get(&node).into_iter().flatten().any(|id| {
                let r = &st.resources[id];
                r.blob == up.key && !matches!(r.status, Status::Rejected)
            });
            if dup && !(staff && extras.allow_duplicate) {
                return Err(Error::Conflict("这个分类下已经有完全相同的文件了".into()));
            }
            let ext = up
                .filename
                .rsplit_once('.')
                .map(|(_, e)| e.to_lowercase())
                .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or_default();
            let status = status_override.unwrap_or(if me.level.publishes_directly() { Status::Published } else { Status::Pending });
            let r = Resource {
                id: st.next_id(tx)?,
                node,
                tag,
                name,
                ext,
                note: clean(&input.note, 500),
                original_name: if staff && !extras.original_name.is_empty() { clean(&extras.original_name, 200) } else { up.filename.clone() },
                source: if staff { clean(&extras.source, 200) } else { String::new() },
                uncertain: staff && extras.uncertain,
                blob: up.key.clone(),
                size: up.size,
                mime: up.mime.clone(),
                status,
                needs_review: me.level == Level::Trusted,
                review_note: String::new(),
                uploader: me.id,
                reviewed_by: None,
                created_at: now(),
                updated_at: now(),
                downloads: 0,
            };
            if let Some(sub) = &input.subtitle {
                Self::set_subtitle(st, tx, &r, sub)?;
            }
            st.put_resource(tx, r.clone())?;
            up.consumed = true;
            tx.put_upload(&up)?;
            st.uploads.insert(up.id, up);
            let mut u = st.users[&me.id].clone();
            u.uploads += 1;
            st.put_user(tx, u)?;
            Ok(r)
        })?;
        self.reindex(&[], &[r.id]);
        Ok(r)
    }

    pub fn update_resource(&self, actor: Viewer, id: Id, input: ResourceInput, extras: AdminExtras) -> Result<Resource> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let staff = me.level >= Level::Reviewer;
        let r = self.mutate(|st, tx| {
            let mut r = st.resources.get(&id).cloned().ok_or(Error::NotFound("资料"))?;
            // Uploaders may fix their own files while waiting and after publication (the edit
            // then goes back to review below). Rejected files no longer have their blob.
            let own = r.uploader == me.id && matches!(r.status, Status::Pending | Status::Published);
            if !staff && !own {
                return Err(Error::Forbidden);
            }
            if !staff && clean(&input.note, 500) != r.note {
                return Err(bad("修改备注需要提交申请，由审核员同意后生效"));
            }
            let (mut name, tag) = Self::build_name(st, &input, &extras, staff, Some(id))?;
            let target = st.resolve(input.node).ok_or(Error::NotFound("分类"))?;
            if !staff && target.kind == NodeKind::Section {
                return Err(bad("请选择具体的课程或分类，不能直接放在栏目下"));
            }
            let node = target.id;
            let old_node = r.node;
            // Keep the existing version when the base name didn't change.
            if name.base() == r.name.base() && node == r.node && extras.version.is_none() {
                name.version = r.name.version;
            }
            r.node = node;
            r.name = name;
            r.tag = tag;
            r.note = clean(&input.note, 500);
            if staff {
                r.uncertain = extras.uncertain;
            }
            r.updated_at = now();
            if let Some(sub) = &input.subtitle {
                Self::set_subtitle(st, tx, &r, sub)?;
            }
            if !staff {
                // An edited published file is checked again, by the same rule as a new upload:
                // trusted uploaders stay public and are re-checked, others wait for approval.
                if r.status == Status::Published {
                    if me.level.publishes_directly() {
                        r.needs_review = true;
                    } else {
                        r.status = Status::Pending;
                    }
                }
                let e = ReviewEvent { id: st.next_id(tx)?, resource: r.id, actor: me.id, action: "edit".into(), note: String::new(), at: now() };
                tx.put_review(&e)?;
                st.reviews.push(e);
            }
            st.put_resource(tx, r.clone())?;
            Ok((r, old_node))
        })?;
        let (r, old_node) = r;
        self.reindex(&[old_node, r.node], &[r.id]);
        Ok(r)
    }

    /// Stores a hand-edited subtitle. Matching the automatic one stores nothing, so later
    /// improvements to the automatic rule still apply.
    fn set_subtitle(st: &mut State, tx: &Tx, r: &Resource, sub: &str) -> Result<()> {
        let sub = clean(sub, 80);
        if sub == crate::text::auto_subtitle(&r.original_name, &r.name.stem()) && !st.subtitles.contains_key(&r.id) {
            return Ok(());
        }
        tx.put_subtitle(r.id, &sub)?;
        st.subtitles.insert(r.id, sub);
        Ok(())
    }

    pub fn subtitle(&self, r: &Resource) -> String {
        self.st.read().subtitle(r)
    }

    /// Moves resources to another node (批量修改所属目录). A name whose course segment was
    /// the old node's label takes the new node's label; versions are renumbered as needed.
    pub fn move_resources(&self, actor: Viewer, ids: &[Id], to: Id) -> Result<usize> {
        actor.at_least(Level::Reviewer)?;
        if ids.len() > 500 {
            return Err(bad("一次最多移动 500 份"));
        }
        let moved = self.mutate(|st, tx| {
            let target = st.resolve(to).cloned().ok_or(Error::NotFound("目标分类"))?;
            if target.kind == NodeKind::Section {
                return Err(bad("请选择具体的课程或分类，不能直接放在栏目下"));
            }
            let mut moved = Vec::new();
            for id in ids {
                let Some(mut r) = st.resources.get(id).cloned() else { continue };
                if r.node == target.id {
                    continue;
                }
                let old_label = st.nodes.get(&r.node).map(|n| n.label.clone()).unwrap_or_default();
                let mut name = r.name.clone();
                if name.course == old_label || name.course.is_empty() {
                    name.course = target.label.clone();
                }
                // Lowest free version for this base name in the target node (moving back restores v1).
                let base = name.base();
                let taken: Vec<u16> = st
                    .by_node
                    .get(&target.id)
                    .into_iter()
                    .flatten()
                    .map(|x| &st.resources[x])
                    .filter(|x| x.name.base() == base && !matches!(x.status, Status::Rejected))
                    .map(|x| x.name.version)
                    .collect();
                name.version = (1..).find(|v| !taken.contains(v)).unwrap_or(1);
                let old_node = r.node;
                r.name = name;
                r.node = target.id;
                r.updated_at = now();
                st.put_resource(tx, r)?;
                moved.push((*id, old_node));
            }
            Ok(moved)
        })?;
        let nodes: Vec<Id> = moved.iter().map(|(_, n)| *n).chain(std::iter::once(to)).collect();
        let ids: Vec<Id> = moved.iter().map(|(i, _)| *i).collect();
        self.reindex(&nodes, &ids);
        Ok(ids.len())
    }

    /// approve / reject / remove / restore / restrict. Returns storage locations that became
    /// unreferenced for the caller to delete (network I/O stays outside the write lock).
    pub fn review(&self, actor: Viewer, id: Id, action: &str, note: &str) -> Result<(Resource, Vec<Location>)> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        let (r, garbage) = self.mutate(|st, tx| {
            let mut r = st.resources.get(&id).cloned().ok_or(Error::NotFound("资料"))?;
            match action {
                "approve" | "restore" => {
                    // A removed or rejected file has been deleted from storage; there is nothing to publish.
                    if !st.blobs.contains_key(&r.blob) { return Err(bad("文件已经删除，无法恢复；请重新上传")); }
                    r.status = Status::Published;
                    r.needs_review = false;
                    r.uncertain = false;
                    // Approving a resource also confirms the node it created.
                    if let Some(mut n) = st.nodes.get(&r.node).cloned() {
                        if n.status == NodeStatus::Pending {
                            n.status = NodeStatus::Active;
                            st.put_node(tx, n)?;
                        }
                    }
                }
                "reject" => r.status = Status::Rejected,
                "remove" => r.status = Status::Removed,
                "restrict" => r.status = Status::Restricted,
                _ => return Err(bad("未知操作")),
            }
            r.review_note = clean(note, 300);
            r.reviewed_by = Some(me.id);
            r.updated_at = now();
            st.put_resource(tx, r.clone())?;
            if matches!(r.status, Status::Removed | Status::Rejected) {
                let open: Vec<Id> = st.resource_change_requests.values()
                    .filter(|q| q.resource == id && q.status == "pending")
                    .map(|q| q.id).collect();
                for qid in open {
                    let mut q = st.resource_change_requests[&qid].clone();
                    q.status = "rejected".into();
                    q.reviewed_by = Some(me.id);
                    q.reviewed_at = Some(now());
                    q.review_note = "资料已下架或未通过审核".into();
                    tx.put_resource_change_request(&q)?;
                    st.resource_change_requests.insert(qid, q);
                }
            }
            let e = ReviewEvent { id: st.next_id(tx)?, resource: r.id, actor: me.id, action: action.to_string(), note: r.review_note.clone(), at: now() };
            tx.put_review(&e)?;
            st.reviews.push(e);
            // Rejected and removed files are deleted from storage, so links copied earlier
            // (mirror / GitHub URLs) stop working too. 「仅内部」 is the reversible way to hide one.
            let garbage = if matches!(r.status, Status::Rejected | Status::Removed) { Self::release_blob(st, tx, &r.blob)? } else { Vec::new() };
            Ok((r, garbage))
        })?;
        self.reindex(&[r.node], &[r.id]);
        Ok((r, garbage))
    }

    pub fn request_resource_change(&self, actor: Viewer, id: Id, kind: &str, value: &str) -> Result<ResourceChangeRequest> {
        let me = actor.at_least(Level::Contributor)?;
        if kind != "note" && kind != "delete" { return Err(bad("未知申请类型")); }
        let value = clean(value, if kind == "note" { 500 } else { 300 });
        if kind == "delete" && value.chars().count() < 4 { return Err(bad("请填写至少 4 个字的删除理由")); }
        self.mutate(|st, tx| {
            let r = st.resources.get(&id).ok_or(Error::NotFound("资料"))?;
            if r.uploader != me.id { return Err(Error::Forbidden); }
            if matches!(r.status, Status::Removed | Status::Rejected) { return Err(bad("这份资料已下架或未通过审核")); }
            if kind == "note" && r.note == value { return Err(bad("新备注与当前备注相同")); }
            if st.resource_change_requests.values().any(|q| q.resource == id && q.status == "pending") {
                return Err(Error::Conflict("这份资料已有待处理申请".into()));
            }
            let q = ResourceChangeRequest {
                id: st.next_id(tx)?, resource: id, uploader: me.id, kind: kind.into(), value,
                status: "pending".into(), created_at: now(), reviewed_by: None, reviewed_at: None, review_note: String::new(),
            };
            tx.put_resource_change_request(&q)?;
            st.resource_change_requests.insert(q.id, q.clone());
            Ok(q)
        })
    }

    pub fn resource_change_request(&self, actor: Viewer, id: Id) -> Result<Option<ResourceChangeRequest>> {
        let me = actor.at_least(Level::Contributor)?;
        let st = self.st.read();
        let r = st.resources.get(&id).ok_or(Error::NotFound("资料"))?;
        if r.uploader != me.id && me.level < Level::Reviewer { return Err(Error::Forbidden); }
        Ok(st.resource_change_requests.values().filter(|q| q.resource == id).max_by_key(|q| q.id).cloned())
    }

    pub fn pending_resource_changes(&self, actor: Viewer) -> Result<Vec<(ResourceChangeRequest, Resource)>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut out: Vec<_> = st.resource_change_requests.values()
            .filter(|q| q.status == "pending")
            .filter_map(|q| Some((q.clone(), st.resources.get(&q.resource)?.clone())))
            .collect();
        out.sort_by_key(|(q, _)| (q.created_at, q.id));
        Ok(out)
    }

    /// Returns storage locations that became unreferenced (an approved deletion), like `review`.
    pub fn review_resource_change(&self, actor: Viewer, id: Id, approve: bool, note: &str) -> Result<(ResourceChangeRequest, Vec<Location>)> {
        let me = actor.at_least(Level::Reviewer)?;
        let note = clean(note, 300);
        let (q, changed, garbage) = self.mutate(|st, tx| {
            let mut q = st.resource_change_requests.get(&id).cloned().ok_or(Error::NotFound("申请"))?;
            if q.status != "pending" { return Err(Error::Conflict("申请已处理".into())); }
            let mut r = st.resources.get(&q.resource).cloned().ok_or(Error::NotFound("资料"))?;
            if approve && matches!(r.status, Status::Removed | Status::Rejected) {
                return Err(bad("资料已下架或未通过审核，无法同意申请"));
            }
            if approve {
                match q.kind.as_str() {
                    "note" => r.note = q.value.clone(),
                    "delete" => {
                        r.status = Status::Removed;
                        r.needs_review = false;
                        r.reviewed_by = Some(me.id);
                        r.review_note = if note.is_empty() { q.value.clone() } else { note.clone() };
                    }
                    _ => return Err(bad("未知申请类型")),
                }
                r.updated_at = now();
                st.put_resource(tx, r.clone())?;
            }
            q.status = if approve { "approved" } else { "rejected" }.into();
            q.reviewed_by = Some(me.id);
            q.reviewed_at = Some(now());
            q.review_note = note.clone();
            tx.put_resource_change_request(&q)?;
            st.resource_change_requests.insert(q.id, q.clone());
            let e = ReviewEvent {
                id: st.next_id(tx)?, resource: r.id, actor: me.id,
                action: format!("{}_{}", q.kind, q.status), note, at: now(),
            };
            tx.put_review(&e)?;
            st.reviews.push(e);
            let garbage = if approve && q.kind == "delete" { Self::release_blob(st, tx, &r.blob)? } else { Vec::new() };
            Ok((q, approve, garbage))
        })?;
        if changed { self.reindex(&[], &[q.resource]); }
        Ok((q, garbage))
    }

    /// Drops a blob no live resource or open upload uses; returns its replicas and thumbnail
    /// for deletion. Rejected and removed resources don't count as users; restricted ones keep
    /// their blob so that decision can be undone.
    pub(super) fn release_blob(st: &mut State, tx: &Tx, key: &str) -> Result<Vec<Location>> {
        let used = st.resources.values().any(|r| r.blob == key && !matches!(r.status, Status::Rejected | Status::Removed))
            || st.uploads.values().any(|u| u.key == key && !u.consumed);
        if used {
            return Ok(Vec::new());
        }
        let Some(b) = st.blobs.remove(key) else { return Ok(Vec::new()) };
        tx.del_blob(key)?;
        let mut out: Vec<Location> = b.parts.into_iter().flat_map(|p| p.replicas).collect();
        if let Some(t) = st.thumbs.remove(key) {
            tx.del_thumb(key)?;
            out.extend(t.loc);
        }
        Ok(out)
    }

    /// Whether the file behind a resource is still in storage (false once it was rejected or removed).
    pub fn has_file(&self, r: &Resource) -> bool {
        self.st.read().blobs.contains_key(&r.blob)
    }

    pub fn resource(&self, viewer: Viewer, id: Id) -> Result<(Resource, Node, Vec<Node>)> {
        let st = self.st.read();
        // Let the uploader inspect restricted/removed metadata and review results,
        // while download access still follows Viewer::can_see.
        let r = st.resources.get(&id)
            .filter(|r| viewer.can_see(r) || (matches!(r.status, Status::Removed | Status::Restricted) && viewer.id() == Some(r.uploader)))
            .ok_or(Error::NotFound("资料"))?;
        let n = st.nodes.get(&r.node).ok_or(Error::NotFound("分类"))?;
        let path = st.ancestors(n.id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect();
        Ok((r.clone(), n.clone(), path))
    }

    fn with_nodes<'a>(st: &State, it: impl Iterator<Item = &'a Resource>) -> Vec<(Resource, Node)> {
        it.filter_map(|r| Some((r.clone(), st.nodes.get(&r.node)?.clone()))).collect()
    }

    pub fn recent(&self, limit: usize) -> Vec<(Resource, Node)> {
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.status == Status::Published).collect();
        v.sort_by_key(|r| std::cmp::Reverse((r.created_at, r.id)));
        Self::with_nodes(&st, v.into_iter().take(limit))
    }

    pub fn popular(&self, limit: usize) -> Vec<(Resource, Node)> {
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.status == Status::Published).collect();
        v.sort_by_key(|r| std::cmp::Reverse((r.downloads, r.id)));
        Self::with_nodes(&st, v.into_iter().take(limit))
    }

    pub fn my_resources(&self, actor: Viewer) -> Result<Vec<(Resource, Node)>> {
        let me = actor.at_least(Level::Contributor)?;
        let st = self.st.read();
        let mut v: Vec<&Resource> = st.resources.values().filter(|r| r.uploader == me.id).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(Self::with_nodes(&st, v.into_iter()))
    }

    /// Review queue: pending first-review items, published-before-review items, and
    /// staff-only statuses when `status` asks for them.
    pub fn review_queue(&self, actor: Viewer, status: Option<&str>, uncertain_only: bool) -> Result<Vec<(Resource, Node)>> {
        actor.at_least(Level::Reviewer)?;
        let wanted = status.and_then(Status::parse);
        let st = self.st.read();
        let mut v: Vec<&Resource> = st
            .resources
            .values()
            .filter(|r| match wanted {
                Some(s) => r.status == s,
                None => r.status == Status::Pending || (r.status == Status::Published && r.needs_review),
            })
            .filter(|r| !uncertain_only || r.uncertain)
            .collect();
        v.sort_by_key(|r| (r.created_at, r.id));
        Ok(Self::with_nodes(&st, v.into_iter()))
    }

    pub fn uploader_name(&self, id: Id) -> String {
        self.st.read().users.get(&id).map(|u| u.nickname.clone()).unwrap_or_default()
    }

    // ---------------------------------------------------------------- reports

    /// Files a complaint. Only signed-in users may; the contact is their account email.
    pub fn report(&self, viewer: Viewer, resource: Id, reason: &str, ip: &str) -> Result<Report> {
        let me = viewer.at_least(Level::Contributor)?.clone();
        let reason = clean(reason, 1000);
        if reason.chars().count() < 4 {
            return Err(bad("请写明投诉或下架理由"));
        }
        self.mutate(|st, tx| {
            if !st.resources.contains_key(&resource) {
                return Err(Error::NotFound("资料"));
            }
            let open_from_ip = st.reports.values().filter(|r| r.ip == ip && !r.handled).count();
            if open_from_ip >= 20 {
                return Err(Error::TooMany("你提交的投诉较多，请等待处理".into()));
            }
            let r = Report {
                id: st.next_id(tx)?,
                resource,
                reason,
                contact: me.email.clone(),
                reporter: Some(me.id),
                ip: clean(ip, 64),
                created_at: now(),
                handled: false,
                handled_by: None,
                handled_note: String::new(),
            };
            tx.put_report(&r)?;
            st.reports.insert(r.id, r.clone());
            Ok(r)
        })
    }

    /// Deletes a complaint outright (test entries, spam). Admins only.
    pub fn delete_report(&self, actor: Viewer, id: Id) -> Result<()> {
        actor.at_least(Level::Admin)?;
        self.mutate(|st, tx| {
            st.reports.remove(&id).ok_or(Error::NotFound("投诉"))?;
            tx.del_report(id)
        })
    }

    pub fn reports(&self, actor: Viewer, include_handled: bool) -> Result<Vec<(Report, Option<(Resource, Node)>)>> {
        actor.at_least(Level::Admin)?;
        let st = self.st.read();
        let mut v: Vec<(Report, Option<(Resource, Node)>)> = st
            .reports
            .values()
            .filter(|r| include_handled || !r.handled)
            .map(|r| {
                let res = st.resources.get(&r.resource).and_then(|x| Some((x.clone(), st.nodes.get(&x.node)?.clone())));
                (r.clone(), res)
            })
            .collect();
        v.sort_by_key(|(r, _)| std::cmp::Reverse(r.created_at));
        Ok(v)
    }

    pub fn handle_report(&self, actor: Viewer, id: Id, note: &str) -> Result<Report> {
        let me = actor.at_least(Level::Admin)?.clone();
        self.mutate(|st, tx| {
            let mut r = st.reports.get(&id).cloned().ok_or(Error::NotFound("投诉"))?;
            r.handled = true;
            r.handled_by = Some(me.id);
            r.handled_note = clean(note, 300);
            tx.put_report(&r)?;
            st.reports.insert(id, r.clone());
            Ok(r)
        })
    }

    // ---------------------------------------------------------------- downloads

    /// Download plan. `count` = false for previews, which don't bump the counter.
    pub fn download(&self, viewer: Viewer, id: Id, count: bool) -> Result<DownloadPlan> {
        let plan = {
            let st = self.st.read();
            let r = st
                .resources
                .get(&id)
                .filter(|r| viewer.can_see(r) && r.status != Status::Rejected)
                .ok_or(Error::NotFound("资料"))?;
            let b = st.blobs.get(&r.blob).ok_or(Error::NotFound("文件"))?;
            DownloadPlan {
                filename: r.filename(),
                size: r.size,
                mime: r.mime.clone(),
                parts: b
                    .parts
                    .iter()
                    .map(|p| DownloadPart { size: p.size, sha256: p.sha256.clone(), urls: self.storage.download_urls(&p.replicas) })
                    .collect(),
            }
        };
        if count {
            if let Some(r) = self.st.write().resources.get_mut(&id) {
                r.downloads += 1;
            }
            self.dirty_downloads.lock().insert(id);
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod change_request_tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::Db;
    use crate::hub::Limits;
    use crate::storage::{Storage, local::LocalBackend};

    fn user(id: Id, level: Level) -> User {
        User {
            id, email: format!("{id}@example.invalid"), nickname: format!("user{id}"),
            password: String::new(), level, banned: false, created_at: now(),
            created_ip: String::new(), last_login: 0, uploads: 0,
        }
    }

    #[test]
    fn uploader_changes_wait_for_review_and_survive_restart() {
        let dir = std::env::temp_dir().join(format!("xmuhub-change-request-{}-{}", std::process::id(), now()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Arc::new(Db::open(&dir.join("test.redb"), 1 << 20).unwrap());
        let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"test".to_vec() })]);
        let h = Hub::open(db.clone(), storage.clone(), Limits::default(), vec![]).unwrap();
        let owner = user(1, Level::Contributor);
        let other = user(2, Level::Contributor);
        let reviewer = user(3, Level::Reviewer);
        let resource = Resource {
            id: 5, node: 4, tag: Tag::Slides,
            name: NameParts { course: "课程".into(), time: "2025".into(), type_word: "资料".into(), version: 1, ..Default::default() },
            ext: "pdf".into(), note: "原备注".into(), original_name: "x.pdf".into(), source: String::new(),
            uncertain: false, blob: "test".into(), size: 1, mime: "application/pdf".into(),
            status: Status::Pending, needs_review: false, review_note: String::new(), uploader: owner.id,
            reviewed_by: None, created_at: now(), updated_at: now(), downloads: 0,
        };
        h.mutate(|st, tx| {
            st.seq = 10;
            tx.put_meta(super::super::SEQ_KEY, &st.seq.to_le_bytes())?;
            for u in [&owner, &other, &reviewer] { st.put_user(tx, u.clone())?; }
            st.put_node(tx, Node {
                id: 4, parent: None, kind: NodeKind::Course, code: String::new(), name: "课程".into(),
                label: "课程".into(), aliases: vec![], bucketed: false, sort: 0,
                status: NodeStatus::Active, created_by: reviewer.id, created_at: now(),
            })?;
            st.put_resource(tx, resource.clone())?;
            Ok(())
        }).unwrap();
        let mine = Viewer { user: Some(&owner) };
        let stranger = Viewer { user: Some(&other) };
        let staff = Viewer { user: Some(&reviewer) };
        let edited = ResourceInput {
            node: 4, course: "课程".into(), time: "2025".into(), type_word: "资料".into(),
            tag: String::new(), paper: String::new(), with_answer: false, extra: String::new(),
            note: "新备注".into(), subtitle: None,
        };
        assert!(h.update_resource(mine, 5, edited, AdminExtras::default()).is_err());
        assert!(h.request_resource_change(stranger, 5, "note", "新备注").is_err());
        let q = h.request_resource_change(mine, 5, "note", "新备注").unwrap();
        assert_eq!(h.resource_change_request(mine, 5).unwrap().unwrap().id, q.id);
        assert!(h.resource_change_request(stranger, 5).is_err());
        assert!(h.pending_resource_changes(mine).is_err());
        assert!(h.request_resource_change(mine, 5, "delete", "内容有误").is_err());
        assert!(h.review_resource_change(mine, q.id, true, "").is_err());
        assert_eq!(h.resource(mine, 5).unwrap().0.note, "原备注");
        drop(h);

        let h = Hub::open(db.clone(), storage, Limits::default(), vec![]).unwrap();
        assert_eq!(h.pending_resource_changes(staff).unwrap().len(), 1);
        h.review_resource_change(staff, q.id, true, "").unwrap();
        assert_eq!(h.resource(mine, 5).unwrap().0.note, "新备注");
        assert!(h.review_resource_change(staff, q.id, true, "").is_err());
        let rejected = h.request_resource_change(mine, 5, "delete", "内容有误").unwrap();
        h.review_resource_change(staff, rejected.id, false, "保留资料").unwrap();
        assert_eq!(h.resource(mine, 5).unwrap().0.status, Status::Pending);
        let remove = h.request_resource_change(mine, 5, "delete", "内容有误").unwrap();
        h.review_resource_change(staff, remove.id, true, "").unwrap();
        assert_eq!(h.resource(mine, 5).unwrap().0.status, Status::Removed);
        assert!(matches!(h.download(mine, 5, false), Err(Error::NotFound("资料"))));
        assert!(h.request_resource_change(mine, 5, "note", "再改").is_err());
        drop(h);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::valid_time;

    #[test]
    fn times() {
        for ok in ["", "2023-2024秋", "2025春", "2023", "2019-2023", "202406", "2024暑"] {
            assert!(valid_time(ok), "{ok}");
        }
        for bad in ["23-24", "2024秋季", "20240", "202406秋", "abc"] {
            assert!(!valid_time(bad), "{bad}");
        }
    }
}
