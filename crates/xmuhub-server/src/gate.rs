//! 人机验证 for what people send in for others to review: past an hourly allowance per kind
//! (uploads, feedback, reports …) a signed-in account is asked to pass a Cloudflare Turnstile
//! check, after which it is left alone for a while. The allowances are high on purpose:
//! ordinary use never meets the check, only scripts and bulk junk do. Admins are never asked.
//! Counts live in memory only (a restart forgets them, which errs on the user's side).

use std::collections::{HashMap, VecDeque};

use xmuhub_core::model::Id;

const HOUR: i64 = 3600;
/// How long a passed check lets the account go on without asking again.
pub const PASS_SECS: i64 = 6 * HOUR;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Upload,
    Feedback,
    Report,
    ChangeRequest,
    Node,
    Link,
    Move,
    Collection,
    Series,
    Avatar,
}

impl Kind {
    /// Submissions of this kind an account may make per hour without being asked.
    pub fn free_per_hour(self) -> usize {
        match self {
            Kind::Upload => 60,
            Kind::Move => 20,
            Kind::Report | Kind::ChangeRequest | Kind::Node | Kind::Link | Kind::Collection | Kind::Series => 10,
            Kind::Feedback | Kind::Avatar => 5,
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Kind::Upload => "上传文件",
            Kind::Feedback => "提交反馈",
            Kind::Report => "投诉",
            Kind::ChangeRequest => "提交修改 / 删除申请",
            Kind::Node => "申请新课程",
            Kind::Link => "推荐站外链接",
            Kind::Move => "提分类建议",
            Kind::Collection => "公开收藏夹",
            Kind::Series => "提交合集",
            Kind::Avatar => "更换头像",
        }
    }
}

#[derive(Default)]
pub struct Gates {
    recent: HashMap<(Id, Kind), VecDeque<i64>>,
    pass_until: HashMap<Id, i64>,
}

impl Gates {
    /// Counts one more submission at `t`; false when it needs the human check first.
    pub fn admit(&mut self, user: Id, kind: Kind, t: i64) -> bool {
        self.forget_old(t);
        let passed = self.pass_until.get(&user).is_some_and(|&until| until > t);
        let recent = self.recent.entry((user, kind)).or_default();
        if recent.len() >= kind.free_per_hour() && !passed {
            return false;
        }
        recent.push_back(t);
        true
    }

    /// The account passed a check at `t`.
    pub fn pass(&mut self, user: Id, t: i64) {
        self.pass_until.insert(user, t + PASS_SECS);
    }

    fn forget_old(&mut self, t: i64) {
        self.recent.retain(|_, v| {
            while v.front().is_some_and(|&at| t - at >= HOUR) {
                v.pop_front();
            }
            !v.is_empty()
        });
        self.pass_until.retain(|_, &mut until| until > t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_submissions_in_an_hour_need_the_human_check_and_a_pass_lasts() {
        let mut g = Gates::default();
        let t = 1_000_000;
        for i in 0..Kind::Feedback.free_per_hour() as i64 {
            assert!(g.admit(7, Kind::Feedback, t + i), "ordinary use is never asked");
        }
        assert!(!g.admit(7, Kind::Feedback, t + 100), "one more within the hour is");
        assert!(g.admit(7, Kind::Report, t + 100), "each kind counts on its own");
        assert!(g.admit(8, Kind::Feedback, t + 100), "and each account");
        g.pass(7, t + 100);
        assert!(g.admit(7, Kind::Feedback, t + 101) && g.admit(7, Kind::Feedback, t + 200), "a passed check lets it go on");
        // After the pass and the hour, the count starts over and old entries are dropped.
        let later = t + 101 + PASS_SECS;
        assert!(g.admit(7, Kind::Feedback, later));
        g.forget_old(later + HOUR);
        assert!(g.recent.is_empty() && g.pass_until.is_empty());
    }
}
