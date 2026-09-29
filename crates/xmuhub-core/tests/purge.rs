//! 封禁并清理: one admin action bans an account and takes back everything it put on the site.

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, PartSpec, Registration, ResourceInput, SeriesText, Viewer, WantFilter};
use xmuhub_core::model::{Id, Level, Status, User};
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::{Receipt, Storage};

fn open(dir: &std::path::Path) -> Hub {
    let db = Arc::new(Db::open(&dir.join("t.redb"), 1 << 20).unwrap());
    let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: b"k".to_vec() })]);
    Hub::open(db, storage, Limits::default(), vec!["a@example.invalid".into()]).unwrap()
}

fn register(h: &Hub, email: &str, nick: &str) -> User {
    let (email, code) = h.request_code(email, CodePurpose::Register, "1.1.1.1").unwrap();
    h.register(Registration { email, code, password: test_password(), nickname: nick.into() }, "1.1.1.1").unwrap().1
}

fn node(h: &Hub, v: Viewer, parent: Option<Id>, kind: &str, name: &str) -> Id {
    h.create_node(v, NodeInput { parent, kind: kind.into(), code: String::new(), name: name.into(), label: String::new(), aliases: Vec::new(), bucketed: false, sort: 0, level: 0 }).unwrap().id
}

fn t(title: &str) -> SeriesText<'_> {
    SeriesText { title, source: "张老师", year: "2023" }
}

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "讲义.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let input = ResourceInput { node, course: String::new(), time: "2023".into(), type_word: "课件".into(), tag: String::new(), paper: String::new(), with_answer: false, extra: String::from_utf8_lossy(content).into(), note: String::new(), subtitle: None, major: None };
    h.create_resource(v, plan.upload_id, input, AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn purge_takes_back_everything() {
    let dir = std::env::temp_dir().join(format!("xmuhub-purge-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let staff = Viewer { user: Some(&admin) };
    let rev = register(&h, "r@example.invalid", "reviewer");
    h.update_user(staff, rev.id, Some(Level::Reviewer), None).unwrap();
    let rev = h.user(rev.id).unwrap();
    let reviewer = Viewer { user: Some(&rev) };
    let bad_user = register(&h, "x@example.invalid", "anti");
    let bad = Viewer { user: Some(&bad_user) };
    let good_user = register(&h, "g@example.invalid", "good");
    let good = Viewer { user: Some(&good_user) };
    let guest = Viewer { user: None };

    let section = node(&h, staff, None, "section", "专业课");
    let course = node(&h, staff, Some(section), "course", "数据结构");
    let theirs = [upload(&h, &dir, bad, course, b"junk 1").await, upload(&h, &dir, bad, course, b"junk 2").await, upload(&h, &dir, bad, course, b"junk 3").await];
    h.review(staff, theirs[0], "approve", "").unwrap();
    let kept = upload(&h, &dir, good, course, b"good one").await;
    h.review(staff, kept, "approve", "").unwrap();
    h.add_comment(bad, kept, "垃圾评论垃圾评论").unwrap();
    h.rate(bad, kept, 1).unwrap();
    h.rate(good, kept, 5).unwrap();
    h.add_want(bad, "广告广告广告", "", None).unwrap();
    h.propose_series(bad, None, course, t("乱建的合集"), &theirs[1..]).unwrap();
    let c = h.create_collection(bad, "我的", "").unwrap();
    h.collect(bad, c.id, kept, true).unwrap();
    h.share_collection(bad, c.id, true).unwrap();
    h.suggest_link(bad, "广告站", "https://ads.example.com/", "").unwrap();
    let (secret, _) = h.create_token(bad, "脚本").unwrap();

    // Only admins, and never on a peer.
    assert!(h.purge_user(reviewer, bad_user.id, "").is_err());
    assert!(h.purge_user(staff, admin.id, "").is_err());
    let rep = h.purge_user(staff, bad_user.id, "恶意账号").unwrap();
    assert_eq!((rep.removed, rep.rejected, rep.comments, rep.ratings, rep.wants, rep.series, rep.collections, rep.links, rep.tokens), (1, 2, 1, 1, 1, 1, 1, 1, 1));
    assert!(h.user(bad_user.id).unwrap().banned);
    assert_eq!(h.resource(staff, theirs[0]).unwrap().0.status, Status::Removed);
    assert_eq!(h.resource(staff, theirs[1]).unwrap().0.status, Status::Rejected);
    assert_eq!(h.resource(staff, theirs[0]).unwrap().0.review_note, "恶意账号");
    assert_eq!(h.resource(guest, kept).unwrap().0.status, Status::Published, "other people's files stay");
    assert_eq!(h.rating(kept).avg, 5.0, "their rating no longer counts");
    assert!(h.pending_series(staff).unwrap().is_empty());
    assert!(h.public_collections(guest, 10).is_empty() && h.pending_collections(staff).unwrap().is_empty());
    assert!(h.wants(staff, WantFilter::Pending, None).unwrap().is_empty());
    assert!(h.token_user(&secret).is_none(), "tokens revoked");
    // Unbanning doesn't bring the content back.
    h.update_user(staff, bad_user.id, None, Some(false)).unwrap();
    assert_eq!(h.resource(staff, theirs[0]).unwrap().0.status, Status::Removed);
    let _ = std::fs::remove_dir_all(dir);
}
