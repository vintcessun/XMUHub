//! Imports from other public GitHub repositories, by reference, plus the bookkeeping the
//! background transfer (GitHub Actions copy into our own storage) needs.

use serde::Serialize;

use super::resources::AdminExtras;
use super::{Hub, ResourceInput, Viewer, clean};
use crate::error::{Error, Result};
use crate::model::*;
use crate::storage::github::{RepoFile, RepoScan};
use crate::text::guess_name;

/// File types imported from repositories: documents and archives, no source code.
pub const DOC_EXTS: &[&str] = &["pdf", "doc", "docx", "ppt", "pptx", "pptm", "xls", "xlsx", "epub", "zip", "rar", "7z", "caj"];

pub fn is_doc(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| DOC_EXTS.contains(&e.to_lowercase().as_str()))
}

/// The folder a file is grouped under: its first `depth` directories.
pub fn group_of(path: &str, depth: usize) -> String {
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    dirs[..dirs.len().min(depth)].join("/")
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub created: usize,
    pub skipped_existing: usize,
    pub unmapped: usize,
}

/// A stored blob that exists only as a reference, awaiting a copy into our own storage.
#[derive(Debug, Clone, Serialize)]
pub struct Unpersisted {
    pub key: String,
    pub size: u64,
    pub url: String,
    pub basename: String,
}

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" | "pptm" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "epub" => "application/epub+zip",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

impl Hub {
    /// Creates pending, "to verify" resources that reference `scan`'s files in place.
    /// `mapping(group)` gives the target node for each file group; unmapped groups are skipped.
    /// Re-running is safe: a file already imported into the same node is skipped.
    pub fn import_repo(&self, actor: Viewer, scan: &RepoScan, depth: usize, mapping: &dyn Fn(&str) -> Option<Id>) -> Result<ImportReport> {
        let me = actor.at_least(Level::Reviewer)?.clone();
        let license = if scan.license.is_empty() { String::new() } else { format!("，{} 许可", scan.license) };
        let note = format!("来源：GitHub {}/{}{license}", scan.owner, scan.repo);
        let files: Vec<(&RepoFile, Id)> = scan.files.iter().filter(|f| is_doc(&f.path)).filter_map(|f| Some((f, mapping(&group_of(&f.path, depth))?))).collect();
        let mut report = ImportReport { unmapped: scan.files.iter().filter(|f| is_doc(&f.path)).count() - files.len(), ..Default::default() };
        let mut touched: Vec<Id> = Vec::new();
        for chunk in files.chunks(200) {
            let (created, skipped, ids) = self.mutate(|st, tx| {
                let (mut created, mut skipped, mut ids) = (0, 0, Vec::new());
                for (f, node) in chunk {
                    let key = format!("git-{}", f.sha);
                    let node = st.resolve(*node).map(|n| n.id).ok_or(Error::NotFound("分类"))?;
                    let dup = st.by_node.get(&node).into_iter().flatten().any(|id| st.resources[id].blob == key);
                    if dup {
                        skipped += 1;
                        continue;
                    }
                    if !st.blobs.contains_key(&key) {
                        let b = Blob {
                            key: key.clone(),
                            size: f.size,
                            parts: vec![Part {
                                size: f.size,
                                sha256: String::new(),
                                replicas: vec![Location::GitHubRepo {
                                    owner: scan.owner.clone(),
                                    repo: scan.repo.clone(),
                                    commit: scan.commit.clone(),
                                    path: f.path.clone(),
                                }],
                            }],
                            created_at: now(),
                        };
                        tx.put_blob(&b)?;
                        st.blobs.insert(key.clone(), b);
                    }
                    // Guess from the path below the group folder: sub-folders often say 期末/实验.
                    let group = group_of(&f.path, depth);
                    let rel = f.path.strip_prefix(&group).unwrap_or(&f.path).trim_start_matches('/');
                    let (stem, ext) = rel.rsplit_once('.').map(|(s, e)| (s, e.to_lowercase())).unwrap_or((rel, String::new()));
                    let g = guess_name(stem, &ext);
                    let input = ResourceInput {
                        node,
                        course: String::new(),
                        time: g.time,
                        type_word: g.type_word.to_string(),
                        tag: String::new(),
                        paper: g.paper,
                        with_answer: g.with_answer,
                        extra: g.extra,
                        note: note.clone(),
                    };
                    let extras = AdminExtras { free_type: true, ..Default::default() };
                    let (name, tag) = Self::build_name(st, &input, &extras, true, None)?;
                    let r = Resource {
                        id: st.next_id(tx)?,
                        node,
                        tag,
                        name,
                        ext: if ext.len() <= 8 { ext.clone() } else { String::new() },
                        note: note.clone(),
                        original_name: clean(&f.path, 200),
                        source: format!("github:{}/{}@{}", scan.owner, scan.repo, &scan.commit[..scan.commit.len().min(12)]),
                        uncertain: true,
                        blob: key,
                        size: f.size,
                        mime: mime_of(&ext).to_string(),
                        status: Status::Pending,
                        needs_review: false,
                        review_note: String::new(),
                        uploader: me.id,
                        reviewed_by: None,
                        created_at: now(),
                        updated_at: now(),
                        downloads: 0,
                    };
                    ids.push(r.id);
                    st.put_resource(tx, r)?;
                    created += 1;
                }
                Ok((created, skipped, ids))
            })?;
            report.created += created;
            report.skipped_existing += skipped;
            touched.extend(ids);
        }
        self.reindex(&[], &touched);
        Ok(report)
    }

    /// Blobs that only exist as references, biggest backlog first capped by count and bytes.
    pub fn unpersisted(&self, limit: usize, max_bytes: u64) -> Vec<Unpersisted> {
        let st = self.st.read();
        let mut out = Vec::new();
        let mut bytes = 0u64;
        let mut keys: Vec<&Blob> = st.blobs.values().filter(|b| b.parts.iter().any(|p| !p.replicas.iter().any(Location::owned))).collect();
        keys.sort_by_key(|b| b.created_at);
        for b in keys {
            if out.len() >= limit || bytes + b.size > max_bytes {
                break;
            }
            // Referenced blobs are always a single part.
            let Some(Location::GitHubRepo { owner, repo, commit, path }) = b.parts.first().and_then(|p| p.replicas.first()) else { continue };
            bytes += b.size;
            out.push(Unpersisted {
                key: b.key.clone(),
                size: b.size,
                url: crate::storage::repo_ref::raw_url(owner, repo, commit, path),
                basename: path.rsplit('/').next().unwrap_or(path).to_string(),
            });
        }
        out
    }

    /// Adds our own copy of a referenced blob; it becomes the preferred download source.
    pub fn attach_replica(&self, key: &str, loc: Location, sha256: &str) -> Result<()> {
        self.mutate(|st, tx| {
            let mut b = st.blobs.get(key).cloned().ok_or(Error::NotFound("文件"))?;
            let part = b.parts.first_mut().ok_or(Error::NotFound("文件"))?;
            if part.replicas.iter().any(|l| l == &loc) {
                return Ok(());
            }
            part.replicas.insert(0, loc);
            if part.sha256.is_empty() {
                part.sha256 = sha256.to_string();
            }
            tx.put_blob(&b)?;
            st.blobs.insert(b.key.clone(), b);
            Ok(())
        })
    }

    /// (blobs only referenced, blobs with an own copy) — for the admin status page.
    pub fn transfer_counts(&self) -> (usize, usize) {
        let st = self.st.read();
        let refs = st.blobs.values().filter(|b| b.parts.iter().any(|p| !p.replicas.iter().any(Location::owned))).count();
        (refs, st.blobs.len() - refs)
    }

    pub fn blob_size(&self, key: &str) -> Option<u64> {
        self.st.read().blobs.get(key).map(|b| b.size)
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        use redb::ReadableDatabase;
        let txn = self.db.inner.begin_read()?;
        let t = txn.open_table(crate::db::META)?;
        Ok(t.get(key)?.map(|v| v.value().to_vec()))
    }

    pub fn meta_put(&self, key: &str, value: &[u8]) -> Result<()> {
        let _w = self.writer.lock();
        self.db.write(|tx| tx.put_meta(key, value))
    }
}
