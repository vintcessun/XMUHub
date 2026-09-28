//! Review batches and questions to uploaders.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, Level, User};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::{Receipt, Storage};

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: "password123".into(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str) -> Id {
    let input = NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 };
    h.create_node(v, input).unwrap().id
}

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, n: usize) -> Id {
    let content = format!("file number {n}").into_bytes();
    let sha = hex::encode(Sha256::digest(&content));
    let plan = h.begin_upload(v, "卷子.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), &content).unwrap();
    assert!(h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap());
    let input = ResourceInput {
        node,
        course: String::new(),
        time: format!("{}", 2000 + n),
        type_word: "期末试卷".into(),
        tag: String::new(),
        paper: String::new(),
        with_answer: false,
        extra: String::new(),
        note: String::new(),
        subtitle: None,
    };
    h.create_resource(v, plan.upload_id, input, AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn batches_split_the_queue() {
    let dir = std::env::temp_dir().join(format!("xmuhub-batches-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap();

    let admin = register(&h, "a@example.invalid", "admin");
    let rev1 = register(&h, "r1@example.invalid", "rev1");
    let rev2 = register(&h, "r2@example.invalid", "rev2");
    let student = register(&h, "s@example.invalid", "student");
    let student2 = register(&h, "t@example.invalid", "student2");
    let staff = Viewer { user: Some(&admin) };
    h.update_user(staff, rev1.id, Some(Level::Reviewer), None).unwrap();
    h.update_user(staff, rev2.id, Some(Level::Reviewer), None).unwrap();
    let (rev1, rev2) = (h.user(rev1.id).unwrap(), h.user(rev2.id).unwrap());
    let (r1, r2, me) = (Viewer { user: Some(&rev1) }, Viewer { user: Some(&rev2) }, Viewer { user: Some(&student) });

    let section = node(&h, staff, None, "section", "专业课");
    let info = node(&h, staff, Some(section), "group", "信息学院");
    let math = node(&h, staff, Some(section), "group", "数学科学学院");
    let ds = node(&h, staff, Some(info), "course", "数据结构");
    let calc = node(&h, staff, Some(math), "course", "数学分析");
    let mut ids = Vec::new();
    // (Two uploaders: a contributor may send 60 files a day.)
    let other = Viewer { user: Some(&student2) };
    for n in 0..60 {
        ids.push(upload(&h, &dir, if n < 30 { me } else { other }, ds, n).await);
    }
    for n in 60..70 {
        upload(&h, &dir, other, calc, n).await;
    }

    // Two reviewers get disjoint batches of at most 50; the rest stays in the pool.
    let b1: Vec<Id> = h.review_batch(r1, None, true).unwrap().iter().map(|(r, _)| r.id).collect();
    let b2: Vec<Id> = h.review_batch(r2, None, true).unwrap().iter().map(|(r, _)| r.id).collect();
    assert_eq!(b1.len(), 50);
    assert_eq!(b2.len(), 20);
    assert!(b1.iter().all(|id| !b2.contains(id)));
    assert_eq!(h.review_pool_size(None), 0);

    // No new batch until this one is done; reviewing shrinks it.
    assert_eq!(h.review_batch(r1, None, true).unwrap().len(), 50);
    h.review(r1, b1[0], "approve", "").unwrap();
    assert_eq!(h.review_batch(r1, None, false).unwrap().len(), 49);

    // 「不懂」 hands a file back and it never comes back to the same reviewer.
    h.skip_in_batch(r1, b1[1]).unwrap();
    assert_eq!(h.review_pool_size(None), 1);
    h.release_batch(r1).unwrap();
    let again: Vec<Id> = h.review_batch(r1, None, true).unwrap().iter().map(|(r, _)| r.id).collect();
    assert!(!again.contains(&b1[1]));
    assert_eq!(again.len(), 48, "the first batch less the reviewed and the skipped file");

    // Asking the uploader takes the file out until it is answered.
    let asked = again[0];
    let q = h.ask_uploader(r1, asked, "这是哪一年的卷子？").unwrap();
    assert!(!h.review_batch(r1, None, false).unwrap().iter().any(|(r, _)| r.id == asked));
    assert_eq!(h.my_open_questions(me).unwrap().len(), 1);
    assert!(h.answer_question(r2, q.id, "不是我传的").is_err(), "only the uploader answers");
    h.answer_question(me, q.id, "2019 年的").unwrap();
    assert!(h.my_open_questions(me).unwrap().is_empty());
    assert_eq!(h.questions_about(r2, asked)[0].answer, "2019 年的");

    // A reviewer can take only their own college's files.
    h.release_batch(r1).unwrap();
    h.release_batch(r2).unwrap();
    let math_only = h.review_batch(r2, Some(math), true).unwrap();
    assert_eq!(math_only.len(), 10);
    assert!(math_only.iter().all(|(r, _)| r.node == calc));

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
