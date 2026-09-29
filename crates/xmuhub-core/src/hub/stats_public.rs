//! The public 统计 page: totals, the courses downloaded most, with the most files and with the
//! most new files lately, and new files per week. Ranked by course only, never by person, so
//! nobody is invited to pad their numbers.

use std::collections::HashMap;

use serde::Serialize;

use super::{Hub, State};
use crate::model::*;

const TOP: usize = 10;
const WEEKS: i64 = 26;

#[derive(Debug, Clone, Serialize)]
pub struct CourseStat {
    pub id: Id,
    pub name: String,
    /// 学院 / 分组 names above it.
    pub path: String,
    pub value: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteStats {
    pub files: usize,
    pub courses: usize,
    pub downloads: u64,
    pub users: usize,
    pub bytes: u64,
    pub most_downloaded: Vec<CourseStat>,
    pub most_files: Vec<CourseStat>,
    /// Courses with the most new files in the last 30 days.
    pub recently_added: Vec<CourseStat>,
    /// New public files per week, oldest week first; `week_start` is its Unix time.
    pub weekly: Vec<(i64, u64)>,
    pub at: i64,
}

impl Hub {
    pub fn site_stats(&self) -> SiteStats {
        let st = self.st.read();
        let t = now();
        let published: Vec<&Resource> = st.resources.values().filter(|r| r.status == Status::Published).collect();
        let (mut downloads, mut files, mut recent) = (HashMap::<Id, u64>::new(), HashMap::<Id, u64>::new(), HashMap::<Id, u64>::new());
        let week0 = (t / (7 * 86400) - (WEEKS - 1)) * 7 * 86400;
        let mut weekly = vec![0u64; WEEKS as usize];
        for r in &published {
            *downloads.entry(r.node).or_default() += r.downloads;
            *files.entry(r.node).or_default() += 1;
            if t - r.created_at <= 30 * 86400 {
                *recent.entry(r.node).or_default() += 1;
            }
            if r.created_at >= week0 {
                let w = ((r.created_at - week0) / (7 * 86400)) as usize;
                if let Some(x) = weekly.get_mut(w) {
                    *x += 1;
                }
            }
        }
        let top = |m: HashMap<Id, u64>| -> Vec<CourseStat> {
            let mut v: Vec<(Id, u64)> = m.into_iter().filter(|(id, x)| *x > 0 && st.inbox != Some(*id)).collect();
            v.sort_by_key(|(id, x)| (std::cmp::Reverse(*x), *id));
            v.into_iter().take(TOP).filter_map(|(id, value)| course_stat(&st, id, value)).collect()
        };
        SiteStats {
            files: published.len(),
            courses: files.len(),
            downloads: published.iter().map(|r| r.downloads).sum(),
            users: st.users.len(),
            bytes: published.iter().map(|r| r.size).sum(),
            most_downloaded: top(downloads),
            most_files: top(files),
            recently_added: top(recent),
            weekly: weekly.into_iter().enumerate().map(|(i, n)| (week0 + i as i64 * 7 * 86400, n)).collect(),
            at: t,
        }
    }
}

fn course_stat(st: &State, id: Id, value: u64) -> Option<CourseStat> {
    let n = st.nodes.get(&id)?;
    let path: Vec<&str> = st.ancestors(id).iter().filter_map(|a| st.nodes.get(a).map(|n| n.name.as_str())).collect();
    Some(CourseStat { id, name: n.name.clone(), path: path.join(" / "), value })
}
