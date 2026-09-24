//! Accounts: single-character nicknames, peers can't ban each other, personal tokens.

use std::sync::Arc;

use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{CodePurpose, Limits, Registration, Viewer};
use xmuhub_core::model::{Level, User};
use xmuhub_core::storage::Storage;
use xmuhub_core::storage::local::LocalBackend;

fn hub(dir: &std::path::Path) -> Hub {
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    Hub::open(db, storage, Limits::default(), vec!["a@x.com".into(), "c@x.com".into()]).unwrap()
}

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: "password123".into(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

#[test]
fn accounts_and_tokens() {
    let dir = std::env::temp_dir().join(format!("xmuhub-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let h = hub(&dir);

    let a = register(&h, "a@x.com", "澈");
    assert_eq!(a.nickname, "澈");
    let b = register(&h, "b@x.com", "bob");
    let c = register(&h, "c@x.com", "carol");
    assert_eq!(a.level, Level::Admin);
    assert_eq!(c.level, Level::Admin);

    // Admins are peers: no banning each other; banning a contributor works.
    assert!(h.update_user(Viewer { user: Some(&a) }, c.id, None, Some(true)).is_err());
    assert!(h.update_user(Viewer { user: Some(&a) }, b.id, None, Some(true)).unwrap().banned);
    h.update_user(Viewer { user: Some(&a) }, b.id, None, Some(false)).unwrap();
    let b = h.user(b.id).unwrap();

    // Personal tokens authenticate as their owner until revoked.
    let (secret, t) = h.create_token(Viewer { user: Some(&b) }, "agent").unwrap();
    assert!(secret.starts_with("xmh_"));
    assert_eq!(h.token_user(&secret).map(|u| u.id), Some(b.id));
    assert!(h.token_user("xmh_wrong").is_none());
    assert_eq!(h.tokens(Viewer { user: Some(&b) }).unwrap().len(), 1);
    // Someone else can't revoke it.
    assert!(h.revoke_token(Viewer { user: Some(&c) }, t.id).is_err());
    h.revoke_token(Viewer { user: Some(&b) }, t.id).unwrap();
    assert!(h.token_user(&secret).is_none());

    // A banned owner's token stops working.
    let (secret, _) = h.create_token(Viewer { user: Some(&b) }, "").unwrap();
    h.update_user(Viewer { user: Some(&a) }, b.id, None, Some(true)).unwrap();
    assert!(h.token_user(&secret).is_none());

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
