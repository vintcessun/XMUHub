//! Uploaders editing their own files: published edits go back to review.

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
    h.register(Registration { email, code, password: "password123".into(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str) -> Id {
    let input = NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0 };
    h.create_node(v, input).unwrap().id
}

fn input(node: Id, time: &str) -> ResourceInput {
    ResourceInput {
        node,
        course: String::new(),
        time: time.into(),
        type_word: "期末试卷".into(),
        tag: String::new(),
        paper: String::new(),
        with_answer: false,
        extra: String::new(),
        note: String::new(),
        subtitle: None,
    }
}

/// Uploads `content` through the local backend and files it under `node`.
async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "卷子.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    assert!(h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap());
    h.create_resource(v, plan.upload_id, input(node, "2023-2024秋"), AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn uploader_edits_go_back_to_review() {
    let dir = std::env::temp_dir().join(format!("xmuhub-edit-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap();

    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "b@example.invalid", "student");
    let other = register(&h, "c@example.invalid", "other");
    let staff = Viewer { user: Some(&admin) };
    let me = Viewer { user: Some(&student) };
    let section = node(&h, staff, None, "section", "专业课");
    let college = node(&h, staff, Some(section), "group", "信息学院");
    let course = node(&h, staff, Some(college), "course", "数据结构");
    let course2 = node(&h, staff, Some(college), "course", "操作系统");

    // A contributor's upload waits; approved, it is public.
    let id = upload(&h, &dir, me, course, b"first file").await;
    assert_eq!(h.resource(staff, id).unwrap().0.status, Status::Pending);
    h.review(staff, id, "approve", "").unwrap();

    // Someone else can't edit it; a section is not a valid place for a file.
    assert!(h.update_resource(Viewer { user: Some(&other) }, id, input(course, "2024春"), AdminExtras::default()).is_err());
    assert!(h.update_resource(me, id, input(section, "2024春"), AdminExtras::default()).is_err());

    // The uploader's edit (here also moving it) sends it back to review, and is logged
    // without counting as review work.
    let r = h.update_resource(me, id, input(course2, "2024春"), AdminExtras::default()).unwrap();
    assert_eq!(r.status, Status::Pending);
    assert_eq!(r.node, course2);
    assert_eq!(r.name.time, "2024春");
    assert_eq!(h.review_log(staff, Some(id), 10).unwrap()[0].action, "edit");
    let days = h.daily_stats(staff, 1).unwrap();
    assert_eq!(days.last().map(|d| d.reviewed), Some(1), "{:?}", days.iter().map(|d| (d.day.clone(), d.reviewed)).collect::<Vec<_>>());

    // Trusted uploaders stay public and are flagged for a re-check.
    h.update_user(staff, student.id, Some(Level::Trusted), None).unwrap();
    let trusted = h.user(student.id).unwrap();
    h.review(staff, id, "approve", "").unwrap();
    let r = h.update_resource(Viewer { user: Some(&trusted) }, id, input(course2, "2023"), AdminExtras::default()).unwrap();
    assert_eq!(r.status, Status::Published);
    assert!(r.needs_review);

    // Rejected files (their blob is released) can't be edited back into review.
    h.review(staff, id, "reject", "").unwrap();
    assert!(h.update_resource(Viewer { user: Some(&trusted) }, id, input(course2, "2023"), AdminExtras::default()).is_err());

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
