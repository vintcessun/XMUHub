//! Full-text search on Tantivy, held entirely in RAM and rebuilt from redb at boot,
//! so there is no second on-disk store to keep consistent.
//!
//! Text is pre-tokenised by `crate::text` and joined with spaces, so the index only
//! needs Tantivy's whitespace tokenizer and queries are built from exact terms.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

use crate::error::Result;
use crate::model::{Id, Node, Resource};
use crate::text::{index_tokens, loose_units, pinyin_forms, query_units};

/// Tantivy's minimum writer arena; we index a few docs at a time, so the floor is plenty.
const WRITER_BUDGET: usize = 15_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocType {
    Node = 0,
    Resource = 1,
}

#[derive(Default, Clone, Copy)]
pub struct Filter {
    pub ty: Option<DocType>,
    /// Restrict node results to actual courses, excluding offering groups/colleges.
    pub courses_only: bool,
    /// Match only the displayed course or resource title, not aliases or metadata.
    pub name_only: bool,
    /// Restrict to this node's subtree.
    pub within: Option<Id>,
    /// Study level: `LEVEL_UNDERGRAD` (also matches untagged courses) or `LEVEL_GRADUATE`.
    pub level: Option<u8>,
    /// Tag index (see `Tag::ALL`).
    pub tag: Option<u64>,
}

/// What a document needs from the category tree: its own node, every ancestor
/// (for subtree filters) and the path's display text (so "物理 期末" finds A3 papers).
pub struct Placement<'a> {
    pub node: Id,
    pub ancestors: &'a [Id],
    pub path_text: &'a str,
    pub aliases_text: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub ty: DocType,
    pub id: Id,
    pub score: f32,
}

struct Fields {
    key: Field,
    ty: Field,
    anc: Field,
    tag: Field,
    course: Field,
    /// 1 when the doc counts as 本科 / 研究生 (a course tagged both counts as both).
    ug: Field,
    grad: Field,
    name: Field,
    name_py: Field,
    main: Field,
    py: Field,
    sub: Field,
    /// Rank multiplier ×100 (see `Hub::node_weight`); resources use 100.
    w: Field,
}

pub struct Search {
    f: Fields,
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    /// Changes staged since the last commit. Writes don't commit (it made every upload,
    /// new course or review wait, and during a batch upload each one queued behind the
    /// others); [`Search::flush`] does, from a background tick and before every search.
    dirty: AtomicBool,
}

fn key(ty: DocType, id: Id) -> u64 {
    ((ty as u64) << 56) | id
}

fn joined(text: &str) -> String {
    index_tokens(text).join(" ")
}

impl Search {
    pub fn new() -> Result<Search> {
        let mut sb = Schema::builder();
        let text = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("whitespace")
                .set_index_option(IndexRecordOption::WithFreqs),
        );
        let f = Fields {
            key: sb.add_u64_field("key", INDEXED | FAST),
            ty: sb.add_u64_field("ty", INDEXED),
            anc: sb.add_u64_field("anc", INDEXED),
            tag: sb.add_u64_field("tag", INDEXED),
            course: sb.add_u64_field("course", INDEXED),
            ug: sb.add_u64_field("ug", INDEXED),
            grad: sb.add_u64_field("grad", INDEXED),
            name: sb.add_text_field("name", text.clone()),
            name_py: sb.add_text_field("name_py", text.clone()),
            main: sb.add_text_field("main", text.clone()),
            py: sb.add_text_field("py", text.clone()),
            sub: sb.add_text_field("sub", text),
            w: sb.add_u64_field("w", FAST),
        };
        let index = Index::create_in_ram(sb.build());
        let writer = index.writer_with_num_threads(1, WRITER_BUDGET)?;
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(Search { f, writer: Mutex::new(writer), reader, dirty: AtomicBool::new(false) })
    }

    pub fn put_node(&self, n: &Node, at: &Placement, weight: u64, is_course: bool, level: u8) -> Result<()> {
        let f = &self.f;
        let (ug, grad) = level_flags(level);
        let mut doc = doc!(
            f.key => key(DocType::Node, n.id),
            f.ty => DocType::Node as u64,
            f.course => u64::from(is_course),
            f.ug => ug,
            f.grad => grad,
            f.name => joined(&n.name),
            f.name_py => joined(&pinyin_forms(&n.name)),
            f.main => joined(&format!("{} {} {} {}", n.name, n.label, n.code, at.aliases_text)),
            f.py => joined(&pinyin_forms(&format!("{} {} {}", n.name, n.label, at.aliases_text))),
            f.sub => joined(at.path_text),
            f.w => weight,
        );
        for a in at.ancestors.iter().chain(std::iter::once(&n.id)) {
            doc.add_u64(f.anc, *a);
        }
        let w = self.writer.lock();
        w.delete_term(Term::from_field_u64(f.key, key(DocType::Node, n.id)));
        w.add_document(doc)?;
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    pub fn put_resource(&self, r: &Resource, at: &Placement, subtitle: &str, level: u8) -> Result<()> {
        let f = &self.f;
        let stem = r.name.stem();
        let tag_idx = crate::model::Tag::ALL.iter().position(|t| *t == r.tag).unwrap_or(0) as u64;
        let (ug, grad) = level_flags(level);
        let mut doc = doc!(
            f.key => key(DocType::Resource, r.id),
            f.ty => DocType::Resource as u64,
            f.tag => tag_idx,
            f.ug => ug,
            f.grad => grad,
            f.name => joined(&stem),
            f.name_py => joined(&pinyin_forms(&stem)),
            f.main => joined(&format!("{stem} {subtitle} {}", at.aliases_text)),
            f.py => joined(&pinyin_forms(&format!("{} {}", r.name.course, at.aliases_text))),
            f.sub => joined(&format!("{} {} {}", at.path_text, r.tag.label(), r.note)),
            f.w => 100u64,
        );
        for a in at.ancestors.iter().chain(std::iter::once(&at.node)) {
            doc.add_u64(f.anc, *a);
        }
        let w = self.writer.lock();
        w.delete_term(Term::from_field_u64(f.key, key(DocType::Resource, r.id)));
        w.add_document(doc)?;
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    pub fn remove(&self, ty: DocType, id: Id) {
        self.writer.lock().delete_term(Term::from_field_u64(self.f.key, key(ty, id)));
        self.dirty.store(true, Ordering::Release);
    }

    /// Makes pending changes visible to searches.
    pub fn commit(&self) -> Result<()> {
        let t = std::time::Instant::now();
        self.writer.lock().commit()?;
        self.reader.reload()?;
        if t.elapsed() > std::time::Duration::from_millis(500) {
            tracing::warn!(ms = t.elapsed().as_millis() as u64, "slow search commit");
        }
        Ok(())
    }

    /// Commits if anything changed since the last commit.
    pub fn flush(&self) -> Result<()> {
        if self.dirty.swap(false, Ordering::AcqRel)
            && let Err(e) = self.commit()
        {
            self.dirty.store(true, Ordering::Release);
            return Err(e);
        }
        Ok(())
    }

    pub fn search(&self, q: &str, filter: Filter, limit: usize, offset: usize) -> Result<(Vec<Hit>, usize)> {
        // A search right after a write sees it (the background tick may not have run yet).
        self.flush()?;
        let strict = self.run(&query_units(q), false, filter, limit, offset)?;
        if strict.1 > 0 {
            return Ok(strict);
        }
        let loose = loose_units(q);
        if loose.len() < 2 {
            return Ok(strict);
        }
        // Abbreviation fallback: every character must be present somewhere.
        self.run(&loose, true, filter, limit, offset)
    }

    fn run(&self, units: &[String], all: bool, filter: Filter, limit: usize, offset: usize) -> Result<(Vec<Hit>, usize)> {
        if units.is_empty() || limit == 0 {
            return Ok((Vec::new(), 0));
        }
        let f = &self.f;
        let term_q = |field: Field, unit: &str, boost: f32| -> (Occur, Box<dyn Query>) {
            let tq = TermQuery::new(Term::from_field_text(field, unit), IndexRecordOption::WithFreqs);
            (Occur::Should, Box::new(BoostQuery::new(Box::new(tq), boost)))
        };
        let unit_queries: Vec<(Occur, Box<dyn Query>)> = units
            .iter()
            .map(|u| {
                let fields = if filter.name_only {
                    vec![term_q(f.name, u, 3.0), term_q(f.name_py, u, 2.0)]
                } else {
                    vec![term_q(f.main, u, 3.0), term_q(f.py, u, 2.0), term_q(f.sub, u, 1.0)]
                };
                let any_field = BooleanQuery::new(fields);
                (Occur::Should, Box::new(any_field) as Box<dyn Query>)
            })
            .collect();
        // Most units must hit; a stray typo in a long query shouldn't empty the results.
        let need = if all { units.len() } else { (units.len() * 7).div_ceil(10).max(1) };
        let text = BooleanQuery::with_minimum_required_clauses(unit_queries, need);

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, Box::new(text))];
        let exact = |field: Field, v: u64| -> (Occur, Box<dyn Query>) {
            (
                Occur::Must,
                Box::new(TermQuery::new(Term::from_field_u64(field, v), IndexRecordOption::Basic)),
            )
        };
        if let Some(ty) = filter.ty {
            clauses.push(exact(f.ty, ty as u64));
        }
        if filter.courses_only {
            clauses.push(exact(f.course, 1));
        }
        if let Some(n) = filter.within {
            clauses.push(exact(f.anc, n));
        }
        match filter.level {
            Some(crate::hub::LEVEL_UNDERGRAD) => clauses.push(exact(f.ug, 1)),
            Some(crate::hub::LEVEL_GRADUATE) => clauses.push(exact(f.grad, 1)),
            _ => {}
        }
        if let Some(t) = filter.tag {
            clauses.push(exact(f.tag, t));
        }
        let query = BooleanQuery::new(clauses);

        let searcher = self.reader.searcher();
        let ranked = TopDocs::with_limit(limit).and_offset(offset).tweak_score(|seg: &tantivy::SegmentReader| {
            let w = seg.fast_fields().u64("w").ok().map(|c| c.first_or_default_col(100));
            move |doc: tantivy::DocId, score: tantivy::Score| score * w.as_ref().map_or(100, |c| c.get_val(doc)) as f32 / 100.0
        });
        let collector = (ranked, Count);
        let (top, total) = searcher.search(&query, &collector)?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let k = self.key_of(&searcher, addr)?;
            let ty = if k >> 56 == 0 { DocType::Node } else { DocType::Resource };
            hits.push(Hit { ty, id: k & ((1 << 56) - 1), score });
        }
        Ok((hits, total))
    }

    fn key_of(&self, searcher: &tantivy::Searcher, addr: DocAddress) -> Result<u64> {
        let col = searcher.segment_reader(addr.segment_ord).fast_fields().u64("key")?;
        Ok(col.first(addr.doc_id).unwrap_or(0))
    }
}

/// (本科, 研究生) index flags for a study level; untagged counts as 本科.
fn level_flags(level: u8) -> (u64, u64) {
    use crate::hub::{LEVEL_BOTH, LEVEL_GRADUATE};
    (u64::from(level != LEVEL_GRADUATE), u64::from(level == LEVEL_GRADUATE || level == LEVEL_BOTH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NameParts, NodeKind, NodeStatus, Resource, Status, Tag};

    fn node(id: Id, name: &str, kind: NodeKind) -> Node {
        Node {
            id, parent: None, kind, name: name.into(), label: name.into(), code: String::new(),
            aliases: vec![], bucketed: false, sort: 0, status: NodeStatus::Active,
            created_by: 0, created_at: 0,
        }
    }

    fn count(index: &Search, q: &str, filter: Filter) -> usize {
        index.search(q, filter, 20, 0).unwrap().1
    }

    #[test]
    fn name_search_excludes_categories_and_resource_metadata() {
        let index = Search::new().unwrap();
        let place = Placement { node: 1, ancestors: &[], path_text: "专业课 信息学院", aliases_text: "算法" };
        index.put_node(&node(1, "信息学院", NodeKind::Group), &place, 100, false, 0).unwrap();
        index.put_node(&node(2, "数据结构", NodeKind::Course), &place, 100, true, 0).unwrap();
        index.put_node(&node(3, "智能制造学院", NodeKind::Course), &place, 100, false, 0).unwrap();
        let resource = Resource {
            id: 4, node: 2, tag: Tag::Exam,
            name: NameParts { course: "数据结构".into(), time: "2025".into(), type_word: "期末试卷".into(), version: 1, ..Default::default() },
            ext: "pdf".into(), note: "讲义".into(), original_name: "x.pdf".into(), source: String::new(),
            uncertain: false, blob: String::new(), size: 1, mime: "application/pdf".into(),
            status: Status::Published, needs_review: false, review_note: String::new(), uploader: 0,
            reviewed_by: None, created_at: 0, updated_at: 0, downloads: 0,
        };
        index.put_resource(&resource, &place, "重点整理", 0).unwrap();
        index.commit().unwrap();

        let courses = Filter { ty: Some(DocType::Node), courses_only: true, name_only: true, ..Default::default() };
        assert_eq!(count(&index, "数据结构", courses), 1);
        assert_eq!(count(&index, "sjjg", courses), 1);
        assert_eq!(count(&index, "信息学院", courses), 0);
        assert_eq!(count(&index, "智能制造学院", courses), 0);
        assert_eq!(count(&index, "算法", courses), 0);

        let files = Filter { ty: Some(DocType::Resource), name_only: true, ..Default::default() };
        assert_eq!(count(&index, "数据结构", files), 1);
        assert_eq!(count(&index, "期末试卷", files), 1);
        assert_eq!(count(&index, "信息学院", files), 0);
        assert_eq!(count(&index, "重点整理", files), 0);
        assert_eq!(count(&index, "讲义", files), 0);
        assert_eq!(count(&index, "算法", files), 0);
        // Other API callers keep the existing broader search behavior.
        assert_eq!(count(&index, "重点整理", Filter { ty: Some(DocType::Resource), ..Default::default() }), 1);
    }
}

#[cfg(test)]
mod deferred_commit_tests {
    use super::*;

    fn node(id: Id, name: &str) -> Node {
        serde_json::from_value(serde_json::json!({
            "id": id, "parent": null, "kind": "Course", "code": "", "name": name, "label": "",
            "aliases": [], "bucketed": false, "sort": 0, "status": "Active", "created_by": 1, "created_at": 0
        }))
        .unwrap()
    }

    #[test]
    fn writes_stage_and_searches_see_them() {
        let s = Search::new().unwrap();
        let at = Placement { node: 7, ancestors: &[], path_text: "", aliases_text: "" };
        s.put_node(&node(7, "微积分"), &at, 100, true, 0).unwrap();
        assert!(s.dirty.load(Ordering::Acquire), "a write only stages");
        assert_eq!(s.reader.searcher().num_docs(), 0, "not committed by the write");
        let (hits, total) = s.search("微积分", Filter::default(), 10, 0).unwrap();
        assert_eq!((hits.len(), total), (1, 1), "a search commits what is pending first");
        assert!(!s.dirty.load(Ordering::Acquire));
        s.remove(DocType::Node, 7);
        s.flush().unwrap();
        assert_eq!(s.search("微积分", Filter::default(), 10, 0).unwrap().1, 0);
    }
}
