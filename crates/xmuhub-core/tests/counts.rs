//! Public-file counts are kept up to date write by write (not recounted from scratch):
//! after any mix of uploads, reviews, moves and tree edits they must equal a full recount.

mod common;

use common::test_password;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::{AdminExtras, CodePurpose, Limits, NodeInput, NodePatch, PartSpec, Registration, ResourceInput, Viewer};
use xmuhub_core::model::{Id, User};
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

async fn upload(h: &Hub, dir: &std::path::Path, v: Viewer<'_>, node: Id, content: &[u8]) -> Id {
    let sha = hex::encode(Sha256::digest(content));
    let plan = h.begin_upload(v, "讲义.pdf", "application/pdf", vec![PartSpec { size: content.len() as u64, sha256: sha.clone() }]).await.unwrap();
    std::fs::write(dir.join("files").join(&sha), content).unwrap();
    h.confirm_part(v, plan.upload_id, 0, Receipt { asset_id: None }).await.unwrap();
    let input = ResourceInput { node, course: String::new(), time: "2023".into(), type_word: "课件".into(), tag: String::new(), paper: String::new(), with_answer: false, extra: String::from_utf8_lossy(content).into(), note: String::new(), subtitle: None, major: None };
    h.create_resource(v, plan.upload_id, input, AdminExtras::default()).unwrap().id
}

#[tokio::test]
async fn incremental_counts_match_a_full_recount() {
    let dir = std::env::temp_dir().join(format!("xmuhub-counts-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let h = open(&dir);
    let admin = register(&h, "a@example.invalid", "admin");
    let v = Viewer { user: Some(&admin) };
    let section = node(&h, v, None, "section", "专业课");
    let colleges: Vec<Id> = (0..3).map(|i| node(&h, v, Some(section), "group", &format!("学院{i}"))).collect();
    let mut courses: Vec<Id> = Vec::new();
    for (i, g) in colleges.iter().enumerate() {
        for k in 0..3 {
            courses.push(node(&h, v, Some(*g), "course", &format!("课程{i}{k}")));
        }
    }
    // A small deterministic generator (no rand dependency).
    let mut seed: u64 = 42;
    let mut next = |m: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % m
    };
    let mut files = Vec::new();
    for i in 0..40u32 {
        files.push(upload(&h, &dir, v, courses[next(courses.len())], format!("file {i}").as_bytes()).await);
    }
    for round in 0..120 {
        let f = files[next(files.len())];
        match next(6) {
            0 => { let _ = h.review(v, f, "remove", "x"); }
            1 => { let _ = h.review(v, f, "restore", ""); }
            2 => { let _ = h.review(v, f, "reject", "x"); }
            3 => { let _ = h.move_resources(v, &[f], courses[next(courses.len())]); }
            4 => {
                // A course moves to another college, taking its files along.
                let c = courses[next(courses.len())];
                let _ = h.update_node(v, c, NodePatch { parent: Some(colleges[next(colleges.len())]), ..Default::default() });
            }
            _ if round % 30 == 5 && courses.len() > 4 => {
                let from = courses.remove(next(courses.len()));
                let into = courses[next(courses.len())];
                h.merge_node(v, from, into).unwrap();
            }
            _ => files.push(upload(&h, &dir, v, courses[next(courses.len())], format!("more {round}").as_bytes()).await),
        }
    }
    let live: std::collections::BTreeMap<Id, usize> = h.tree().iter().map(|i| (i.node.id, i.count)).collect();
    drop(h);
    let fresh = open(&dir);
    let recounted: std::collections::BTreeMap<Id, usize> = fresh.tree().iter().map(|i| (i.node.id, i.count)).collect();
    assert_eq!(live, recounted, "incremental counts drifted from a full recount");
    assert!(recounted.values().sum::<usize>() > 0);
    let _ = std::fs::remove_dir_all(dir);
}
