//! 站外资源 links suggested by students, added to the list only after another reviewer agrees.

mod common;

use common::test_password;

use std::sync::Arc;

use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{CodePurpose, Limits, Registration, Viewer};
use xmuhub_core::model::User;
use xmuhub_core::storage::Storage;
use xmuhub_core::storage::local::LocalBackend;

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

#[tokio::test]
async fn suggested_links_wait_for_another_reviewer() {
    let dir = std::env::temp_dir().join(format!("xmuhub-linksugg-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let open = || {
        let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
        let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
        Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap()
    };
    let h = open();
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let student = register(&h, "s@example.invalid", "student");
    let me = Viewer { user: Some(&student) };
    let guest = Viewer { user: None };

    assert!(h.suggest_link(guest, "资料站", "https://example.org/", "").is_err(), "signed-in only");
    assert!(h.suggest_link(me, "资料站", "ftp://example.org/", "").is_err(), "http(s) only");
    let s = h.suggest_link(me, "资料站", "https://example.org/", "电子书").unwrap();
    assert_eq!(s.status, "pending");
    assert!(h.suggest_link(me, "又一个", "https://example.org", "").is_err(), "already waiting");
    assert!(h.links().is_empty(), "not listed before review");
    assert!(h.review_link_suggestion(me, s.id, true, "", None).is_err(), "students can't review");
    assert_eq!(h.link_suggestions(staff).unwrap().len(), 1);
    assert_eq!(h.link_suggestions(me).unwrap()[0].id, s.id);

    let l = h.review_link_suggestion(staff, s.id, true, "", Some(("资料站（电子书）", "https://example.org/", "可搜电子书", 5))).unwrap().unwrap();
    assert_eq!((l.title.as_str(), l.sort, l.created_by), ("资料站（电子书）", 5, student.id));
    assert_eq!(h.links().len(), 1);
    assert!(h.review_link_suggestion(staff, s.id, false, "重复", None).is_err(), "already handled");
    assert!(h.suggest_link(me, "资料站", "https://example.org/", "").is_err(), "already on the list");

    // Turned down only with a reason, which its author sees; a reviewer's own needs someone else.
    let bad = h.suggest_link(me, "广告", "https://ads.example.com/", "").unwrap();
    assert!(h.review_link_suggestion(staff, bad.id, false, "", None).is_err());
    h.review_link_suggestion(staff, bad.id, false, "和学习资料无关", None).unwrap();
    let mine = h.link_suggestions(me).unwrap();
    assert_eq!(mine.iter().find(|x| x.id == bad.id).unwrap().review_note, "和学习资料无关");
    let own = h.suggest_link(staff, "审核员推荐", "https://staff.example.net/", "").unwrap();
    assert!(h.review_link_suggestion(staff, own.id, true, "", None).is_err());

    drop(h);
    let h = open();
    assert_eq!(h.links().len(), 1);
    assert_eq!(h.link_suggestions(staff).unwrap().len(), 1, "survives a restart");
    let _ = std::fs::remove_dir_all(dir);
}
