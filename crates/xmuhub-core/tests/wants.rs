//! 求资料 posts (reviewed before they are public, capped per user) and the site announcement.

mod common;

use common::test_password;

use std::sync::Arc;

use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{CodePurpose, Limits, NodeInput, Registration, Viewer, WantFilter};
use xmuhub_core::model::{Level, User, WantStatus};
use xmuhub_core::storage::Storage;
use xmuhub_core::storage::local::LocalBackend;

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn open(tag: &str) -> (Hub, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("xmuhub-{tag}-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    (Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap(), dir)
}

#[tokio::test]
async fn wants_are_reviewed_then_answered() {
    let (h, dir) = open("wants");
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let student = register(&h, "s@example.invalid", "student");
    let me = Viewer { user: Some(&student) };
    let other = register(&h, "o@example.invalid", "other");
    let them = Viewer { user: Some(&other) };
    let guest = Viewer { user: None };
    let input = NodeInput { parent: None, kind: "section".into(), code: String::new(), name: "专业课".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 };
    let course = h.create_node(staff, input).unwrap().id;

    assert!(h.add_want(guest, "想要微积分期末", "", None).is_err(), "signed-in only");
    let w = h.add_want(me, "想要数据结构 2023 期末", "最好带答案", Some(course)).unwrap();
    assert_eq!(w.status, "pending");
    // Waiting for review: only the author and staff see it.
    assert!(h.want(them, w.id).is_err());
    assert!(h.want(guest, w.id).is_err());
    assert!(h.wants(guest, WantFilter::Open, None).unwrap().is_empty());
    assert!(h.wants(me, WantFilter::Pending, None).is_err(), "the review queue is staff-only");
    assert_eq!(h.wants(staff, WantFilter::Pending, None).unwrap().len(), 1);
    assert!(h.reply_want(them, w.id, "我有", None).is_err());
    assert!(h.review_want(me, w.id, true, "").is_err(), "authors can't approve their own");

    h.review_want(staff, w.id, true, "").unwrap();
    let open = h.wants(guest, WantFilter::Open, Some(course)).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].author.nickname, "student");

    // 「我也要」, replies, and marking it found.
    assert_eq!(h.vote_want(them, w.id, true).unwrap(), 1);
    assert_eq!(h.vote_want(them, w.id, true).unwrap(), 1, "one vote per person");
    h.reply_want(them, w.id, "群文件里有", None).unwrap();
    assert!(h.reply_want(them, w.id, "再来一条", None).is_err(), "replies are spaced out");
    let (view, replies) = h.want(guest, w.id).unwrap();
    assert_eq!((view.votes, view.replies, replies.len()), (1, 1, 1));
    assert!(!replies[0].can_delete);
    assert!(h.resolve_want(them, w.id, WantStatus::Found, None).is_err(), "only the author or staff");
    h.resolve_want(me, w.id, WantStatus::Found, None).unwrap();
    assert!(h.wants(guest, WantFilter::Open, None).unwrap().is_empty());
    assert_eq!(h.wants(guest, WantFilter::Found, None).unwrap().len(), 1);
    h.delete_want_reply(staff, replies[0].id).unwrap();
    assert_eq!(h.want(guest, w.id).unwrap().1.len(), 0);

    // A rejected post stays with its author, with the reason.
    let bad = h.add_want(me, "广告广告", "", None).unwrap();
    h.review_want(staff, bad.id, false, "和资料无关").unwrap();
    assert!(h.want(guest, bad.id).is_err());
    let mine = h.wants(me, WantFilter::Mine, None).unwrap();
    assert_eq!(mine.iter().find(|x| x.id == bad.id).unwrap().review_note, "和资料无关");

    // Reviewers' posts wait for another reviewer too; admins' don't. A user can't flood the queue.
    let rev = register(&h, "r@example.invalid", "reviewer");
    h.update_user(staff, rev.id, Some(Level::Reviewer), None).unwrap();
    let rev = h.user(rev.id).unwrap();
    let reviewer = Viewer { user: Some(&rev) };
    let own = h.add_want(reviewer, "征集马原期末", "", None).unwrap();
    assert_eq!(own.status, "pending");
    assert!(h.review_want(reviewer, own.id, true, "").is_err(), "not their own");
    h.review_want(staff, own.id, true, "").unwrap();
    assert_eq!(h.add_want(staff, "征集毛概期末", "", None).unwrap().status, "open");
    for i in 0..3 {
        h.add_want(me, &format!("资料 {i}"), "", None).unwrap();
    }
    assert!(h.add_want(me, "资料 3", "", None).is_err(), "posts per day");

    // State survives a restart.
    drop(h);
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db, storage, Limits::default(), vec![]).unwrap();
    let (view, _) = h.want(guest, w.id).unwrap();
    assert_eq!((view.status, view.votes), ("found", 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn announcement_defaults_and_is_admin_only() {
    common::test_site();
    let (h, dir) = open("announce");
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "s@example.invalid", "student");
    assert!(h.announcement().text.contains("厦门大学"), "a default until an admin sets one");
    assert!(h.set_announcement(Viewer { user: Some(&student) }, "hi").is_err());
    h.set_announcement(Viewer { user: Some(&admin) }, "").unwrap();
    assert_eq!(h.announcement().text, "", "empty hides it");
    h.set_announcement(Viewer { user: Some(&admin) }, "考试周资料更新中").unwrap();
    assert_eq!(h.announcement().text, "考试周资料更新中");
    let _ = std::fs::remove_dir_all(dir);
}
