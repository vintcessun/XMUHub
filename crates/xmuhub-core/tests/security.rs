//! Abuse defences: sign-up checks, sessions, tokens after a password change, nicknames,
//! banned users' comments, no-op edits, re-uploads of refused files, download counting.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, Viewer, needs_captcha};
use xmuhub_core::model::{Id, Level, Status, User};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::{Receipt, Storage};

fn open(tag: &str) -> (Hub, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("xmuhub-{tag}-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    (Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap(), dir)
}

fn register(h: &Hub, email: &str, nick: &str) -> (String, User) {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: "password123".into(), nickname: nick.into() }, "1.1.1.1").unwrap()
}

fn input(node: Id) -> ResourceInput {
    ResourceInput {
        node,
        course: String::new(),
        time: "2023-2024秋".into(),
        type_word: "期末试卷".into(),
        tag: String::new(),
        paper: String::new(),
        with_answer: false,
        extra: String::new(),
        note: String::new(),
        subtitle: None,
    }
}

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "卷子.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    if !plan.dedup {
        std::fs::write(dir.join("files").join(&sha), content).unwrap();
        assert!(h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap());
    }
    h.create_resource(v, plan.upload_id, input(node), AdminExtras::default()).unwrap().id
}

#[test]
fn uncommon_mail_domains_need_a_human_check() {
    for ok in ["a@qq.com", "a@163.com", "a@gmail.com", "a@stu.xmu.edu.cn", "a@xmu.edu.cn", "a@math.xmu.edu.cn", "a@outlook.com"] {
        assert!(!needs_captcha(ok), "{ok}");
    }
    for check in ["a@mailinator.com", "a@example.org", "a+1@gmail.com", "a@xmu.edu.cn.evil.com", "a@fakexmu.edu.cn"] {
        assert!(needs_captcha(check), "{check}");
    }
}

#[test]
fn accounts_are_bounded() {
    let (h, dir) = open("security-accounts");
    let (_, admin) = register(&h, "a@example.invalid", "admin");
    let (_, user) = register(&h, "b@example.invalid", "student");

    // Staff-looking nicknames are for staff.
    let (email, code) = h.request_code("c@example.invalid", CodePurpose::Register, "1.1.1.1").unwrap();
    assert!(h.register(Registration { email, code, password: "password123".into(), nickname: "鹭岛书阁 管理员".into() }, "1.1.1.1").is_err());
    assert!(h.update_profile(&user, "", Some("管理员"), None, None).is_err());
    assert!(h.update_profile(&admin, "", Some("管理员小王"), None, None).is_ok());

    // Signing in over and over keeps only the newest sessions.
    let first = h.login("b@example.invalid", "password123", "2.2.2.2").unwrap().0;
    assert!(h.session_user(&first).is_some());
    let mut last = String::new();
    for _ in 0..21 {
        last = h.login("b@example.invalid", "password123", "2.2.2.2").unwrap().0;
    }
    assert!(h.session_user(&first).is_none(), "oldest session dropped");
    assert!(h.session_user(&last).is_some());

    // A password reset revokes API tokens made with the old password.
    let me = h.user(user.id).unwrap();
    let (token, _) = h.create_token(Viewer { user: Some(&me) }, "script").unwrap();
    assert!(h.token_user(&token).is_some());
    let (email, code) = h.request_code("b@example.invalid", CodePurpose::Reset, "1.1.1.1").unwrap();
    h.reset_password(&email, &code, "newpassword1", "1.1.1.1").unwrap();
    assert!(h.token_user(&token).is_none());

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn content_abuse_is_contained() {
    let (h, dir) = open("security-content");
    let (_, admin) = register(&h, "a@example.invalid", "admin");
    let (_, student) = register(&h, "b@example.invalid", "student");
    let staff = Viewer { user: Some(&admin) };
    h.update_user(staff, student.id, Some(Level::Trusted), None).unwrap();
    let trusted = h.user(student.id).unwrap();
    let me = Viewer { user: Some(&trusted) };
    let section = h.create_node(staff, NodeInput { parent: None, kind: "section".into(), code: String::new(), name: "专业课".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;
    let course = h.create_node(staff, NodeInput { parent: Some(section), kind: "course".into(), code: String::new(), name: "数据结构".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;

    // A trusted upload is public; saving it unchanged starts no review round.
    let id = upload(&h, &dir, me, course, b"a file").await;
    assert_eq!(h.resource(staff, id).unwrap().0.status, Status::Published);
    h.review(staff, id, "approve", "").unwrap();
    let log_before = h.review_log(staff, Some(id), 50).unwrap().len();
    let r = h.update_resource(me, id, input(course), AdminExtras::default()).unwrap();
    assert!(r.status == Status::Published && !r.needs_review);
    assert_eq!(h.review_log(staff, Some(id), 50).unwrap().len(), log_before);

    // Moving a published file into 「待整理」 takes it off the site.
    let inbox = h.inbox(me).unwrap().id;
    let r = h.update_resource(me, id, input(inbox), AdminExtras::default()).unwrap();
    assert_eq!(r.status, Status::Pending);

    // The same bytes as a removed file come back only through review, even when trusted.
    let gone = upload(&h, &dir, me, course, b"refused file").await;
    h.review(staff, gone, "remove", "侵权").unwrap();
    let course2 = h.create_node(staff, NodeInput { parent: Some(section), kind: "course".into(), code: String::new(), name: "操作系统".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;
    let again = upload(&h, &dir, me, course2, b"refused file").await;
    assert_eq!(h.resource(staff, again).unwrap().0.status, Status::Pending);

    // A banned account's comments disappear with it.
    let other = upload(&h, &dir, staff, course, b"commented file").await;
    h.add_comment(me, other, "广告广告广告").unwrap();
    assert_eq!(h.comments(staff, other).unwrap().len(), 1);
    h.update_user(staff, student.id, None, Some(true)).unwrap();
    assert!(h.comments(staff, other).unwrap().is_empty());

    // The same visitor downloading again the same day counts once.
    let before = h.resource(staff, other).unwrap().0.downloads;
    for _ in 0..5 {
        h.download(Viewer { user: None }, other, Some("9.9.9.9")).unwrap();
    }
    assert_eq!(h.resource(staff, other).unwrap().0.downloads, before + 1);

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
