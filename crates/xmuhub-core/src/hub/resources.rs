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
    fn build_name(st: &State, input: &ResourceInput, extras: &AdminExtras, staff: bool, exclude: Option<Id>) -> Result<(NameParts, Tag)> {
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
            if dup {
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
            let own_pending = r.uploader == me.id && r.status == Status::Pending;
            if !staff && !own_pending {
                return Err(Error::Forbidden);
            }
            let (mut name, tag) = Self::build_name(st, &input, &extras, staff, Some(id))?;
            let node = st.resolve(input.node).map(|n| n.id).ok_or(Error::NotFound("分类"))?;
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
            st.put_resource(tx, r.clone())?;
            Ok(r)
        })?;
        self.reindex(&[], &[r.id]);
        Ok(r)
    }

    /// approve / reject / remove / restore / restrict. Returns storage locations that became
    /// unreferenced for the caller to delete (network I/O stays outside the write lock).
    pub fn review(&self, actor: Viewer, id: Id, action: &str, note: &str) -> Result<(Resource, Vec<Location>)> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        let (r, garbage) = self.mutate(|st, tx| {
            let mut r = st.resources.get(&id).cloned().ok_or(Error::NotFound("资料"))?;
            match action {
                "approve" | "restore" => {
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
            let garbage = if r.status == Status::Rejected { Self::release_blob(st, tx, &r.blob)? } else { Vec::new() };
            Ok((r, garbage))
        })?;
        self.reindex(&[r.node], &[r.id]);
        Ok((r, garbage))
    }

    /// Drops a blob no live resource or open upload uses; returns its replicas for deletion.
    /// Removed/restricted resources keep their blob so the decision can be undone.
    pub(super) fn release_blob(st: &mut State, tx: &Tx, key: &str) -> Result<Vec<Location>> {
        let used = st.resources.values().any(|r| r.blob == key && r.status != Status::Rejected)
            || st.uploads.values().any(|u| u.key == key && !u.consumed);
        if used {
            return Ok(Vec::new());
        }
        let Some(b) = st.blobs.remove(key) else { return Ok(Vec::new()) };
        tx.del_blob(key)?;
        Ok(b.parts.into_iter().flat_map(|p| p.replicas).collect())
    }

    pub fn resource(&self, viewer: Viewer, id: Id) -> Result<(Resource, Node, Vec<Node>)> {
        let st = self.st.read();
        let r = st.resources.get(&id).filter(|r| viewer.can_see(r)).ok_or(Error::NotFound("资料"))?;
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

    pub fn report(&self, viewer: Viewer, resource: Id, reason: &str, contact: &str, ip: &str) -> Result<Report> {
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
                contact: clean(contact, 100),
                reporter: viewer.id(),
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

    pub fn reports(&self, actor: Viewer, include_handled: bool) -> Result<Vec<(Report, Option<(Resource, Node)>)>> {
        actor.at_least(Level::Reviewer)?;
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
        let me = actor.at_least(Level::Reviewer)?.clone();
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

    pub fn download(&self, viewer: Viewer, id: Id) -> Result<DownloadPlan> {
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
        if let Some(r) = self.st.write().resources.get_mut(&id) {
            r.downloads += 1;
        }
        self.dirty_downloads.lock().insert(id);
        Ok(plan)
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
