//! 站内提醒 and 关注: a user follows courses (or a whole college) and hears about files newly
//! public there; they also hear about decisions on what they submitted and replies to their
//! 求资料 posts. Only on the site: no mail, no push.

use serde::Serialize;

use super::{Hub, State, Viewer};
use crate::db::Tx;
use crate::error::{Error, Result, bad};
use crate::model::*;

/// How long a reminder is kept, and how many one person keeps at most.
const KEEP_SECS: i64 = 90 * 86400;
const KEEP_MAX: usize = 200;
/// Courses / colleges one person may follow.
const FOLLOW_MAX: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct NoticeList {
    pub items: Vec<Notice>,
    pub unread: usize,
}

impl State {
    /// Adds a reminder for `user`. New files in the same course on the same day (and the
    /// same day's approvals of their uploads), not yet read, become one reminder with a count.
    pub(super) fn notify(&mut self, tx: &Tx, user: Id, kind: &str, node: Option<Id>, text: String, link: String) -> Result<()> {
        let t = now();
        let list = self.notices.entry(user).or_default();
        if (kind == "course" || kind == "approved")
            && let Some(n) = list.iter_mut().rev().find(|n| n.kind == kind && n.node == node && !n.read && t - n.created_at < 86400)
        {
            n.count += 1;
            n.text = text;
            n.link = link;
            n.created_at = t;
            tx.put_notice(n)?;
            return Ok(());
        }
        let n = Notice { id: 0, user, kind: kind.into(), text, link, node, count: 1, created_at: t, read: false };
        let id = self.next_id(tx)?;
        let n = Notice { id, ..n };
        tx.put_notice(&n)?;
        let list = self.notices.entry(user).or_default();
        list.push(n);
        // Old and surplus reminders go.
        let cut = list.iter().take_while(|n| t - n.created_at > KEEP_SECS).count().max(list.len().saturating_sub(KEEP_MAX));
        for n in list.drain(..cut) {
            tx.del_notice(n.id)?;
        }
        Ok(())
    }

    /// A file just became public: everyone following its course or a college / group above
    /// it hears about it (not its uploader).
    pub(super) fn tell_followers(&mut self, tx: &Tx, r: &Resource) -> Result<()> {
        let mut nodes = vec![r.node];
        nodes.extend(self.ancestors(r.node));
        let mut users: Vec<Id> = nodes.iter().filter_map(|n| self.followers.get(n)).flatten().copied().filter(|u| *u != r.uploader).collect();
        users.sort_unstable();
        users.dedup();
        if users.is_empty() {
            return Ok(());
        }
        let course = self.nodes.get(&r.node).map(|n| n.name.clone()).unwrap_or_default();
        let file = r.name.stem();
        for u in users {
            // The text a merged reminder ends up with is the count's.
            let same_day = self.notices.get(&u).and_then(|l| {
                l.iter().rev().find(|n| n.kind == "course" && n.node == Some(r.node) && !n.read && now() - n.created_at < 86400).map(|n| n.count)
            });
            let (text, link) = match same_day {
                Some(c) => (format!("你关注的「{course}」今天新增了 {} 份资料", c + 1), format!("/n/{}", r.node)),
                None => (format!("你关注的「{course}」新增资料：{file}"), format!("/r/{}", r.id)),
            };
            self.notify(tx, u, "course", Some(r.node), text, link)?;
        }
        Ok(())
    }
}

impl State {
    /// A reviewer decided on someone's file: its uploader hears about it. Approvals on the
    /// same day are one reminder with a count; the owner's import account hears nothing.
    pub(super) fn tell_uploader(&mut self, tx: &Tx, r: &Resource, action: &str) -> Result<()> {
        if self.users.get(&r.uploader).is_none_or(|u| u.email == super::SYSTEM_EMAIL) {
            return Ok(());
        }
        let name = r.name.stem();
        let link = format!("/r/{}", r.id);
        match action {
            "approve" | "restore" => {
                let same_day = self.notices.get(&r.uploader).and_then(|l| {
                    l.iter().rev().find(|n| n.kind == "approved" && !n.read && now() - n.created_at < 86400).map(|n| n.count)
                });
                let (text, link) = match same_day {
                    Some(c) => (format!("你上传的 {} 份资料今天通过了审核，已公开", c + 1), "/me".to_string()),
                    None => (format!("你上传的「{name}」通过了审核，已公开"), link),
                };
                self.notify(tx, r.uploader, "approved", None, text, link)
            }
            "reject" => {
                let why = if r.review_note.is_empty() { String::new() } else { format!("：{}", r.review_note) };
                self.notify(tx, r.uploader, "upload", None, format!("你上传的「{name}」没有通过审核{why}"), link)
            }
            "remove" => {
                let why = if r.review_note.is_empty() { String::new() } else { format!("：{}", r.review_note) };
                self.notify(tx, r.uploader, "upload", None, format!("你上传的「{name}」被下架了{why}"), link)
            }
            _ => Ok(()),
        }
    }
}

impl Hub {
    /// The viewer's reminders, newest first, with the unread count.
    pub fn notices(&self, viewer: Viewer, limit: usize) -> Result<NoticeList> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let t = now();
        let all: Vec<&Notice> = st.notices.get(&me).map(|l| l.iter().rev().filter(|n| t - n.created_at <= KEEP_SECS).collect()).unwrap_or_default();
        Ok(NoticeList { unread: all.iter().filter(|n| !n.read).count(), items: all.into_iter().take(limit).cloned().collect() })
    }

    pub fn unread_notices(&self, user: Id) -> usize {
        let t = now();
        self.st.read().notices.get(&user).map_or(0, |l| l.iter().filter(|n| !n.read && t - n.created_at <= KEEP_SECS).count())
    }

    /// Marks one reminder read, or all of them.
    pub fn read_notices(&self, viewer: Viewer, id: Option<Id>) -> Result<()> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let Some(list) = st.notices.get_mut(&me) else { return Ok(()) };
            for n in list.iter_mut().filter(|n| !n.read && id.is_none_or(|id| id == n.id)) {
                n.read = true;
                tx.put_notice(n)?;
            }
            Ok(())
        })
    }

    /// Follows (or stops following) a course, group or college. Returns whether it's followed now.
    pub fn follow(&self, viewer: Viewer, node: Id, on: bool) -> Result<bool> {
        let me = viewer.at_least(Level::Contributor)?.id;
        self.mutate(|st, tx| {
            let (node, section) = st.resolve(node).map(|n| (n.id, n.kind == NodeKind::Section)).ok_or(Error::NotFound("分类"))?;
            if on && section {
                return Err(bad("请关注具体的课程或学院"));
            }
            if on && st.following.get(&me).is_some_and(|s| s.len() >= FOLLOW_MAX && !s.contains(&node)) {
                return Err(Error::TooMany(format!("最多关注 {FOLLOW_MAX} 个课程或学院")));
            }
            tx.put_follow(me, node, on)?;
            if on {
                st.followers.entry(node).or_default().insert(me);
                st.following.entry(me).or_default().insert(node);
            } else {
                st.followers.get_mut(&node).map(|s| s.remove(&me));
                st.following.get_mut(&me).map(|s| s.remove(&node));
            }
            Ok(on)
        })
    }

    pub fn is_following(&self, user: Id, node: Id) -> bool {
        self.st.read().following.get(&user).is_some_and(|s| s.contains(&node))
    }

    /// What the viewer follows, with each node's path.
    pub fn following(&self, viewer: Viewer) -> Result<Vec<(Node, Vec<Node>)>> {
        let me = viewer.at_least(Level::Contributor)?.id;
        let st = self.st.read();
        let mut v: Vec<(Node, Vec<Node>)> = st
            .following
            .get(&me)
            .into_iter()
            .flatten()
            .filter_map(|id| st.nodes.get(id))
            .map(|n| {
                let path: Vec<Node> = st.ancestors(n.id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect();
                (n.clone(), path)
            })
            .collect();
        v.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        Ok(v)
    }
}
