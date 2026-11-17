//! Router-level download history regressions; all storage plans are metadata-only.
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use xmuhub_core::Hub;
use xmuhub_core::db::Db;
use xmuhub_core::hub::Limits;
use xmuhub_core::model::*;
use xmuhub_core::storage::{Storage, local::LocalBackend, mirrors::Mirrors};

use crate::api::{App, CSRF_HEADER, SESSION_COOKIE, router};

fn temp_dir(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("xmuhub-api-downloads-{label}-{}-{nonce}-{sequence}", std::process::id()))
}

fn site() -> Arc<crate::web::Site> {
    static SITE: OnceLock<Arc<crate::web::Site>> = OnceLock::new();
    SITE.get_or_init(|| {
        let dir = temp_dir("site");
        let web = dir.join("web");
        let config_dir = dir.join("site");
        std::fs::create_dir_all(web.join("assets/site")).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        // Only the two required pictures: no production pages, libraries or network.
        for name in ["logo", "background"] {
            std::fs::write(web.join(format!("assets/site/{name}.svg")), "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        }
        let mut config = json!({});
        for key in ["name", "subtitle", "school", "school_short", "description", "slogan", "footer", "domain", "repo", "mcp_name", "verified_label", "link_example", "announcement"] {
            config[key] = json!("test");
        }
        std::fs::write(config_dir.join("site.json"), serde_json::to_vec(&config).unwrap()).unwrap();
        let config = crate::site::Site::load(&config_dir, &web).unwrap();
        let site = Arc::new(crate::web::Site::load(&web, config).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
        site
    }).clone()
}

struct Fixture {
    // Drop the router/DB handles before deleting the temporary directory (also on Windows).
    app: Option<Arc<App>>,
    dir: PathBuf,
    cookies: [String; 2],
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.app.take());
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

impl Fixture {
    fn new() -> Self {
        let site = site();
        let dir = temp_dir("db");
        let db = Arc::new(Db::open(&dir.join("test.redb"), 1 << 20).unwrap());
        let cookies = [format!("{}-first", dir.display()), format!("{}-second", dir.display())];
        db.write(|tx| {
            for (index, cookie) in cookies.iter().enumerate() {
                let id = index as Id + 1;
                tx.put_user(&User {
                    id, email: format!("reader{id}@example.invalid"), nickname: format!("reader{id}"),
                    password: String::new(), level: Level::Contributor, banned: false,
                    created_at: now(), created_ip: String::new(), last_login: 0, uploads: 0,
                })?;
                tx.put_session(&Session {
                    hash: Sha256::digest(cookie.as_bytes()).into(), user: id,
                    created_at: now(), expires_at: now() + 3600, ip: String::new(),
                })?;
            }
            tx.put_node(&Node {
                id: 1000, parent: None, kind: NodeKind::Course, code: String::new(),
                name: "course".into(), label: "course".into(), aliases: vec![], bucketed: false,
                sort: 0, status: NodeStatus::Active, created_by: 2, created_at: now(),
            })?;
            for (id, status, parts) in [(1001, Status::Published, 1), (1002, Status::Pending, 1), (1003, Status::Removed, 1), (1004, Status::Restricted, 1), (1005, Status::Published, 2), (1006, Status::Published, 1)] {
                let key = format!("blob-{id}");
                tx.put_blob(&Blob {
                    key: key.clone(), size: parts, created_at: now(),
                    parts: (0..parts).map(|i| Part {
                        size: 1, sha256: format!("hash-{id}-{i}"),
                        replicas: if id == 1006 { vec![] } else { vec![Location::Local { path: format!("part-{id}-{i}") }] },
                    }).collect(),
                })?;
                tx.put_resource(&Resource {
                    id, node: 1000, tag: Tag::Notes,
                    name: NameParts { course: "course".into(), time: String::new(), type_word: "笔记".into(), paper: String::new(), with_answer: false, extra: String::new(), version: 1 },
                    ext: "pdf".into(), note: String::new(), original_name: String::new(), source: String::new(), uncertain: false,
                    blob: key, size: parts, mime: "application/pdf".into(), status, needs_review: false,
                    review_note: String::new(), uploader: 2, reviewed_by: None, created_at: now(), updated_at: now(), downloads: 0,
                })?;
            }
            Ok(())
        }).unwrap();
        let storage = Storage::new(vec![Arc::new(LocalBackend { dir: dir.join("files"), secret: vec![] })]);
        let hub = Arc::new(Hub::open(db, storage, Limits::default(), vec![]).unwrap());
        let app = Arc::new(App {
            hub, site, mirrors: Arc::new(Mirrors::new(vec![])), local: None,
            worker_url: String::new(), relay: None, mailer: None, script_token: None,
            secure_cookie: false, github: None, missing_seen: Default::default(),
            gates: Default::default(), tree_json: Default::default(), scans: Default::default(),
            turnstile: None, secret: vec![],
        });
        Self { app: Some(app), dir, cookies }
    }

    async fn request(&self, method: Method, path: &str, user: Option<usize>, csrf: bool, body: Value) -> Response {
        let mut req = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
        if let Some(user) = user {
            req = req.header(header::COOKIE, format!("{SESSION_COOKIE}={}", self.cookies[user]));
        }
        if csrf {
            req = req.header(CSRF_HEADER, "1");
        }
        router(self.app.as_ref().unwrap().clone()).oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap()
    }

    async fn me(&self, user: Option<usize>) -> Value {
        let response = self.request(Method::GET, "/api/me", user, false, Value::Null).await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
    }

    async fn assert_empty(&self) {
        for user in [Some(0), Some(1), None] {
            assert_eq!(self.me(user).await["downloaded_resources"], json!([]));
        }
    }
}

#[tokio::test]
async fn download_tests_auth_csrf_account_and_visibility_guards() {
    let f = Fixture::new();
    let path = "/api/resources/1001/downloaded";
    for (user, csrf, account, status) in [
        (None, true, 1, StatusCode::UNAUTHORIZED),
        (Some(0), false, 1, StatusCode::FORBIDDEN),
        (Some(0), true, 2, StatusCode::FORBIDDEN),
        (Some(1), true, 1, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(f.request(Method::POST, path, user, csrf, json!({"account": account})).await.status(), status);
        f.assert_empty().await;
    }
    for id in [1002, 1003, 1004, 9999] {
        let response = f.request(Method::POST, &format!("/api/resources/{id}/downloaded"), Some(0), true, json!({"account": 1})).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "resource {id}");
        f.assert_empty().await;
    }
    // A GET cannot accidentally turn a speculative fetch into a history write.
    assert_eq!(f.request(Method::GET, path, Some(0), false, Value::Null).await.status(), StatusCode::METHOD_NOT_ALLOWED);
    f.assert_empty().await;
}

#[tokio::test]
async fn download_tests_record_is_idempotent_and_account_private() {
    let f = Fixture::new();
    for _ in 0..2 {
        let response = f.request(Method::POST, "/api/resources/1001/downloaded", Some(0), true, json!({"account": 1})).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(f.me(Some(0)).await["downloaded_resources"], json!([1001]));
        assert_eq!(f.me(Some(1)).await["downloaded_resources"], json!([]));
        assert_eq!(f.me(None).await["downloaded_resources"], json!([]));
    }
    let response = f.request(Method::POST, "/api/resources/1005/downloaded", Some(1), true, json!({"account": 2})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.me(Some(1)).await["downloaded_resources"], json!([1005]));
    assert_eq!(f.me(Some(0)).await["downloaded_resources"], json!([1001]));
}

#[tokio::test]
async fn download_tests_get_plans_and_redirects_do_not_write_account_history() {
    let f = Fixture::new();
    for suffix in ["", "?peek=1", "?peek=true"] {
        let response = f.request(Method::GET, &format!("/api/resources/1001/download{suffix}"), Some(0), false, Value::Null).await;
        assert_eq!(response.status(), StatusCode::OK);
        let plan: Value = serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(plan["account"], json!(1));
        assert_eq!(plan["parts"].as_array().unwrap().len(), 1);
        f.assert_empty().await;
    }
    let guest = f.request(Method::GET, "/d/1001", None, false, Value::Null).await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert!(guest.headers()[header::LOCATION].to_str().unwrap().starts_with("/login?next="));
    let multipart = f.request(Method::GET, "/d/1005", Some(0), false, Value::Null).await;
    assert_eq!(multipart.status(), StatusCode::SEE_OTHER);
    assert_eq!(multipart.headers()[header::LOCATION], "/r/1005");
    f.assert_empty().await;
    let unavailable = f.request(Method::GET, "/d/1006", Some(0), false, Value::Null).await;
    assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
    f.assert_empty().await;
    let response = f.request(Method::GET, "/d/1001", Some(0), false, Value::Null).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/api/local/file/part-1001-0");
    f.assert_empty().await;
}
