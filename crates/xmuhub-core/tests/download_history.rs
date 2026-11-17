//! Download markers belong to accounts, not browsers or administrators.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::{ReadableDatabase, ReadableTableMetadata};
use xmuhub_core::Hub;
use xmuhub_core::db::{DOWNLOAD_HISTORY, Db, Dump};
use xmuhub_core::hub::{CodePurpose, Limits, Registration, Viewer};
use xmuhub_core::model::*;
use xmuhub_core::storage::{Storage, local::LocalBackend};

fn open(db: Arc<Db>, dir: &Path) -> Hub {
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    Hub::open(db, storage, Limits::default(), vec!["admin@example.invalid".into()]).unwrap()
}

fn register(h: &Hub, email: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: common::test_password(), nickname: "reader".into() }, "1.1.1.1").unwrap().1
}

fn resource(id: Id, uploader: Id, status: Status) -> Resource {
    Resource {
        id, node: 1000, tag: Tag::Notes,
        name: NameParts { course: "course".into(), time: String::new(), type_word: "笔记".into(), paper: String::new(), with_answer: false, extra: String::new(), version: 1 },
        ext: "pdf".into(), note: String::new(), original_name: String::new(), source: String::new(), uncertain: false,
        blob: "metadata-only".into(), size: 1, mime: "application/pdf".into(), status, needs_review: false,
        review_note: String::new(), uploader, reviewed_by: None, created_at: now(), updated_at: now(), downloads: 0,
    }
}

struct Fixture {
    dir: PathBuf,
    db: Arc<Db>,
    hub: Hub,
    admin: User,
    first: User,
    second: User,
}

fn fixture(label: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("xmuhub-downloads-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let hub = open(db.clone(), &dir);
    let admin = register(&hub, "admin@example.invalid");
    let first = register(&hub, "first@example.invalid");
    let second = register(&hub, "second@example.invalid");
    drop(hub);
    db.write(|tx| {
        tx.put_node(&Node { id: 1000, parent: None, kind: NodeKind::Course, code: String::new(), name: "course".into(), label: "course".into(), aliases: Vec::new(), bucketed: false, sort: 0, status: NodeStatus::Active, created_by: admin.id, created_at: now() })?;
        tx.put_blob(&Blob { key: "metadata-only".into(), size: 1, parts: vec![], created_at: now() })?;
        tx.put_resource(&resource(1001, admin.id, Status::Published))?;
        tx.put_resource(&resource(1002, admin.id, Status::Published))?;
        tx.put_resource(&resource(1003, admin.id, Status::Pending))?;
        tx.put_resource(&resource(1004, first.id, Status::Removed))?;
        tx.put_resource(&resource(1005, first.id, Status::Restricted))?;
        tx.put_resource(&resource(1006, first.id, Status::Rejected))?;
        let mut missing = resource(1007, first.id, Status::Published);
        missing.blob = "missing-blob".into();
        tx.put_resource(&missing)?;
        Ok(())
    }).unwrap();
    let hub = open(db.clone(), &dir);
    Fixture { dir, db, hub, admin, first, second }
}

fn viewer(user: &User) -> Viewer<'_> { Viewer { user: Some(user) } }

#[test]
fn isolation_idempotence_and_reopening() {
    let f = fixture("isolation");
    f.hub.record_download(viewer(&f.first), 1001).unwrap();
    f.hub.record_download(viewer(&f.first), 1001).unwrap();
    f.hub.record_download(viewer(&f.second), 1002).unwrap();
    assert_eq!(f.hub.downloaded_resources(viewer(&f.first)).unwrap(), vec![1001]);
    assert_eq!(f.hub.downloaded_resources(viewer(&f.second)).unwrap(), vec![1002]);
    assert!(f.hub.downloaded_resources(viewer(&f.admin)).unwrap().is_empty());
    let txn = f.db.inner.begin_read().unwrap();
    assert_eq!(txn.open_table(DOWNLOAD_HISTORY).unwrap().len().unwrap(), 2);
    drop(txn);
    let Fixture { dir, db, hub, first, second, .. } = f;
    drop(hub);
    drop(db);
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let hub = open(db.clone(), &dir);
    assert_eq!(hub.downloaded_resources(viewer(&first)).unwrap(), vec![1001]);
    assert_eq!(hub.downloaded_resources(viewer(&second)).unwrap(), vec![1002]);
    drop(hub);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn authentication_visibility_and_account_deletion() {
    let f = fixture("authorization");
    let guest = Viewer { user: None };
    assert!(f.hub.record_download(guest, 1001).is_err());
    assert!(f.hub.downloaded_resources(guest).is_err());
    for id in [1003, 1004, 1005, 1006, 1007, 9999] {
        assert!(f.hub.record_download(viewer(&f.first), id).is_err());
    }
    // Metadata can remain visible even when the file cannot be downloaded.
    assert!(f.hub.resource(viewer(&f.first), 1004).is_ok());
    assert!(f.hub.resource(viewer(&f.first), 1006).is_ok());
    assert!(f.hub.record_download(viewer(&f.admin), 1006).is_err());
    f.hub.record_download(viewer(&f.first), 1001).unwrap();
    f.hub.record_download(viewer(&f.second), 1002).unwrap();
    f.hub.update_user(viewer(&f.admin), f.first.id, None, Some(true)).unwrap();
    // Even a stale pre-ban Viewer cannot write or read history.
    assert!(f.hub.record_download(viewer(&f.first), 1002).is_err());
    assert!(f.hub.downloaded_resources(viewer(&f.first)).is_err());
    f.hub.update_user(viewer(&f.admin), f.first.id, None, Some(false)).unwrap();
    f.hub.delete_account(&f.first, &common::test_password()).unwrap();
    assert!(f.db.downloaded_resources(f.first.id).unwrap().is_empty());
    assert_eq!(f.hub.downloaded_resources(viewer(&f.second)).unwrap(), vec![1002]);
    assert!(f.hub.record_download(viewer(&f.first), 1001).is_err());
    assert!(f.hub.downloaded_resources(viewer(&f.first)).is_err());
    let Fixture { dir, db, hub, .. } = f;
    drop(hub);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exported_history_roundtrip_and_legacy_json() {
    let f = fixture("backup");
    f.hub.record_download(viewer(&f.first), 1001).unwrap();
    f.hub.record_download(viewer(&f.second), 1002).unwrap();
    let bytes = f.db.export().unwrap();
    let dump: Dump = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dump.download_history.len(), 2);
    assert!(dump.download_history.iter().all(|(_, _, timestamp)| *timestamp > 0));
    let restored = Db::open(&f.dir.join("restored.redb"), 1 << 20).unwrap();
    // Exercise only the history's import primitives, not a general database restore API.
    restored.write(|tx| {
        for (user, resource, timestamp) in &dump.download_history {
            tx.put_download(*user, *resource, *timestamp)?;
        }
        Ok(())
    }).unwrap();
    assert_eq!(restored.downloaded_resources(f.first.id).unwrap(), vec![1001]);
    assert_eq!(restored.downloaded_resources(f.second.id).unwrap(), vec![1002]);
    assert!(restored.downloaded_resources(f.admin.id).unwrap().is_empty());
    let roundtrip: Dump = serde_json::from_slice(&restored.export().unwrap()).unwrap();
    assert_eq!(roundtrip.download_history, dump.download_history);
    let mut legacy: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    legacy.as_object_mut().unwrap().remove("download_history");
    let legacy: Dump = serde_json::from_value(legacy).unwrap();
    assert!(legacy.download_history.is_empty());
    assert_eq!(legacy.users.len(), dump.users.len());
    assert_eq!(legacy.resources.len(), dump.resources.len());
    drop(restored);
    let Fixture { dir, db, hub, .. } = f;
    drop(hub);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn user_prefix_range_includes_extreme_resource_ids() {
    let dir = std::env::temp_dir().join(format!("xmuhub-downloads-range-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Db::open(&dir.join("t.redb"), 1 << 20).unwrap();
    db.write(|tx| {
        for user in [0, 1, Id::MAX] {
            tx.put_download(user, 0, 1)?;
            tx.put_download(user, Id::MAX, 2)?;
        }
        Ok(())
    }).unwrap();
    for user in [0, 1, Id::MAX] {
        assert_eq!(db.downloaded_resources(user).unwrap(), vec![0, Id::MAX]);
    }
    db.write(|tx| tx.del_download_history(1)).unwrap();
    assert!(db.downloaded_resources(1).unwrap().is_empty());
    assert_eq!(db.downloaded_resources(0).unwrap(), vec![0, Id::MAX]);
    assert_eq!(db.downloaded_resources(Id::MAX).unwrap(), vec![0, Id::MAX]);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
