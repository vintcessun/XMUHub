//! Full-text search on Tantivy, held entirely in RAM and rebuilt from redb at boot,
//! so there is no second on-disk store to keep consistent.
//!
//! Text is pre-tokenised by `crate::text` and joined with spaces, so the index only
//! needs Tantivy's whitespace tokenizer and queries are built from exact terms.

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
    /// Restrict to this node's subtree.
    pub within: Option<Id>,
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
    main: Field,
    py: Field,
    sub: Field,
}

pub struct Search {
    f: Fields,
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
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
            main: sb.add_text_field("main", text.clone()),
            py: sb.add_text_field("py", text.clone()),
            sub: sb.add_text_field("sub", text),
        };
        let index = Index::create_in_ram(sb.build());
        let writer = index.writer_with_num_threads(1, WRITER_BUDGET)?;
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(Search { f, writer: Mutex::new(writer), reader })
    }

    pub fn put_node(&self, n: &Node, at: &Placement) -> Result<()> {
        let f = &self.f;
        let mut doc = doc!(
            f.key => key(DocType::Node, n.id),
            f.ty => DocType::Node as u64,
            f.main => joined(&format!("{} {} {} {}", n.name, n.label, n.code, at.aliases_text)),
            f.py => joined(&pinyin_forms(&format!("{} {} {}", n.name, n.label, at.aliases_text))),
            f.sub => joined(at.path_text),
        );
        for a in at.ancestors.iter().chain(std::iter::once(&n.id)) {
            doc.add_u64(f.anc, *a);
        }
        let w = self.writer.lock();
        w.delete_term(Term::from_field_u64(f.key, key(DocType::Node, n.id)));
        w.add_document(doc)?;
        Ok(())
    }

    pub fn put_resource(&self, r: &Resource, at: &Placement) -> Result<()> {
        let f = &self.f;
        let stem = r.name.stem();
        let tag_idx = crate::model::Tag::ALL.iter().position(|t| *t == r.tag).unwrap_or(0) as u64;
        let mut doc = doc!(
            f.key => key(DocType::Resource, r.id),
            f.ty => DocType::Resource as u64,
            f.tag => tag_idx,
            f.main => joined(&format!("{stem} {}", at.aliases_text)),
            f.py => joined(&pinyin_forms(&format!("{} {}", r.name.course, at.aliases_text))),
            f.sub => joined(&format!("{} {} {}", at.path_text, r.tag.label(), r.note)),
        );
        for a in at.ancestors.iter().chain(std::iter::once(&at.node)) {
            doc.add_u64(f.anc, *a);
        }
        let w = self.writer.lock();
        w.delete_term(Term::from_field_u64(f.key, key(DocType::Resource, r.id)));
        w.add_document(doc)?;
        Ok(())
    }

    pub fn remove(&self, ty: DocType, id: Id) {
        self.writer.lock().delete_term(Term::from_field_u64(self.f.key, key(ty, id)));
    }

    /// Makes pending changes visible to searches.
    pub fn commit(&self) -> Result<()> {
        self.writer.lock().commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search(&self, q: &str, filter: Filter, limit: usize, offset: usize) -> Result<(Vec<Hit>, usize)> {
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
                let any_field = BooleanQuery::new(vec![
                    term_q(f.main, u, 3.0),
                    term_q(f.py, u, 2.0),
                    term_q(f.sub, u, 1.0),
                ]);
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
        if let Some(n) = filter.within {
            clauses.push(exact(f.anc, n));
        }
        if let Some(t) = filter.tag {
            clauses.push(exact(f.tag, t));
        }
        let query = BooleanQuery::new(clauses);

        let searcher = self.reader.searcher();
        let collector = (TopDocs::with_limit(limit).and_offset(offset).order_by_score(), Count);
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
