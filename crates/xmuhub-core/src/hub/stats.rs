//! Daily activity for the admin dashboard: submissions, review decisions, library size.

use serde::Serialize;

use super::{Hub, Viewer};
use crate::error::Result;
use crate::model::*;

/// Days are counted in China Standard Time.
const TZ: i64 = 8 * 3600;

#[derive(Debug, Clone, Serialize)]
pub struct DayStats {
    /// "2026-09-24"
    pub day: String,
    /// Resources created that day (uploads and imports).
    pub submitted: usize,
    /// Review decisions recorded that day.
    pub reviewed: usize,
    /// Published resources at the end of that day (by publish-or-create time).
    pub total: usize,
}

fn day_index(t: i64) -> i64 {
    (t + TZ).div_euclid(86400)
}

fn day_label(idx: i64) -> String {
    // Civil-from-days (Howard Hinnant), for the CST day index.
    let z = idx + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

impl Hub {
    /// The last `days` days, oldest first.
    pub fn daily_stats(&self, actor: Viewer, days: usize) -> Result<Vec<DayStats>> {
        actor.at_least(Level::Reviewer)?;
        let days = days.clamp(7, 365) as i64;
        let st = self.st.read();
        let today = day_index(now());
        let first = today - days + 1;
        let n = days as usize;
        let (mut submitted, mut reviewed, mut added) = (vec![0usize; n], vec![0usize; n], vec![0usize; n]);
        let mut before = 0usize;
        for r in st.resources.values() {
            let d = day_index(r.created_at);
            if d >= first && d <= today {
                submitted[(d - first) as usize] += 1;
            }
            if r.status == Status::Published {
                // Published items count from creation (approval time isn't stored for old items).
                if d < first { before += 1 } else if d <= today { added[(d - first) as usize] += 1 }
            }
        }
        for e in &st.reviews {
            let d = day_index(e.at);
            if d >= first && d <= today {
                reviewed[(d - first) as usize] += 1;
            }
        }
        let mut total = before;
        Ok((0..n)
            .map(|i| {
                total += added[i];
                DayStats { day: day_label(first + i as i64), submitted: submitted[i], reviewed: reviewed[i], total }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn labels() {
        assert_eq!(super::day_label(0), "1970-01-01");
        assert_eq!(super::day_label(super::day_index(1_790_226_312)), "2026-09-24");
    }
}
