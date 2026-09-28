//! Study levels (本科 / 研究生) on courses and groups, and uploaders showing their name.

use std::sync::Arc;

use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{CodePurpose, LEVEL_BOTH, LEVEL_GRADUATE, LEVEL_UNDERGRAD, Limits, NodeInput, NodePatch, Registration, Viewer};
use xmuhub_core::model::{Id, User};
use xmuhub_core::search::{DocType, Filter};
use xmuhub_core::storage::Storage;
use xmuhub_core::storage::local::LocalBackend;

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: "password123".into(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str, level: u8) -> Id {
    let input = NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level };
    h.create_node(v, input).unwrap().id
}

fn set_level(h: &Hub, v: Viewer, id: Id, level: u8) {
    let p = NodePatch { code: None, name: None, label: None, aliases: None, bucketed: None, sort: None, parent: None, level: Some(level), approve: false };
    h.update_node(v, id, p).unwrap();
}

/// Course names matching `q` under a level filter.
fn courses(h: &Hub, v: Viewer, q: &str, level: Option<u8>) -> Vec<String> {
    let f = Filter { ty: Some(DocType::Node), level, ..Default::default() };
    let mut names: Vec<String> = h.search(v, q, f, 50, 0).unwrap().0.into_iter().filter_map(|i| match i {
        xmuhub_core::hub::SearchItem::Node { node, .. } => Some(node.name),
        _ => None,
    }).collect();
    names.sort();
    names
}

#[test]
fn levels_inherit_and_filter_search() {
    let dir = std::env::temp_dir().join(format!("xmuhub-levels-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    let h = Hub::open(db.clone(), storage.clone(), Limits::default(), vec!["a@example.invalid".into()]).unwrap();
    let admin = register(&h, "a@example.invalid", "admin");
    let student = register(&h, "b@example.invalid", "student");
    let staff = Viewer { user: Some(&admin) };

    let section = node(&h, staff, None, "section", "专业课", 0);
    let college = node(&h, staff, Some(section), "group", "信息学院", 0);
    let ug = node(&h, staff, Some(college), "course", "数据结构", 0);
    let grad = node(&h, staff, Some(college), "course", "数据挖掘", LEVEL_GRADUATE);
    let both = node(&h, staff, Some(college), "course", "数据库", LEVEL_BOTH);
    assert_eq!(h.node_level(ug), (0, 0));
    assert_eq!(h.node_level(both), (LEVEL_BOTH, LEVEL_BOTH));
    assert_eq!(h.node_level(grad), (LEVEL_GRADUATE, LEVEL_GRADUATE));

    // Untagged courses count as 本科; "both" shows under either filter.
    assert_eq!(courses(&h, staff, "数据", Some(LEVEL_UNDERGRAD)), ["数据库", "数据结构"]);
    assert_eq!(courses(&h, staff, "数据", Some(LEVEL_GRADUATE)), ["数据库", "数据挖掘"]);

    // A whole group tagged 研究生: its courses inherit it unless they have their own level.
    let school = node(&h, staff, Some(section), "group", "研究生院", 0);
    let inherit = node(&h, staff, Some(school), "course", "数据科学前沿", 0);
    set_level(&h, staff, school, LEVEL_GRADUATE);
    assert_eq!(h.node_level(inherit), (0, LEVEL_GRADUATE));
    assert_eq!(courses(&h, staff, "数据", Some(LEVEL_GRADUATE)), ["数据库", "数据挖掘", "数据科学前沿"]);
    set_level(&h, staff, inherit, LEVEL_UNDERGRAD);
    assert_eq!(h.node_level(inherit), (LEVEL_UNDERGRAD, LEVEL_UNDERGRAD));
    // Clearing it inherits again.
    set_level(&h, staff, inherit, 0);
    assert_eq!(h.node_level(inherit).1, LEVEL_GRADUATE);

    // Uploaders choose whether their nickname is shown; it survives a restart.
    let me = Viewer { user: Some(&student) };
    assert!(!h.public_uploader(student.id));
    h.set_public_uploader(me, true).unwrap();
    drop(h);
    let h = Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap();
    assert!(h.public_uploader(student.id));
    assert_eq!(h.node_level(inherit).1, LEVEL_GRADUATE);
    h.set_public_uploader(me, false).unwrap();
    assert!(!h.public_uploader(student.id));

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
