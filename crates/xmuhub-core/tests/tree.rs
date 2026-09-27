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
                password: "password123".into(),
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
