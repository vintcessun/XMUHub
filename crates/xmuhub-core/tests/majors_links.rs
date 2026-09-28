//! 适用专业 labels, 站外资源 links, and GitHub imports that take course / time from folders.

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, User};
use xmuhub_core::storage::github::{RepoFile, RepoScan};
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

fn input(node: Id, major: Option<&str>) -> ResourceInput {
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
        major: major.map(str::to_string),
    }
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
async fn majors_label_files() {
    let (h, dir) = open("majors");
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let section = node(&h, staff, None, "section", "专业课");
    let course = node(&h, staff, Some(section), "course", "计算机组成原理");

    let content = b"cs exam";
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(staff, "计组.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    h.confirm_part(staff, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let id = h.create_resource(staff, plan.upload_id, input(course, Some(" 软件工程/ ")), AdminExtras::default()).unwrap().id;
    assert_eq!(h.major_of(id), "软件工程", "trimmed, separators dropped");

    // None keeps it, a new value replaces it, "" clears it.
    h.update_resource(staff, id, input(course, None), AdminExtras::default()).unwrap();
    assert_eq!(h.major_of(id), "软件工程");
    h.update_resource(staff, id, input(course, Some("计算机科学与技术")), AdminExtras::default()).unwrap();
    assert_eq!(h.major_of(id), "计算机科学与技术");
    h.update_resource(staff, id, input(course, Some("")), AdminExtras::default()).unwrap();
    assert_eq!(h.major_of(id), "");

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn links_are_kept_by_staff() {
    let (h, dir) = open("links");
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "b@example.invalid", "student");
    let (staff, me) = (Viewer { user: Some(&admin) }, Viewer { user: Some(&student) });

    assert!(h.add_link(me, "合集", "https://example.com/x", "", 0).is_err(), "contributors can't add");
    for bad in ["javascript:alert(1)", "ftp://example.com", "https://", "https://exa mple.com", "https://example.com/\"x"] {
        assert!(h.add_link(staff, "合集", bad, "", 0).is_err(), "{bad}");
    }
    let b = h.add_link(staff, "网盘合集", "https://pan.example.com/s/1", "提取码 abcd", 20).unwrap();
    let a = h.add_link(staff, "XMU-CS-exam", "https://github.com/example/XMU-CS-exam", "", 10).unwrap();
    assert_eq!(h.links().iter().map(|l| l.id).collect::<Vec<_>>(), vec![a.id, b.id], "sorted by the sort number");
    h.update_link(staff, b.id, "网盘合集", "https://pan.example.com/s/2", "", 5).unwrap();
    assert_eq!(h.links()[0].url, "https://pan.example.com/s/2");
    assert!(h.delete_link(me, a.id).is_err());
    h.delete_link(staff, a.id).unwrap();
    assert_eq!(h.links().len(), 1);

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn imports_take_time_and_major_from_the_repo() {
    let (h, dir) = open("import-path");
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let section = node(&h, staff, None, "section", "专业课");
    let course = node(&h, staff, Some(section), "course", "数据结构");

    // 学期/课程/文件: the file name says nothing about when.
    let scan = RepoScan {
        owner: "someone".into(),
        repo: "FishTouchingXMUer".into(),
        branch: "main".into(),
        commit: "0123456789abcdef0123".into(),
        license: String::new(),
        files: vec![RepoFile { path: "2023-2024秋/数据结构/期末.pdf".into(), size: 10, sha: "aaaa".into() }],
    };
    let report = h.import_repo(staff, &scan, 2, "软件工程", &|g: &str| (g == "2023-2024秋/数据结构").then_some(course)).unwrap();
    assert_eq!(report.created, 1);
    let (_, _, _, files) = h.node(staff, course).unwrap();
    assert_eq!(files[0].name.time, "2023-2024秋", "time taken from the folder");
    assert_eq!(h.major_of(files[0].id), "软件工程");

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
