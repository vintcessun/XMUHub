//! 合集: a set of files of one course in a set order, proposed by anyone signed in and shown
//! only once a reviewer who didn't propose it approves (admins are exempt).

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, NodePatch, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, Level, User};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::{Receipt, Storage};

fn open(dir: &std::path::Path) -> Hub {
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap()
}

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str) -> Id {
    h.create_node(v, NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id
}

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "讲义.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let input = ResourceInput { node, course: String::new(), time: "2023".into(), type_word: "课件".into(), tag: String::new(), paper: String::new(), with_answer: false, extra: String::from_utf8_lossy(content).into(), note: String::new(), subtitle: None, major: None };
    h.create_resource(v, plan.upload_id, input, AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn series_are_proposed_then_reviewed() {
    let dir = std::env::temp_dir().join(format!("xmuhub-series-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let rev = register(&h, "r@example.invalid", "reviewer");
    h.update_user(staff, rev.id, Some(Level::Reviewer), None).unwrap();
    let rev = h.user(rev.id).unwrap();
    let reviewer = Viewer { user: Some(&rev) };
    let student = register(&h, "s@example.invalid", "student");
    let me = Viewer { user: Some(&student) };
    let guest = Viewer { user: None };

    let section = node(&h, staff, None, "section", "专业课");
    let course = node(&h, staff, Some(section), "course", "数据结构");
    let other = node(&h, staff, Some(section), "course", "操作系统");
    h.update_node(reviewer, course, NodePatch { approve: true, ..Default::default() }).ok();
    let mut files = Vec::new();
    for i in 1..=4 {
        files.push(upload(&h, &dir, staff, course, format!("第{i}讲").as_bytes()).await);
    }
    let elsewhere = upload(&h, &dir, staff, other, b"elsewhere").await;
    let hidden = upload(&h, &dir, reviewer, course, b"pending one").await;

    // Checks on what goes in.
    assert!(h.propose_series(guest, None, course, "讲义", &files).is_err(), "signed-in only");
    assert!(h.propose_series(me, None, course, "讲义", &files[..1]).is_err(), "two files at least");
    assert!(h.propose_series(me, None, course, "", &files).is_err(), "needs a name");
    assert!(h.propose_series(me, None, course, "讲义", &[files[0], elsewhere]).is_err(), "one course");
    assert!(h.propose_series(me, None, course, "讲义", &[files[0], hidden]).is_err(), "someone else's unpublished file");
    assert!(h.propose_series(me, None, section, "讲义", &files).is_err(), "not on a section");

    // A student's proposal: invisible until a reviewer approves it.
    let order = [files[2], files[0], files[1]];
    let s = h.propose_series(me, None, course, "数据结构讲义", &order).unwrap();
    assert_eq!(s.status, "new");
    assert!(s.draft.as_ref().unwrap().mine);
    assert!(h.node_series(guest, course).is_empty());
    assert!(h.series_of(guest, files[0]).is_none());
    assert!(h.series(guest, s.id).is_err());
    assert!(h.propose_series(me, None, course, "又一个", &[files[0], files[3]]).is_err(), "a file is in one 合集");
    assert_eq!(h.pending_series(reviewer).unwrap().len(), 1);
    assert!(h.pending_series(me).is_err());
    assert!(h.review_series(me, s.id, true, "").is_err(), "students can't review");
    assert!(h.review_series(reviewer, s.id, false, "").is_err(), "a reason to turn down");
    h.review_series(reviewer, s.id, true, "").unwrap();

    let live = h.node_series(guest, course);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].items.iter().map(|i| i.id).collect::<Vec<_>>(), order.to_vec(), "kept in the proposed order");
    assert!(live[0].draft.is_none());
    assert_eq!(h.series_of(guest, files[1]).unwrap().title, "数据结构讲义");
    assert!(h.series_of(guest, files[3]).is_none());
    assert_eq!(h.notices(me, 50).unwrap().items[0].kind, "series");

    // A change waits while the approved version stays up; only one change at a time.
    let longer = [files[0], files[1], files[2], files[3]];
    h.propose_series(me, Some(s.id), course, "数据结构讲义（全）", &longer).unwrap();
    assert_eq!(h.node_series(guest, course)[0].items.len(), 3);
    assert!(h.node_series(guest, course)[0].draft.is_none(), "drafts are for their author and staff");
    assert!(h.propose_series(reviewer, Some(s.id), course, "别的", &files).is_err(), "someone else's change is waiting");
    h.review_series(reviewer, s.id, false, "标题不用改").unwrap();
    assert_eq!(h.node_series(guest, course)[0].title, "数据结构讲义");
    h.propose_series(me, Some(s.id), course, "数据结构讲义", &longer).unwrap();
    h.review_series(reviewer, s.id, true, "").unwrap();
    assert_eq!(h.series_of(guest, files[3]).unwrap().items.len(), 4);

    // Reviewers don't pass their own proposals; admins' apply at once.
    h.propose_series(reviewer, Some(s.id), course, "讲义", &files[..2]).unwrap();
    assert!(h.review_series(reviewer, s.id, true, "").is_err(), "not their own");
    h.review_series(staff, s.id, true, "").unwrap();
    assert_eq!(h.series_of(guest, files[0]).unwrap().items.len(), 2);
    let direct = h.propose_series(staff, Some(s.id), course, "讲义", &[]).unwrap();
    assert_eq!(direct.status, "closed", "an empty list 解散 the 合集");
    assert!(h.node_series(guest, course).is_empty());

    // Survives a restart.
    drop(h);
    let h = open(&dir);
    assert_eq!(h.series(staff, s.id).unwrap().status, "closed");
    let _ = std::fs::remove_dir_all(dir);
}
