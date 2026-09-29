mod common;

use common::test_password;

use std::sync::Arc;

use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{CodePurpose, Limits, NodeInput, Registration, Viewer};
use xmuhub_core::model::NodeStatus;
use xmuhub_core::storage::Storage;
use xmuhub_core::storage::local::LocalBackend;

fn input(parent: Option<u64>, kind: &str, name: &str) -> NodeInput {
    NodeInput {
        parent,
        kind: kind.into(),
        code: String::new(),
        name: name.into(),
        label: String::new(),
        aliases: Vec::new(),
        bucketed: false,
        sort: 0,
        level: 0,
    }
}

#[test]
fn contributor_can_create_under_misclassified_college_only() {
    let dir = std::env::temp_dir().join(format!("xmuhub-tree-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend {
        dir: dir.join("files"),
        secret: b"k".to_vec(),
    })]);
    let h = Hub::open(
        db,
        storage,
        Limits::default(),
        vec!["a@example.invalid".into()],
    )
    .unwrap();

    let register = |email: &str, nick: &str| {
        let (email, code) = h
            .request_code(email, CodePurpose::Register, "1.1.1.1")
            .unwrap();
        h.register(
            Registration {
                email,
                code,
                password: test_password(),
                nickname: nick.into(),
            },
            "1.1.1.1",
        )
        .unwrap()
        .1
    };
    let admin = register("a@example.invalid", "admin");
    let contributor = register("b@example.invalid", "student");
    let staff = Viewer { user: Some(&admin) };
    let uploader = Viewer {
        user: Some(&contributor),
    };
    let section = h
        .create_node(staff, input(None, "section", "专业课"))
        .unwrap();
    let college = h
        .create_node(staff, input(Some(section.id), "course", "智能制造学院"))
        .unwrap();
    let group = h
        .create_node(staff, input(Some(section.id), "group", "信息学院"))
        .unwrap();

    let course = h
        .create_node(uploader, input(Some(college.id), "course", "机械设计"))
        .unwrap();
    assert_eq!(course.status, NodeStatus::Pending);
    assert_eq!(course.parent, Some(college.id));
    assert!(
        h.create_node(uploader, input(Some(group.id), "course", "数据结构"))
            .is_ok()
    );
    assert!(
        h.create_node(
            uploader,
            input(Some(course.id), "course", "不允许的嵌套课程")
        )
        .is_err()
    );
    assert!(
        h.create_node(
            uploader,
            input(Some(section.id), "course", "不允许的直属课程")
        )
        .is_err()
    );

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A refused write leaves memory as it was: nothing half-made shows up, before or after a
/// restart (a refusal no longer reloads the whole state from the database).
#[test]
fn refused_writes_leave_nothing_behind() {
    let dir = std::env::temp_dir().join(format!("xmuhub-tree-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let open = || {
        let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
        let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
        Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap()
    };
    let h = open();
    let (email, code) = h.request_code("a@example.invalid", CodePurpose::Register, "1.1.1.1").unwrap();
    let admin = h.register(Registration { email, code, password: test_password(), nickname: "admin".into() }, "1.1.1.1").unwrap().1;
    let v = Viewer { user: Some(&admin) };
    let section = h.create_node(v, input(None, "section", "专业课")).unwrap().id;
    let mut bad = input(Some(section), "course", "层次不对的课");
    bad.level = 9;
    assert!(h.create_node(v, bad).is_err());
    let named = |h: &Hub| h.tree().iter().filter(|i| i.node.name == "层次不对的课").count();
    assert_eq!(named(&h), 0, "not in memory");
    h.create_node(v, input(Some(section), "course", "正常的课")).unwrap();
    drop(h);
    let h = open();
    assert_eq!(named(&h), 0, "not in the database");
    assert_eq!(h.tree().iter().filter(|i| i.node.name == "正常的课").count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// The tree's generation moves with every change a cached copy of the tree would miss.
#[test]
fn tree_generation_follows_changes() {
    let dir = std::env::temp_dir().join(format!("xmuhub-tree-gen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap();
    let (email, code) = h.request_code("a@example.invalid", CodePurpose::Register, "1.1.1.1").unwrap();
    let admin = h.register(Registration { email, code, password: test_password(), nickname: "admin".into() }, "1.1.1.1").unwrap().1;
    let v = Viewer { user: Some(&admin) };
    let g0 = h.tree_generation();
    let section = h.create_node(v, input(None, "section", "专业课")).unwrap().id;
    let g1 = h.tree_generation();
    assert!(g1 > g0, "a new node");
    let course = h.create_node(v, input(Some(section), "course", "数据结构")).unwrap().id;
    let g2 = h.tree_generation();
    assert!(g2 > g1);
    h.update_node(v, course, xmuhub_core::hub::NodePatch { level: Some(2), ..Default::default() }).unwrap();
    assert!(h.tree_generation() > g2, "a study level");
    let mut bad = input(Some(section), "course", "x");
    bad.level = 9;
    let g3 = h.tree_generation();
    assert!(h.create_node(v, bad).is_err());
    assert_eq!(h.tree_generation(), g3, "a refused write changes nothing");
    let _ = std::fs::remove_dir_all(dir);
}
