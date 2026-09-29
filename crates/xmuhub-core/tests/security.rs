//! Abuse defences: sign-up checks, sessions, tokens after a password change, nicknames,
//! banned users' comments, no-op edits, re-uploads of refused files, download counting.

mod common;

use common::{other_test_password, test_password};

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, NodePatch, PartSpec, Registration, ResourceInput, Viewer, needs_captcha};
use xmuhub_core::model::{Id, Level, NodeStatus, Status, User};
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
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap()
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
        major: None,
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
    let (signup_session, user) = register(&h, "b@example.invalid", "student");

    // Staff-looking nicknames are for staff.
    let (email, code) = h.request_code("c@example.invalid", CodePurpose::Register, "1.1.1.1").unwrap();
    assert!(h.register(Registration { email, code, password: test_password(), nickname: "鹭岛书阁 管理员".into() }, "1.1.1.1").is_err());
    assert!(h.update_profile(&user, "", Some("管理员"), None, None).is_err());
    assert!(h.update_profile(&admin, "", Some("管理员小王"), None, None).is_ok());

    // Signing in over and over keeps only the newest sessions.
    // (Sessions made within the same second may be dropped in either order, so count them.)
    let mut secrets = vec![signup_session];
    for _ in 0..24 {
        secrets.push(h.login("b@example.invalid", &test_password(), "2.2.2.2").unwrap().0);
    }
    assert_eq!(secrets.iter().filter(|s| h.session_user(s).is_some()).count(), 20);
    assert!(h.session_user(secrets.last().unwrap()).is_some(), "the newest one always stays");

    // A password reset revokes API tokens made with the old password.
    let me = h.user(user.id).unwrap();
    let (token, _) = h.create_token(Viewer { user: Some(&me) }, "script").unwrap();
    assert!(h.token_user(&token).is_some());
    let (email, code) = h.request_code("b@example.invalid", CodePurpose::Reset, "1.1.1.1").unwrap();
    h.reset_password(&email, &code, &other_test_password(), "1.1.1.1").unwrap();
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
    let other = upload(&h, &dir, me, course, b"commented file").await;
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

#[tokio::test]
async fn reviewer_uploads_wait_for_another_reviewer() {
    let (h, dir) = open("staffupload");
    let (_, admin) = register(&h, "a@example.invalid", "admin");
    let (_, second) = register(&h, "r@example.invalid", "reviewer");
    let staff = Viewer { user: Some(&admin) };
    h.update_user(staff, second.id, Some(Level::Reviewer), None).unwrap();
    let second = h.user(second.id).unwrap();
    let reviewer = Viewer { user: Some(&second) };
    let section = h.create_node(staff, NodeInput { parent: None, kind: "section".into(), code: String::new(), name: "专业课".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;
    let course = h.create_node(reviewer, NodeInput { parent: Some(section), kind: "course".into(), code: String::new(), name: "数据结构".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;

    let id = upload(&h, &dir, reviewer, course, b"reviewer's own file").await;
    assert_eq!(h.resource(staff, id).unwrap().0.status, Status::Pending, "no review exemption for reviewers");
    assert!(h.review(reviewer, id, "approve", "").is_err(), "not by its own uploader");
    assert!(h.review_batch(reviewer, None, true).unwrap().is_empty(), "nor handed to them in a batch");
    assert_eq!(h.review_batch(staff, None, true).unwrap().len(), 1);
    h.review(staff, id, "approve", "").unwrap();
    assert_eq!(h.resource(staff, id).unwrap().0.status, Status::Published);

    // Categories reviewers create wait for another reviewer too (approving a file in it counts).
    let course_node = h.node(staff, course).unwrap().0.node;
    assert_eq!(course_node.status, NodeStatus::Active, "confirmed by approving the file in it");
    let other = h.create_node(reviewer, NodeInput { parent: Some(section), kind: "course".into(), code: String::new(), name: "操作系统".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;
    assert_eq!(h.node(staff, other).unwrap().0.node.status, NodeStatus::Pending);
    assert!(h.update_node(reviewer, other, NodePatch { approve: true, ..Default::default() }).is_err(), "not their own");
    h.update_node(staff, other, NodePatch { approve: true, ..Default::default() }).unwrap();

    // Editing their own published file sends it back to review; so does asking to publish directly.
    let mut changed = input(course);
    changed.extra = "改过".into();
    h.update_resource(reviewer, id, changed, AdminExtras::default()).unwrap();
    assert_eq!(h.resource(staff, id).unwrap().0.status, Status::Pending);
    h.review(staff, id, "approve", "").unwrap();
    let sha = hex::encode(Sha256::digest(b"straight to published?"));
    let plan = h.begin_upload(reviewer, "卷子.pdf", "application/pdf", vec![PartSpec { size: 22, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), b"straight to published?").unwrap();
    h.confirm_part(reviewer, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let extras = AdminExtras { status: Some("published".into()), ..Default::default() };
    assert_eq!(h.create_resource(reviewer, plan.upload_id, input(other), extras).unwrap().status, Status::Pending);

    // Their own note / removal requests go to someone else as well.
    let q = h.request_resource_change(reviewer, id, "note", "新备注").unwrap();
    assert!(h.review_resource_change(reviewer, q.id, true, "").is_err());
    h.review_resource_change(staff, q.id, true, "").unwrap();

    // Admins are exempt: their uploads and categories go live at once.
    let mine = upload(&h, &dir, staff, course, b"admin's own file").await;
    assert_eq!(h.resource(staff, mine).unwrap().0.status, Status::Published);
    let third = h.create_node(staff, NodeInput { parent: Some(section), kind: "course".into(), code: String::new(), name: "编译原理".into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id;
    assert_eq!(h.node(staff, third).unwrap().0.node.status, NodeStatus::Active);
    let _ = std::fs::remove_dir_all(dir);
}
