//! 「待整理」 uploads and profile pictures.

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, Level, Status, User};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::{Receipt, Storage};

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str) -> Id {
    let input = NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 };
    h.create_node(v, input).unwrap().id
}

fn input(node: Id, course: &str) -> ResourceInput {
    ResourceInput {
        node,
        course: course.into(),
        time: "2023-2024秋".into(),
        type_word: "期末试卷".into(),
        tag: String::new(),
        paper: String::new(),
        with_answer: false,
        extra: String::new(),
        note: "大一上高数".into(),
        subtitle: None,
        major: None,
    }
}

/// Uploads `content` through the local backend; returns the finished upload id.
async fn send(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, name: &str, mime: &str, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, name, mime, vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    assert!(h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap());
    plan.upload_id
}

fn open(tag: &str) -> (Hub, std::path::PathBuf, Arc<Db>) {
    let dir = std::env::temp_dir().join(format!("xmuhub-{tag}-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    (Hub::open(db.clone(), storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap(), dir, db)
}

#[tokio::test]
async fn inbox_files_wait_to_be_sorted() {
    let (h, dir, db) = open("inbox");
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "b@example.invalid", "student");
    let staff = Viewer { user: Some(&admin) };
    h.update_user(staff, student.id, Some(Level::Trusted), None).unwrap();
    let trusted = h.user(student.id).unwrap();
    let me = Viewer { user: Some(&trusted) };
    let section = node(&h, staff, None, "section", "专业课");
    let course = node(&h, staff, Some(section), "course", "数据结构");

    // One inbox, created on first use, listed nowhere.
    let inbox = h.inbox(me).unwrap();
    assert_eq!(h.inbox(staff).unwrap().id, inbox.id);
    assert!(h.tree().iter().all(|i| i.node.id != inbox.id));
    assert!(h.suggest_nodes("待整理", 10).is_empty());

    // Even a trusted uploader's file waits there, flagged for sorting.
    let up = send(&h, &dir, me, "高数.pdf", "application/pdf", b"unsorted").await;
    let r = h.create_resource(me, up, input(inbox.id, ""), AdminExtras::default()).unwrap();
    assert_eq!(r.status, Status::Pending);
    assert!(r.uncertain && !r.needs_review);
    assert_eq!(r.name.course, "待整理");

    // It can't be approved where it is; moved to its course it takes that course's name.
    assert!(h.review(staff, r.id, "approve", "").is_err());
    let moved = h.update_resource(staff, r.id, input(course, "待整理"), AdminExtras::default()).unwrap();
    assert_eq!(moved.name.course, "数据结构");
    h.review(staff, r.id, "approve", "").unwrap();
    assert_eq!(h.resource(staff, r.id).unwrap().0.status, Status::Published);

    // The inbox survives a restart and still stays out of the tree.
    drop(h);
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db, storage, Limits::default(), vec![]).unwrap();
    assert_eq!(h.inbox_id(), Some(inbox.id));
    assert!(h.tree().iter().all(|i| i.node.id != inbox.id));

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn avatars_wait_for_review() {
    let (h, dir, _db) = open("avatar");
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "b@example.invalid", "student");
    let staff = Viewer { user: Some(&admin) };
    let me = Viewer { user: Some(&student) };

    // Not an image, or too big: refused.
    let doc = send(&h, &dir, me, "a.pdf", "application/pdf", b"not a picture").await;
    assert!(h.set_avatar(me, doc).is_err());
    let big = send(&h, &dir, me, "big.webp", "image/webp", &vec![7u8; 500 * 1024]).await;
    assert!(h.set_avatar(me, big).is_err());

    // A picture waits for review: the owner sees it pending, everyone else sees none.
    let pic = send(&h, &dir, me, "avatar.webp", "image/webp", b"picture one").await;
    h.set_avatar(me, pic).unwrap();
    assert!(h.set_avatar(me, pic).is_err(), "an upload is used once");
    let (cur, pending) = h.own_avatar(student.id);
    assert!(cur.is_empty() && !pending.is_empty());
    assert!(h.avatar_of(student.id).is_empty());
    assert!(h.pending_avatars(me).is_err());
    assert_eq!(h.pending_avatars(staff).unwrap().len(), 1);

    h.review_avatar(staff, student.id, "approve").unwrap();
    assert_eq!(h.avatar_of(student.id), pending);
    assert!(h.pending_avatars(staff).unwrap().is_empty());

    // A rejected replacement leaves the approved one up, and nothing is deleted.
    let pic2 = send(&h, &dir, me, "avatar.webp", "image/webp", b"picture two").await;
    h.set_avatar(me, pic2).unwrap();
    h.review_avatar(staff, student.id, "reject").unwrap();
    assert_eq!(h.avatar_of(student.id), pending);
    assert!(dir.join("files").join(hex::encode(Sha256::digest(b"picture two"))).exists());

    // Staff pictures wait too, for another reviewer; a user can take theirs down.
    let pic3 = send(&h, &dir, staff, "avatar.webp", "image/webp", b"picture three").await;
    h.set_avatar(staff, pic3).unwrap();
    assert!(h.avatar_of(admin.id).is_empty());
    assert!(h.review_avatar(staff, admin.id, "approve").is_err(), "not their own");
    h.clear_avatar(me).unwrap();
    assert!(h.avatar_of(student.id).is_empty());

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
