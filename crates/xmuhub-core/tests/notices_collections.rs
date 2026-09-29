//! 关注 and 站内提醒, 收藏夹 (shared only after another reviewer approves), and the public 统计.

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
    let n = h.create_node(v, NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap();
    n.id
}

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "卷子.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let input = ResourceInput { node, course: String::new(), time: "2023".into(), type_word: "期末试卷".into(), tag: String::new(), paper: String::new(), with_answer: false, extra: String::from_utf8_lossy(&content[..4.min(content.len())]).into(), note: String::new(), subtitle: None, major: None };
    h.create_resource(v, plan.upload_id, input, AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn follows_notices_and_collections() {
    let dir = std::env::temp_dir().join(format!("xmuhub-notices-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let second = register(&h, "r@example.invalid", "reviewer");
    h.update_user(staff, second.id, Some(Level::Reviewer), None).unwrap();
    let second = h.user(second.id).unwrap();
    let reviewer = Viewer { user: Some(&second) };
    let student = register(&h, "s@example.invalid", "student");
    let me = Viewer { user: Some(&student) };
    let fan = register(&h, "f@example.invalid", "fan");
    let them = Viewer { user: Some(&fan) };

    let section = node(&h, staff, None, "section", "专业课");
    let college = node(&h, staff, Some(section), "group", "信息学院");
    let course = node(&h, staff, Some(college), "course", "数据结构");
    for n in [section, college, course] {
        h.update_node(reviewer, n, NodePatch { approve: true, ..Default::default() }).unwrap();
    }

    // Following: a course, or a whole college; not a section.
    assert!(h.follow(them, section, true).is_err());
    assert!(h.follow(them, college, true).unwrap());
    assert!(h.is_following(fan.id, college));
    assert_eq!(h.following(them).unwrap()[0].0.id, college);

    // A student's upload: nothing until it's approved; then the uploader and followers hear.
    let a = upload(&h, &dir, me, course, b"aaaa one").await;
    assert_eq!(h.notices(them, 50).unwrap().unread, 0);
    h.review(staff, a, "approve", "").unwrap();
    let n = h.notices(them, 50).unwrap();
    assert_eq!((n.unread, n.items[0].kind.as_str()), (1, "course"));
    assert!(n.items[0].text.contains("数据结构"));
    assert_eq!(n.items[0].link, format!("/r/{a}"));
    assert_eq!(h.notices(me, 50).unwrap().items[0].kind, "approved");
    // The same day's next files there are the same reminder, with a count.
    let b = upload(&h, &dir, me, course, b"bbbb two").await;
    h.review(staff, b, "approve", "").unwrap();
    let n = h.notices(them, 50).unwrap();
    assert_eq!((n.unread, n.items[0].count), (1, 2));
    assert_eq!(n.items[0].link, format!("/n/{course}"));
    assert_eq!(h.notices(me, 50).unwrap().items.len(), 1, "approvals merge too");
    let c = upload(&h, &dir, me, course, b"cccc bad").await;
    h.review(staff, c, "reject", "看不清").unwrap();
    assert!(h.notices(me, 50).unwrap().items[0].text.contains("看不清"));
    h.read_notices(them, None).unwrap();
    assert_eq!(h.notices(them, 50).unwrap().unread, 0);
    assert!(h.notices(Viewer { user: None }, 50).is_err());

    // 收藏夹: private, shared only after another reviewer approves; renaming re-reviews.
    let list = h.create_collection(me, "期末复习", "数据结构").unwrap();
    assert!(h.collect(me, list.id, c, true).is_err(), "only public files");
    h.collect(me, list.id, a, true).unwrap();
    h.collect(me, list.id, b, true).unwrap();
    assert_eq!(h.collections_with(me, a).unwrap(), vec![list.id]);
    assert!(h.collect(them, list.id, a, false).is_err(), "not their list");
    assert!(h.collection(them, list.id).is_err(), "private");
    h.share_collection(me, list.id, true).unwrap();
    assert!(h.collection(them, list.id).is_err(), "not before review");
    assert!(h.review_collection(me, list.id, true, "").is_err());
    assert!(h.review_collection(reviewer, list.id, false, "").is_err(), "a reason to turn it down");
    h.review_collection(reviewer, list.id, true, "").unwrap();
    let (view, items) = h.collection(them, list.id).unwrap();
    assert_eq!((view.status.as_str(), items.len()), ("public", 2));
    assert_eq!(h.public_collections(them, 10).len(), 1);
    assert_eq!(h.notices(me, 50).unwrap().items[0].kind, "collection");
    h.update_collection(me, list.id, "期末复习（新）", "").unwrap();
    assert!(h.collection(them, list.id).is_err(), "renamed: reviewed again");
    // A file taken down: its owner sees it gone, others don't see it.
    h.review_collection(reviewer, list.id, true, "").unwrap();
    h.review(staff, b, "remove", "").unwrap();
    assert_eq!(h.collection(them, list.id).unwrap().1.len(), 1);
    assert!(h.collection(me, list.id).unwrap().1.iter().any(|(id, hit)| *id == b && hit.is_none()));

    // 统计: public numbers by course.
    let s = h.site_stats();
    assert_eq!((s.files, s.courses), (1, 1));
    assert_eq!(s.most_files[0].name, "数据结构");
    assert_eq!(s.most_files[0].path, "专业课 / 信息学院");
    assert_eq!(s.weekly.len(), 26);
    assert_eq!(s.weekly.last().unwrap().1, 1);

    // Everything survives a restart.
    drop(h);
    let h = open(&dir);
    assert!(h.is_following(fan.id, college));
    assert_eq!(h.notices(Viewer { user: Some(&fan) }, 50).unwrap().items.len(), 1);
    assert_eq!(h.my_collections(Viewer { user: Some(&student) }).unwrap()[0].title, "期末复习（新）");
    h.delete_collection(Viewer { user: Some(&student) }, list.id).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
