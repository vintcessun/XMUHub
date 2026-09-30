//! 建议换个分类: a student suggests another course for a published file; another reviewer
//! moves it there or turns it down, and the student hears back.

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, User};
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
async fn move_suggestions_are_reviewed_by_someone_else() {
    let dir = std::env::temp_dir().join(format!("xmuhub-moves-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "s@example.invalid", "student");
    let va = Viewer { user: Some(&admin) };
    let vs = Viewer { user: Some(&student) };
    let section = node(&h, va, None, "section", "专业课");
    let college = node(&h, va, Some(section), "group", "信息学院");
    let wrong = node(&h, va, Some(college), "course", "操作系统");
    let right = node(&h, va, Some(college), "course", "计算机网络");
    let f = upload(&h, &dir, va, wrong, b"slides").await;
    let g = upload(&h, &dir, va, wrong, b"other slides").await;

    // Nonsense is refused: not a course, where it already is, a missing file.
    assert!(h.suggest_move(vs, f, section, "").is_err());
    assert!(h.suggest_move(vs, f, wrong, "").is_err());
    assert!(h.suggest_move(vs, 999_999, right, "").is_err());
    let s = h.suggest_move(vs, f, right, "这是计网的课件").unwrap();
    assert!(h.suggest_move(vs, f, right, "").is_err(), "one pending suggestion per file");
    assert!(h.review_move_suggestion(vs, s.id, true, "").is_err(), "students don't review");
    assert_eq!(h.move_suggestions(va).unwrap().len(), 1);
    assert_eq!(h.move_suggestions(vs).unwrap()[0].to, s.to);

    h.review_move_suggestion(va, s.id, true, "").unwrap();
    assert_eq!(h.resource(va, f).unwrap().0.node, right, "moved");
    assert!(h.review_move_suggestion(va, s.id, true, "").is_err(), "only once");
    assert!(h.move_suggestions(va).unwrap().is_empty());

    let t = h.suggest_move(vs, g, right, "").unwrap();
    assert!(h.review_move_suggestion(va, t.id, false, "").is_err(), "a reason is needed");
    h.review_move_suggestion(va, t.id, false, "就是操作系统的").unwrap();
    assert_eq!(h.resource(va, g).unwrap().0.node, wrong, "stays");
    let got = h.notices(vs, 10).unwrap();
    assert!(got.items.iter().any(|n| n.kind == "move" && n.text.contains("已采纳")));
    assert!(got.items.iter().any(|n| n.kind == "move" && n.text.contains("就是操作系统的")));

    // Kept across restarts.
    drop(h);
    let h = open(&dir);
    assert_eq!(h.move_suggestions(Viewer { user: Some(&student) }).unwrap().len(), 2);
    drop(h);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn review_tabs_count_what_waits_and_feedback_replies_reach_the_sender() {
    let dir = std::env::temp_dir().join(format!("xmuhub-queues-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "s@example.invalid", "student");
    let (va, vs) = (Viewer { user: Some(&admin) }, Viewer { user: Some(&student) });
    let section = node(&h, va, None, "section", "专业课");
    let college = node(&h, va, Some(section), "group", "信息学院");
    let a = node(&h, va, Some(college), "course", "操作系统");
    let b = node(&h, va, Some(college), "course", "计算机网络");
    let f = upload(&h, &dir, va, a, b"slides").await;
    upload(&h, &dir, vs, a, b"waiting").await;
    h.suggest_move(vs, f, b, "").unwrap();
    let fb = h.submit_feedback(vs, "上传太慢了", "", "/upload", "1.1.1.1").unwrap();

    assert!(h.review_counts(vs).is_err(), "reviewers only");
    let c = h.review_counts(va).unwrap();
    assert_eq!((c["queue"], c["moves"], c["feedback"]), (1, 1, 1));

    h.handle_feedback(va, fb.id, "已经改成同时传三个文件了").unwrap();
    assert_eq!(h.review_counts(va).unwrap()["feedback"], 0);
    let got = h.notices(vs, 10).unwrap();
    assert!(got.items.iter().any(|n| n.kind == "feedback" && n.text.contains("同时传三个文件")));
    drop(h);
    let _ = std::fs::remove_dir_all(dir);
}
