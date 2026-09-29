//! JSON API under `/api`, the `/d/{id}` download redirect and HTML page routes.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use xmuhub_core::hub::{AdminExtras, CodePurpose, NodeInput, NodePatch, PartSpec, Registration, ResourceInput, SearchItem, Viewer, WantFilter};
use xmuhub_core::model::{Id, Level, Node, NodeStatus, Report, Resource, Status, TYPE_WORDS, Tag, User, WantStatus, now};
use xmuhub_core::search::{DocType, Filter};
use xmuhub_core::storage::Receipt;
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::mirrors::Mirrors;
use xmuhub_core::{Error, Hub};

use crate::mailer::Mailer;
use crate::web::Site;

/// Session cookie. Over HTTPS it carries the `__Host-` prefix: the browser then only takes
/// it from this exact host (Secure, Path=/, no Domain), so a sibling subdomain can't plant
/// one to sign a visitor into someone else's account. Plain `xh_sid` is what local HTTP
/// development uses, and is still read so sessions made before the switch keep working.
pub const SESSION_COOKIE: &str = "__Host-xh_sid";
const LEGACY_SESSION_COOKIE: &str = "xh_sid";
/// Non-GET API calls must carry this header. Browsers never attach custom headers to
/// cross-site form posts, so together with SameSite=Lax cookies it stops CSRF.
pub const CSRF_HEADER: &str = "x-xmuhub";

pub struct App {
    pub hub: Arc<Hub>,
    pub site: Arc<Site>,
    pub mirrors: Arc<Mirrors>,
    pub local: Option<Arc<LocalBackend>>,
    pub worker_url: String,
    pub relay: Option<crate::relay::Relay>,
    pub mailer: Option<Mailer>,
    pub script_token: Option<String>,
    pub secure_cookie: bool,
    pub github: Option<Arc<xmuhub_core::storage::github::GitHubBackend>>,
    /// Repository scans awaiting the admin's mapping, by scan id.
    pub scans: parking_lot::Mutex<std::collections::HashMap<String, xmuhub_core::storage::github::RepoScan>>,
    /// Human check for sign-ups from uncommon mail domains (None = not configured).
    pub turnstile: Option<crate::captcha::Turnstile>,
    /// Signs receipts for things submitted without an account (feedback).
    pub secret: Vec<u8>,
}

type S = State<Arc<App>>;

// ------------------------------------------------------------------ errors

pub struct ApiError(Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::TooMany(_) => StatusCode::TOO_MANY_REQUESTS,
            Error::Upstream(_) => StatusCode::BAD_GATEWAY,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status.is_server_error() {
            tracing::error!("{}", self.0);
        }
        (status, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}

type R<T> = Result<T, ApiError>;

fn bad(msg: &str) -> ApiError {
    ApiError(xmuhub_core::error::bad(msg))
}

/// Runs a (possibly fsync-ing or password-hashing) call off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> xmuhub_core::Result<T> + Send + 'static) -> R<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(Error::Internal(e.to_string())))?
        .map_err(ApiError)
}

pub async fn csrf_guard(req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    // Bearer-token clients (scripts, agents) can't be driven cross-site: browsers never attach
    // an Authorization header to a cross-site request without a CORS preflight we don't allow.
    let bearer = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("Bearer "));
    if !safe && !bearer && req.uri().path().starts_with("/api/") && !req.headers().contains_key(CSRF_HEADER) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "缺少请求头，请刷新页面后重试" }))).into_response();
    }
    next.run(req).await
}

// ------------------------------------------------------------------ auth

fn cookie_value<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The signed-in user, if any. Invalid or expired sessions are treated as guests.
pub struct Auth {
    pub user: Option<User>,
    pub session: Option<String>,
}

impl Auth {
    pub fn viewer(&self) -> Viewer<'_> {
        Viewer { user: self.user.as_ref() }
    }
    fn require(&self) -> R<&User> {
        self.user.as_ref().ok_or(ApiError(Error::Unauthorized))
    }
}

impl FromRequestParts<Arc<App>> for Auth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Self::Rejection> {
        // A bearer token, when present, is the only credential considered (never the cookie).
        if let Some(given) = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
            let given = given.trim();
            if app.script_token.as_deref().is_some_and(|t| constant_eq(t.as_bytes(), given.as_bytes())) {
                return Ok(Auth { user: app.hub.system_user().ok(), session: None });
            }
            return Ok(Auth { user: app.hub.token_user(given), session: None });
        }
        let session = cookie_value(&parts.headers, SESSION_COOKIE).or_else(|| cookie_value(&parts.headers, LEGACY_SESSION_COOKIE)).map(str::to_string);
        let user = session.as_deref().and_then(|s| app.hub.session_user(s));
        Ok(Auth { user, session })
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The visitor's address as nginx saw it (X-Real-IP, which nginx sets itself and takes
/// from Cloudflare only for Cloudflare's own ranges). X-Forwarded-For is never trusted: its
/// first entry is whatever the client wrote. We only ever listen on loopback.
pub fn real_ip(h: &HeaderMap) -> String {
    h.get("x-real-ip").and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "local".into())
}

/// The key per-visitor limits count by: the IPv4 address, or the /64 an IPv6 address is in
/// (one household or phone gets a whole /64 and can hop inside it freely).
pub fn client_ip(h: &HeaderMap) -> String {
    let ip = real_ip(h);
    match ip.parse::<std::net::Ipv6Addr>() {
        Ok(v6) if v6.to_ipv4_mapped().is_none() => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => ip,
    }
}

fn session_cookie(app: &App, secret: &str, max_age: i64) -> HeaderValue {
    // `__Host-` cookies must be Secure, which plain-HTTP development can't set.
    let (name, secure) = if app.secure_cookie { (SESSION_COOKIE, "; Secure") } else { (LEGACY_SESSION_COOKIE, "") };
    HeaderValue::from_str(&format!("{name}={secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")).unwrap()
}

/// Clears a pre-`__Host-` session cookie (sent alongside a new one, and on sign-out).
fn legacy_cookie_gone() -> HeaderValue {
    HeaderValue::from_static("xh_sid=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

fn with_session(app: &App, secret: &str, body: Value) -> Response {
    let mut res = Json(body).into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(app, secret, xmuhub_core::hub::SESSION_TTL));
    if app.secure_cookie {
        res.headers_mut().append(header::SET_COOKIE, legacy_cookie_gone());
    }
    res
}

// ------------------------------------------------------------------ views

fn node_brief(n: &Node) -> Value {
    json!({ "id": n.id, "name": n.name, "code": n.code, "label": n.label, "kind": n.kind.as_str() })
}

pub(crate) fn node_view(hub: &Hub, n: &Node, count: usize) -> Value {
    // Study level: `own_level` is set on this node, `level` includes what it inherits.
    let (own_level, level) = hub.node_level(n.id);
    json!({
        "id": n.id, "parent": n.parent, "kind": n.kind.as_str(), "code": n.code, "name": n.name,
        "label": n.label, "aliases": n.aliases, "bucketed": n.bucketed, "sort": n.sort, "count": count,
        "level": level, "own_level": own_level,
        "status": match n.status { NodeStatus::Pending => "pending", NodeStatus::Active => "active", NodeStatus::Merged(_) => "merged" },
    })
}

fn tag_view(t: Tag) -> Value {
    json!({ "code": t.code(), "label": t.label(), "bucket": t.bucket() })
}

fn user_view(u: &User) -> Value {
    json!({
        "id": u.id, "email": u.email, "nickname": u.nickname, "level": u.level as u8, "banned": u.banned,
        "created_at": u.created_at, "last_login": u.last_login, "uploads": u.uploads, "xmu": u.xmu_verified(),
    })
}

pub(crate) fn resource_view(app: &App, r: &Resource, node: &Node, path: &[Node], viewer: Viewer) -> Value {
    let mine = viewer.id() == Some(r.uploader);
    let staff = viewer.staff();
    let mut v = json!({
        "id": r.id,
        "title": r.name.stem(),
        "subtitle": app.hub.subtitle(r),
        "major": app.hub.major_of(r.id),
        "filename": r.filename(),
        "ext": r.ext,
        "name": {
            "course": r.name.course, "time": r.name.time, "type_word": r.name.type_word, "paper": r.name.paper,
            "with_answer": r.name.with_answer, "extra": r.name.extra, "version": r.name.version,
        },
        "tag": tag_view(r.tag),
        "node": node_brief(node),
        "path": path.iter().map(node_brief).collect::<Vec<_>>(),
        "note": r.note,
        "size": r.size,
        "mime": r.mime,
        "status": r.status.as_str(),
        // "Awaiting re-review" is internal; the public just sees a published resource.
        "needs_review": r.needs_review && (mine || staff),
        "review_note": if mine || staff { r.review_note.as_str() } else { "" },
        "created_at": r.created_at,
        "updated_at": r.updated_at,
        "downloads": r.downloads,
        "rating": app.hub.rating(r.id),
        "thumb": thumb_urls(app, &r.blob),
        "mine": mine,
    });
    if staff {
        // 分类规则 §5.6–5.7: uploader, source and original name are staff-only.
        v["original_name"] = json!(r.original_name);
        v["source"] = json!(r.source);
        v["uncertain"] = json!(r.uncertain);
        v["uploader"] = json!({ "id": r.uploader, "nickname": app.hub.uploader_name(r.uploader), "avatar": app.hub.avatar_of(r.uploader) });
        if let Some(by) = r.reviewed_by {
            v["reviewer"] = json!({ "id": by, "nickname": app.hub.uploader_name(by) });
        }
    } else if app.hub.public_uploader(r.uploader) {
        // The uploader chose to show their nickname (「我的」 page); no id for the public.
        v["uploader"] = json!({ "nickname": app.hub.uploader_name(r.uploader), "avatar": app.hub.avatar_of(r.uploader) });
    }
    if mine || staff {
        // Reviewers' questions to the uploader and the answers (「问上传者」).
        v["questions"] = json!(app.hub.questions_about(viewer, r.id));
    }
    v
}

/// A few ways to load a blob's thumbnail: the two fastest mirrors, then GitHub directly.
fn thumb_urls(app: &App, key: &str) -> Vec<String> {
    let Some(loc) = app.hub.thumb_of(key) else { return Vec::new() };
    let urls = app.hub.storage.download_urls(std::slice::from_ref(&loc));
    let mut out: Vec<String> = urls.iter().take(2).cloned().collect();
    if let Some(last) = urls.last()
        && !out.contains(last)
    {
        out.push(last.clone());
    }
    out
}

fn list(app: &App, v: Vec<(Resource, Node)>, viewer: Viewer) -> Json<Value> {
    Json(json!(v.iter().map(|(r, n)| resource_view(app, r, n, &[], viewer)).collect::<Vec<_>>()))
}

// ------------------------------------------------------------------ routes

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/meta", get(meta))
        .route("/me", get(me).patch(update_me))
        .route("/me/tokens", get(list_tokens).post(create_token))
        .route("/me/avatar", post(set_avatar).delete(clear_avatar))
        .route("/inbox", post(inbox))
        .route("/me/tokens/{id}", axum::routing::delete(revoke_token))
        .route("/auth/code", post(send_code))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/reset", post(reset))
        .route("/tree", get(tree))
        .route("/nodes", post(create_node))
        .route("/nodes/suggest", get(suggest))
        .route("/nodes/{id}", get(node).patch(patch_node).delete(delete_node))
        .route("/nodes/{id}/merge", post(merge_node))
        .route("/search", get(search))
        .route("/recent", get(recent))
        .route("/popular", get(popular))
        .route("/mine", get(mine))
        .route("/resources", post(create_resource))
        .route("/resources/preview-name", post(preview_name))
        .route("/resources/move", post(move_resources))
        .route("/resources/{id}", get(resource).patch(patch_resource))
        .route("/resources/{id}/change-request", get(resource_change_request).post(request_resource_change))
        .route("/resources/{id}/review", post(review))
        .route("/resources/{id}/report", post(report))
        .route("/resources/{id}/download", get(download_plan))
        .route("/resources/{id}/social", get(social))
        .route("/resources/{id}/rating", put(rate))
        .route("/resources/{id}/comments", post(add_comment))
        .route("/resources/{id}/reviews", get(resource_reviews))
        .route("/comments/{id}", axum::routing::delete(delete_comment))
        .route("/feedback", post(feedback))
        .route("/feedback/mine", get(my_feedback))
        .route("/uploads", post(begin_upload))
        .route("/uploads/{id}", get(upload_plan))
        .route("/uploads/{id}/parts/{index}", post(confirm_part))
        .route("/uploads/{id}/parts/{index}/renew", post(renew_part))
        .route("/review", get(review_queue))
        .route("/review/change-requests", get(resource_change_requests))
        .route("/review/change-requests/{id}", post(review_resource_change))
        .route("/review/nodes", get(pending_nodes))
        .route("/review/batch", get(review_batch).post(take_review_batch))
        .route("/review/batch/release", post(release_review_batch))
        .route("/review/batch/skip/{id}", post(skip_in_review_batch))
        .route("/resources/{id}/questions", post(ask_uploader))
        .route("/questions/{id}/answer", post(answer_question))
        .route("/me/questions", get(my_questions))
        .route("/links", get(links).post(add_link))
        .route("/links/{id}", axum::routing::patch(update_link).delete(delete_link))
        .route("/links/suggestions", get(link_suggestions).post(suggest_link))
        .route("/links/suggestions/{id}/review", post(review_link_suggestion))
        .route("/wants", get(wants).post(add_want))
        .route("/wants/{id}", get(want))
        .route("/wants/{id}/review", post(review_want))
        .route("/wants/{id}/status", post(resolve_want))
        .route("/wants/{id}/vote", put(vote_want))
        .route("/wants/{id}/replies", post(reply_want))
        .route("/want-replies/{id}", axum::routing::delete(delete_want_reply))
        .route("/announcement", put(set_announcement))
        .route("/stats", get(site_stats))
        .route("/notices", get(notices))
        .route("/notices/read", post(read_notices))
        .route("/nodes/{id}/follow", get(follow_state).put(follow))
        .route("/me/follows", get(my_follows))
        .route("/collections", get(collections).post(create_collection))
        .route("/collections/{id}", get(collection).patch(update_collection).delete(delete_collection))
        .route("/collections/{id}/items/{resource}", put(collect))
        .route("/collections/{id}/share", post(share_collection))
        .route("/collections/{id}/review", post(review_collection))
        .route("/review/collections", get(pending_collections))
        .route("/resources/{id}/collections", get(collections_with))
        .route("/admin/avatars", get(pending_avatars))
        .route("/admin/avatars/{user}", post(review_avatar))
        .route("/admin/reports", get(reports))
        .route("/admin/reports/{id}/handle", post(handle_report))
        .route("/admin/reports/{id}", axum::routing::delete(delete_report))
        .route("/admin/feedback", get(feedback_list))
        .route("/admin/feedback/{id}/handle", post(handle_feedback))
        .route("/admin/reviews", get(review_log))
        .route("/admin/daily", get(daily))
        .route("/admin/users", get(users))
        .route("/admin/users/{id}", axum::routing::patch(update_user))
        .route("/admin/status", get(status))
        .route("/admin/github/scan", post(gh_scan))
        .route("/admin/github/import", post(gh_import))
        .route("/admin/github/transfer", get(gh_transfer).post(gh_transfer_kick))
        .route("/admin/thumbs", get(thumbs_status).post(thumbs_kick))
        .route("/relay/upload", post(crate::relay::upload).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/local/upload", put(local_upload).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/local/file/{name}", get(local_file))
        .fallback(|| async { ApiError(Error::NotFound("接口")) });

    Router::new()
        .nest("/api", api)
        .route("/d/{id}", get(download_redirect))
        .route("/mcp", post(crate::mcp::post).get(crate::mcp::get))
        .route("/", get(|s: S, h: HeaderMap| async move { page(s, "index.html", h) }))
        .route("/n/{id}", get(|s: S, h: HeaderMap| async move { page(s, "node.html", h) }))
        .route("/r/{id}", get(|s: S, h: HeaderMap| async move { page(s, "resource.html", h) }))
        .route("/{*path}", get(static_file))
        .layer(axum::middleware::from_fn(csrf_guard))
        .layer(axum::middleware::from_fn(crate::ratelimit::guard))
        .with_state(app)
}

fn page(State(app): S, file: &str, h: HeaderMap) -> Response {
    app.site.respond(file, None, &h, StatusCode::OK)
}

#[derive(Deserialize)]
struct StaticQ {
    /// Content key the page linked the file with (see versioning.rs).
    v: Option<String>,
}

async fn static_file(State(app): S, Path(path): Path<String>, Query(q): Query<StaticQ>, h: HeaderMap) -> Response {
    // Clean URLs: /search → search.html.
    let path = if app.site.get(&path).is_none() && app.site.get(&format!("{path}.html")).is_some() { format!("{path}.html") } else { path };
    app.site.respond(&path, q.v.as_deref(), &h, StatusCode::OK)
}

// ------------------------------------------------------------------ meta & account

async fn meta(State(app): S) -> Json<Value> {
    let l = &app.hub.limits;
    Json(json!({
        "tags": Tag::ALL.iter().map(|t| tag_view(*t)).collect::<Vec<_>>(),
        "type_words": TYPE_WORDS.iter().map(|(w, t)| json!({ "word": w, "tag": t.code() })).collect::<Vec<_>>(),
        "papers": ["A卷", "B卷", "C卷"],
        "levels": ["访客", "贡献者", "可信贡献者", "审核员", "管理员"],
        "limits": { "max_file": l.max_file, "max_part": l.max_part },
        "stats": app.hub.stats(),
        "upload_via": if app.worker_url.is_empty() { "relay" } else { "worker" },
        "mail": app.mailer.is_some(),
        "announcement": app.hub.announcement(),
    }))
}

async fn me(State(app): S, auth: Auth) -> Json<Value> {
    Json(json!({ "user": auth.user.as_ref().map(|u| me_view(&app, u)) }))
}

/// The signed-in user as they see themselves (with their own settings).
fn me_view(app: &App, u: &User) -> Value {
    let mut v = user_view(u);
    v["public_name"] = json!(app.hub.public_uploader(u.id));
    let (avatar, pending) = app.hub.own_avatar(u.id);
    v["avatar"] = json!(avatar);
    v["avatar_pending"] = json!(pending);
    v["unread"] = json!(app.hub.unread_notices(u.id));
    v
}

#[derive(Deserialize)]
struct CodeIn {
    email: String,
    #[serde(default)]
    purpose: String,
    /// Turnstile token, for sign-ups that need a human check.
    #[serde(default)]
    captcha: String,
}

async fn send_code(State(app): S, h: HeaderMap, Json(b): Json<CodeIn>) -> R<Response> {
    let purpose = if b.purpose == "reset" { CodePurpose::Reset } else { CodePurpose::Register };
    // Signing up with an uncommon mail domain (or a +alias) takes a human check first; the
    // answer names the site key so the page can show the widget and ask again.
    if purpose == CodePurpose::Register
        && let Some(t) = &app.turnstile
        && xmuhub_core::hub::normalize_email(&b.email).is_ok_and(|e| xmuhub_core::hub::needs_captcha(&e))
        && !t.verify(&b.captcha, &real_ip(&h)).await
    {
        let msg = if b.captcha.is_empty() { "用这个邮箱注册需要先完成人机验证" } else { "人机验证没有通过，请再试一次" };
        return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": msg, "captcha": t.sitekey }))).into_response());
    }
    let mailer = app.mailer.as_ref().ok_or_else(|| bad("站点暂未开放邮件验证，请联系管理员"))?;
    let (email, code) = app.hub.request_code(&b.email, purpose, &client_ip(&h))?;
    if let Err(e) = mailer.send_code(&email, &code, if purpose == CodePurpose::Reset { "reset" } else { "register" }).await {
        tracing::warn!("send code to {email}: {e}");
        app.hub.cancel_code(&email, purpose);
        return Err(ApiError(Error::Upstream("验证码邮件发送失败，请稍后重试".into())));
    }
    Ok(Json(json!({ "sent": true })).into_response())
}

#[derive(Deserialize)]
struct RegisterIn {
    email: String,
    code: String,
    password: String,
    nickname: String,
}

async fn register(State(app): S, h: HeaderMap, Json(b): Json<RegisterIn>) -> R<Response> {
    let hub = app.hub.clone();
    let ip = client_ip(&h);
    let (secret, u) = blocking(move || hub.register(Registration { email: b.email, code: b.code, password: b.password, nickname: b.nickname }, &ip)).await?;
    Ok(with_session(&app, &secret, json!({ "user": user_view(&u) })))
}

#[derive(Deserialize)]
struct LoginIn {
    email: String,
    password: String,
}

async fn login(State(app): S, h: HeaderMap, Json(b): Json<LoginIn>) -> R<Response> {
    let hub = app.hub.clone();
    let ip = client_ip(&h);
    let (secret, u) = blocking(move || hub.login(&b.email, &b.password, &ip)).await?;
    Ok(with_session(&app, &secret, json!({ "user": user_view(&u) })))
}

async fn logout(State(app): S, auth: Auth) -> R<Response> {
    if let Some(s) = auth.session {
        let hub = app.hub.clone();
        blocking(move || hub.logout(&s)).await?;
    }
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(&app, "", 0));
    res.headers_mut().append(header::SET_COOKIE, legacy_cookie_gone());
    Ok(res)
}

#[derive(Deserialize)]
struct ResetIn {
    email: String,
    code: String,
    password: String,
}

async fn reset(State(app): S, h: HeaderMap, Json(b): Json<ResetIn>) -> R<Response> {
    let hub = app.hub.clone();
    let ip = client_ip(&h);
    let (secret, u) = blocking(move || hub.reset_password(&b.email, &b.code, &b.password, &ip)).await?;
    Ok(with_session(&app, &secret, json!({ "user": user_view(&u) })))
}

#[derive(Deserialize)]
struct MeIn {
    nickname: Option<String>,
    old_password: Option<String>,
    new_password: Option<String>,
    /// Show my nickname on the files I uploaded.
    public_name: Option<bool>,
}

async fn update_me(State(app): S, auth: Auth, Json(b): Json<MeIn>) -> R<Json<Value>> {
    let me = auth.require()?.clone();
    let session = auth.session.clone().unwrap_or_default();
    let hub = app.hub.clone();
    let u = blocking(move || {
        if let Some(p) = b.public_name {
            hub.set_public_uploader(Viewer { user: Some(&me) }, p)?;
        }
        if b.nickname.is_none() && b.new_password.is_none() {
            return Ok(me);
        }
        hub.update_profile(&me, &session, b.nickname.as_deref(), b.old_password.as_deref(), b.new_password.as_deref())
    })
    .await?;
    Ok(Json(json!({ "user": me_view(&app, &u) })))
}

// ------------------------------------------------------------------ personal access tokens

async fn list_tokens(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.tokens(auth.viewer())?)))
}

#[derive(Deserialize)]
struct TokenIn {
    #[serde(default)]
    name: String,
}

async fn create_token(State(app): S, auth: Auth, Json(b): Json<TokenIn>) -> R<Json<Value>> {
    // Tokens can't mint tokens: creating one needs a signed-in browser session.
    if auth.session.is_none() {
        return Err(ApiError(Error::Forbidden));
    }
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let (secret, t) = blocking(move || hub.create_token(Viewer { user: user.as_ref() }, &b.name)).await?;
    Ok(Json(json!({ "secret": secret, "token": t })))
}

async fn revoke_token(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.revoke_token(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------------ tree

/// The hidden 「待整理」 node for uploads that the uploader couldn't place (created on first use).
async fn inbox(State(app): S, auth: Auth) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let n = blocking(move || hub.inbox(Viewer { user: user.as_ref() })).await?;
    Ok(Json(node_view(&app.hub, &n, 0)))
}

#[derive(Deserialize)]
struct AvatarIn {
    upload_id: Id,
}

/// The picture itself was uploaded like any file (`/uploads`); this makes it the avatar.
async fn set_avatar(State(app): S, auth: Auth, Json(b): Json<AvatarIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.set_avatar(Viewer { user: user.as_ref() }, b.upload_id)).await?;
    let me = auth.user.as_ref().ok_or(Error::Unauthorized)?;
    Ok(Json(me_view(&app, me)))
}

async fn clear_avatar(State(app): S, auth: Auth) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.clear_avatar(Viewer { user: user.as_ref() })).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn pending_avatars(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.pending_avatars(auth.viewer())?)))
}

#[derive(Deserialize)]
struct AvatarReviewIn {
    action: String,
}

async fn review_avatar(State(app): S, auth: Auth, Path(user): Path<Id>, Json(b): Json<AvatarReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let viewer = auth.user.clone();
    blocking(move || hub.review_avatar(Viewer { user: viewer.as_ref() }, user, &b.action)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn tree(State(app): S) -> Json<Value> {
    Json(json!(app.hub.tree().iter().map(|i| node_view(&app.hub, &i.node, i.count)).collect::<Vec<_>>()))
}

async fn node(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let (info, path, children, resources) = app.hub.node(auth.viewer(), id)?;
    let v = auth.viewer();
    Ok(Json(json!({
        "node": node_view(&app.hub, &info.node, info.count),
        "path": path.iter().map(node_brief).collect::<Vec<_>>(),
        "children": children.iter().map(|c| node_view(&app.hub, &c.node, c.count)).collect::<Vec<_>>(),
        "resources": resources.iter().map(|r| resource_view(&app, r, &info.node, &[], v)).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Q {
    q: Option<String>,
}

async fn suggest(State(app): S, Query(q): Query<Q>) -> Json<Value> {
    let v: Vec<Value> = app
        .hub
        .suggest_nodes(q.q.as_deref().unwrap_or(""), 12)
        .iter()
        .map(|(n, path)| json!({ "node": node_view(&app.hub, n, 0), "path": path.iter().map(node_brief).collect::<Vec<_>>() }))
        .collect();
    Json(json!(v))
}

async fn create_node(State(app): S, auth: Auth, Json(b): Json<NodeInput>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let n = blocking(move || hub.create_node(Viewer { user: user.as_ref() }, b)).await?;
    Ok(Json(node_view(&app.hub, &n, 0)))
}

async fn patch_node(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<NodePatch>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let n = blocking(move || hub.update_node(Viewer { user: user.as_ref() }, id, b)).await?;
    Ok(Json(node_view(&app.hub, &n, 0)))
}

#[derive(Deserialize)]
struct MergeIn {
    into: Id,
}

async fn merge_node(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<MergeIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let n = blocking(move || hub.merge_node(Viewer { user: user.as_ref() }, id, b.into)).await?;
    Ok(Json(node_view(&app.hub, &n, 0)))
}

async fn delete_node(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_node(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------------ search & lists

#[derive(Deserialize)]
struct SearchQ {
    q: Option<String>,
    #[serde(rename = "type")]
    ty: Option<String>,
    tag: Option<String>,
    #[serde(default)]
    name_only: bool,
    /// "ug" (本科, includes untagged courses) or "grad" (研究生).
    level: Option<String>,
    // Strings, so an empty `within=` / `page=` from a form means "unset" instead of a 400.
    within: Option<String>,
    page: Option<String>,
}

async fn search(State(app): S, auth: Auth, Query(q): Query<SearchQ>) -> R<Json<Value>> {
    const PAGE: usize = 20;
    let courses_only = q.ty.as_deref() == Some("course");
    let filter = Filter {
        ty: match q.ty.as_deref() {
            Some("node" | "course") => Some(DocType::Node),
            Some("resource") => Some(DocType::Resource),
            _ => None,
        },
        courses_only,
        name_only: courses_only || q.name_only,
        within: q.within.as_deref().and_then(|w| w.trim().parse().ok()),
        level: match q.level.as_deref() {
            Some("ug") => Some(xmuhub_core::hub::LEVEL_UNDERGRAD),
            Some("grad") => Some(xmuhub_core::hub::LEVEL_GRADUATE),
            _ => None,
        },
        tag: q.tag.as_deref().and_then(Tag::parse).and_then(|t| Tag::ALL.iter().position(|x| *x == t)).map(|i| i as u64),
    };
    let page = q.page.as_deref().and_then(|p| p.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 50);
    let v = auth.viewer();
    // Off the two async worker threads: a search takes CPU time.
    let (hub, user, text) = (app.hub.clone(), auth.user.clone(), q.q.clone().unwrap_or_default());
    let (items, total) = blocking(move || hub.search(Viewer { user: user.as_ref() }, &text, filter, PAGE, (page - 1) * PAGE)).await?;
    let items: Vec<Value> = items
        .iter()
        .map(|i| match i {
            SearchItem::Node { node, path, count } => {
                json!({ "type": "node", "node": node_view(&app.hub, node, *count), "path": path.iter().map(node_brief).collect::<Vec<_>>() })
            }
            SearchItem::Resource { resource, node, path } => json!({ "type": "resource", "resource": resource_view(&app, resource, node, path, v) }),
        })
        .collect();
    Ok(Json(json!({ "items": items, "total": total, "page": page, "page_size": PAGE })))
}

async fn recent(State(app): S, auth: Auth) -> Json<Value> {
    list(&app, app.hub.recent(12), auth.viewer())
}

async fn popular(State(app): S, auth: Auth) -> Json<Value> {
    list(&app, app.hub.popular(12), auth.viewer())
}

#[derive(Deserialize)]
struct MineQ {
    /// Matches title, category name, note or the original file name (case-insensitive; these are
    /// the caller's own files, so their original names are theirs to search).
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

/// The caller's uploads, newest first, a page at a time (some people have thousands: sending
/// them all at once froze phones).
async fn mine(State(app): S, auth: Auth, Query(q): Query<MineQ>) -> R<Json<Value>> {
    let all = app.hub.my_resources(auth.viewer())?;
    let total = all.len();
    let needle = q.q.trim().to_lowercase();
    let hits: Vec<&(Resource, Node)> = all
        .iter()
        .filter(|(r, _)| q.status.is_empty() || r.status.as_str() == q.status)
        .filter(|(r, n)| {
            needle.is_empty() || [r.name.stem(), n.name.clone(), r.note.clone(), r.original_name.clone()].iter().any(|x| x.to_lowercase().contains(&needle))
        })
        .collect();
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let v = auth.viewer();
    let items: Vec<Value> = hits.iter().skip(q.offset).take(limit).map(|(r, n)| resource_view(&app, r, n, &[], v)).collect();
    Ok(Json(json!({ "items": items, "matched": hits.len(), "total": total })))
}

// ------------------------------------------------------------------ resources

async fn resource(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let (r, n, path) = app.hub.resource(auth.viewer(), id)?;
    Ok(Json(resource_view(&app, &r, &n, &path, auth.viewer())))
}

#[derive(Deserialize)]
struct CreateResource {
    upload_id: Id,
    #[serde(flatten)]
    input: ResourceInput,
    #[serde(default)]
    admin: Option<AdminExtras>,
}

async fn create_resource(State(app): S, auth: Auth, Json(b): Json<CreateResource>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let r = blocking(move || hub.create_resource(Viewer { user: user.as_ref() }, b.upload_id, b.input, b.admin.unwrap_or_default())).await?;
    let (r, n, path) = app.hub.resource(auth.viewer(), r.id)?;
    Ok(Json(resource_view(&app, &r, &n, &path, auth.viewer())))
}

#[derive(Deserialize)]
struct PreviewIn {
    #[serde(flatten)]
    input: ResourceInput,
    #[serde(default)]
    ext: String,
}

async fn preview_name(State(app): S, Json(b): Json<PreviewIn>) -> R<Json<Value>> {
    Ok(Json(json!({ "filename": app.hub.preview_name(&b.input, &b.ext)? })))
}

#[derive(Deserialize)]
struct PatchResource {
    #[serde(flatten)]
    input: ResourceInput,
    #[serde(default)]
    admin: Option<AdminExtras>,
}

async fn patch_resource(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<PatchResource>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.update_resource(Viewer { user: user.as_ref() }, id, b.input, b.admin.unwrap_or_default())).await?;
    let (r, n, path) = app.hub.resource(auth.viewer(), id)?;
    Ok(Json(resource_view(&app, &r, &n, &path, auth.viewer())))
}

#[derive(Deserialize)]
struct ReviewIn {
    action: String,
    #[serde(default)]
    note: String,
}

async fn review(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<ReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let r = blocking(move || hub.review(Viewer { user: user.as_ref() }, id, &b.action, &b.note)).await?;
    Ok(Json(json!({ "status": r.status.as_str() })))
}

#[derive(Deserialize)]
struct ResourceChangeIn {
    kind: String,
    #[serde(default)]
    value: String,
}

async fn request_resource_change(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<ResourceChangeIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let q = blocking(move || hub.request_resource_change(Viewer { user: user.as_ref() }, id, &b.kind, &b.value)).await?;
    Ok(Json(json!(q)))
}

async fn resource_change_request(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.resource_change_request(auth.viewer(), id)?)))
}

async fn resource_change_requests(State(app): S, auth: Auth) -> R<Json<Value>> {
    let items = app.hub.pending_resource_changes(auth.viewer())?;
    Ok(Json(json!(items.iter().map(|(q, r)| json!({
        "request": q, "title": r.name.stem(), "current_note": r.note,
        "uploader": app.hub.uploader_name(q.uploader),
    })).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct ResourceChangeReviewIn {
    approve: bool,
    #[serde(default)]
    note: String,
}

async fn review_resource_change(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<ResourceChangeReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let q = blocking(move || hub.review_resource_change(Viewer { user: user.as_ref() }, id, b.approve, &b.note)).await?;
    Ok(Json(json!(q)))
}

#[derive(Deserialize)]
struct ReportIn {
    reason: String,
}

async fn report(State(app): S, auth: Auth, h: HeaderMap, Path(id): Path<Id>, Json(b): Json<ReportIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let ip = client_ip(&h);
    blocking(move || hub.report(Viewer { user: user.as_ref() }, id, &b.reason, &ip)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PlanQ {
    #[serde(default)]
    peek: Option<String>,
}

/// `?peek=1` is for in-page previews: not counted as a download, and each part lists the
/// mirrors that allow cross-origin reads. File bytes never pass through this server (see
/// AGENTS.md): if no mirror is readable, the page says so and offers the download instead.
async fn download_plan(State(app): S, auth: Auth, h: HeaderMap, Path(id): Path<Id>, Query(q): Query<PlanQ>) -> R<Json<Value>> {
    let peek = matches!(q.peek.as_deref(), Some("1" | "true"));
    let who = client_ip(&h);
    let plan = app.hub.download(auth.viewer(), id, (!peek).then_some(who.as_str()))?;
    let mut v = json!(plan);
    if peek {
        for (i, p) in plan.parts.iter().enumerate() {
            v["parts"][i]["preview_urls"] = json!(app.mirrors.cors_urls(&p.urls));
        }
    }
    Ok(Json(v))
}

// ------------------------------------------------------------------ ratings, comments, feedback

async fn social(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let v = auth.viewer();
    let comments = app.hub.comments(v, id)?;
    Ok(Json(json!({ "rating": app.hub.rating(id), "my_rating": app.hub.my_rating(v, id), "comments": comments })))
}

#[derive(Deserialize)]
struct RateIn {
    stars: u8,
}

async fn rate(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<RateIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let sum = blocking(move || hub.rate(Viewer { user: user.as_ref() }, id, b.stars)).await?;
    Ok(Json(json!({ "rating": sum, "my_rating": b.stars })))
}

#[derive(Deserialize)]
struct CommentIn {
    body: String,
}

async fn add_comment(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<CommentIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let c = blocking(move || hub.add_comment(Viewer { user: user.as_ref() }, id, &b.body)).await?;
    Ok(Json(json!({ "id": c.id })))
}

async fn delete_comment(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_comment(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct FeedbackIn {
    body: String,
    #[serde(default)]
    contact: String,
    #[serde(default)]
    page: String,
}

async fn feedback(State(app): S, auth: Auth, h: HeaderMap, Json(b): Json<FeedbackIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let ip = client_ip(&h);
    let f = blocking(move || hub.submit_feedback(Viewer { user: user.as_ref() }, &b.body, &b.contact, &b.page, &ip)).await?;
    // The receipt lets the sender (even without an account) see the reply later.
    Ok(Json(json!({ "ok": true, "id": f.id, "key": xmuhub_core::ticket::receipt(&app.secret, "feedback", f.id) })))
}

#[derive(Deserialize)]
struct MyFeedbackQ {
    /// `id.key` pairs from feedback sent in this browser, comma separated.
    #[serde(default)]
    k: String,
}

/// The sender's own feedback with the admins' replies: what this browser sent (by receipt)
/// and, when signed in, everything sent from the account.
async fn my_feedback(State(app): S, auth: Auth, Query(q): Query<MyFeedbackQ>) -> Json<Value> {
    let ids: Vec<Id> = q
        .k
        .split(',')
        .take(50)
        .filter_map(|p| p.split_once('.'))
        .filter_map(|(id, key)| id.parse::<Id>().ok().filter(|id| xmuhub_core::ticket::check_receipt(&app.secret, "feedback", *id, key)))
        .collect();
    Json(json!(app.hub.my_feedback(auth.viewer(), &ids)))
}

async fn feedback_list(State(app): S, auth: Auth, Query(q): Query<ReportsQ>) -> R<Json<Value>> {
    let items: Vec<Value> = app
        .hub
        .feedback(auth.viewer(), q.all)?
        .into_iter()
        .map(|(f, nick)| {
            json!({
                "id": f.id, "body": f.body, "contact": f.contact, "page": f.page, "nickname": nick,
                "created_at": f.created_at, "handled": f.handled, "handled_note": f.handled_note,
                "handled_by": f.handled_by.map(|u| app.hub.uploader_name(u)),
            })
        })
        .collect();
    Ok(Json(json!(items)))
}

async fn handle_feedback(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<HandleIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.handle_feedback(Viewer { user: user.as_ref() }, id, &b.note)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MoveIn {
    ids: Vec<Id>,
    node: Id,
}

async fn move_resources(State(app): S, auth: Auth, Json(b): Json<MoveIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let moved = blocking(move || hub.move_resources(Viewer { user: user.as_ref() }, &b.ids, b.node)).await?;
    Ok(Json(json!({ "moved": moved })))
}

#[derive(Deserialize)]
struct DaysQ {
    days: Option<usize>,
}

async fn daily(State(app): S, auth: Auth, Query(q): Query<DaysQ>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.daily_stats(auth.viewer(), q.days.unwrap_or(30))?)))
}

async fn review_log(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.review_log(auth.viewer(), None, 300)?)))
}

async fn resource_reviews(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.review_log(auth.viewer(), Some(id), 50)?)))
}

/// Plain link for sharing / wget: 302 to the best mirror of a single-part file.
async fn download_redirect(State(app): S, auth: Auth, h: HeaderMap, Path(id): Path<Id>) -> R<Response> {
    let plan = app.hub.download(auth.viewer(), id, Some(&client_ip(&h)))?;
    if plan.parts.len() != 1 {
        // Multi-part files need the page to stitch them together.
        return Ok(Redirect::to(&format!("/r/{id}")).into_response());
    }
    let url = plan.parts[0].urls.first().cloned().ok_or(Error::NotFound("下载地址"))?;
    Ok(Redirect::to(&url).into_response())
}

// ------------------------------------------------------------------ uploads

#[derive(Deserialize)]
struct BeginUpload {
    filename: String,
    #[serde(default)]
    mime: String,
    parts: Vec<PartSpec>,
}

async fn begin_upload(State(app): S, auth: Auth, Json(b): Json<BeginUpload>) -> R<Json<Value>> {
    let plan = app.hub.begin_upload(auth.viewer(), &b.filename, &b.mime, b.parts).await?;
    Ok(Json(json!(plan)))
}

async fn upload_plan(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.upload_plan(auth.viewer(), id)?)))
}

#[derive(Deserialize)]
struct ReceiptIn {
    asset_id: Option<u64>,
}

async fn confirm_part(State(app): S, auth: Auth, Path((id, index)): Path<(Id, usize)>, Json(r): Json<ReceiptIn>) -> R<Json<Value>> {
    let finished = app.hub.confirm_part(auth.viewer(), id, index, Receipt { asset_id: r.asset_id }).await?;
    Ok(Json(json!({ "finished": finished })))
}

#[derive(Deserialize)]
struct RenewQ {
    /// "relay": the Worker failed, send this part through the server instead.
    via: Option<String>,
}

async fn renew_part(State(app): S, auth: Auth, Path((id, index)): Path<(Id, usize)>, Query(q): Query<RenewQ>) -> R<Json<Value>> {
    let fallback = q.via.as_deref() == Some("relay");
    Ok(Json(json!(app.hub.renew_part(auth.viewer(), id, index, fallback).await?)))
}

// ------------------------------------------------------------------ review & admin

#[derive(Deserialize)]
struct QueueQ {
    status: Option<String>,
    #[serde(default)]
    uncertain: bool,
}

async fn review_queue(State(app): S, auth: Auth, Query(q): Query<QueueQ>) -> R<Json<Value>> {
    let v = auth.viewer();
    let items = app.hub.review_queue(v, q.status.as_deref(), q.uncertain)?;
    Ok(Json(json!(items.iter().map(|(r, n)| resource_view(&app, r, n, &[], v)).collect::<Vec<_>>())))
}

// ------------------------------------------------------------------ 站外资源 (outside sources)

// ------------------------------------------------------------------ 求资料, announcement

#[derive(Deserialize)]
struct WantsQ {
    #[serde(default)]
    status: String,
    node: Option<Id>,
}

async fn wants(State(app): S, auth: Auth, Query(q): Query<WantsQ>) -> R<Json<Value>> {
    let filter = match q.status.as_str() {
        "found" => WantFilter::Found,
        "mine" => WantFilter::Mine,
        "pending" => WantFilter::Pending,
        _ => WantFilter::Open,
    };
    Ok(Json(json!(app.hub.wants(auth.viewer(), filter, q.node)?)))
}

async fn want(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let (w, replies) = app.hub.want(auth.viewer(), id)?;
    Ok(Json(json!({ "want": w, "replies": replies })))
}

#[derive(Deserialize)]
struct WantIn {
    title: String,
    #[serde(default)]
    body: String,
    node: Option<Id>,
}

async fn add_want(State(app): S, auth: Auth, Json(b): Json<WantIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let w = blocking(move || hub.add_want(Viewer { user: user.as_ref() }, &b.title, &b.body, b.node)).await?;
    Ok(Json(json!(w)))
}

#[derive(Deserialize)]
struct WantReviewIn {
    approve: bool,
    #[serde(default)]
    note: String,
}

async fn review_want(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<WantReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.review_want(Viewer { user: user.as_ref() }, id, b.approve, &b.note)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct WantStatusIn {
    status: String,
    resource: Option<Id>,
}

async fn resolve_want(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<WantStatusIn>) -> R<Json<Value>> {
    let status = match b.status.as_str() {
        "open" => WantStatus::Open,
        "found" => WantStatus::Found,
        "closed" => WantStatus::Closed,
        _ => return Err(ApiError(Error::BadRequest("状态不对".into()))),
    };
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.resolve_want(Viewer { user: user.as_ref() }, id, status, b.resource)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct VoteIn {
    on: bool,
}

async fn vote_want(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<VoteIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let votes = blocking(move || hub.vote_want(Viewer { user: user.as_ref() }, id, b.on)).await?;
    Ok(Json(json!({ "votes": votes })))
}

#[derive(Deserialize)]
struct WantReplyIn {
    #[serde(default)]
    body: String,
    resource: Option<Id>,
}

async fn reply_want(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<WantReplyIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.reply_want(Viewer { user: user.as_ref() }, id, &b.body, b.resource)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_want_reply(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_want_reply(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AnnouncementIn {
    text: String,
}

async fn set_announcement(State(app): S, auth: Auth, Json(b): Json<AnnouncementIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let a = blocking(move || hub.set_announcement(Viewer { user: user.as_ref() }, &b.text)).await?;
    Ok(Json(json!(a)))
}

// ------------------------------------------------------------------ 统计, 提醒, 关注, 收藏夹

/// The public 统计 page. Nobody's own data is in it, so shared caches may keep it a while.
async fn site_stats(State(app): S) -> Response {
    let mut res = Json(json!(app.hub.site_stats())).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=300"));
    res
}

#[derive(Deserialize)]
struct NoticesQ {
    limit: Option<usize>,
}

async fn notices(State(app): S, auth: Auth, Query(q): Query<NoticesQ>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.notices(auth.viewer(), q.limit.unwrap_or(50).clamp(1, 200))?)))
}

#[derive(Deserialize)]
struct ReadIn {
    /// One reminder; all of them when absent.
    id: Option<Id>,
}

async fn read_notices(State(app): S, auth: Auth, Json(b): Json<ReadIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.read_notices(Viewer { user: user.as_ref() }, b.id)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn follow_state(State(app): S, auth: Auth, Path(id): Path<Id>) -> Json<Value> {
    Json(json!({ "following": auth.user.as_ref().is_some_and(|u| app.hub.is_following(u.id, id)) }))
}

#[derive(Deserialize)]
struct OnIn {
    on: bool,
}

async fn follow(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<OnIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let on = blocking(move || hub.follow(Viewer { user: user.as_ref() }, id, b.on)).await?;
    Ok(Json(json!({ "following": on })))
}

async fn my_follows(State(app): S, auth: Auth) -> R<Json<Value>> {
    let list = app.hub.following(auth.viewer())?;
    Ok(Json(json!(list.iter().map(|(n, path)| json!({ "node": node_view(&app.hub, n, 0), "path": path.iter().map(node_brief).collect::<Vec<_>>() })).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct CollectionsQ {
    /// "public" (shared lists) or "mine" (default).
    #[serde(default)]
    scope: String,
}

async fn collections(State(app): S, auth: Auth, Query(q): Query<CollectionsQ>) -> R<Json<Value>> {
    Ok(Json(if q.scope == "public" { json!(app.hub.public_collections(auth.viewer(), 100)) } else { json!(app.hub.my_collections(auth.viewer())?) }))
}

#[derive(Deserialize)]
struct CollectionIn {
    title: String,
    #[serde(default)]
    note: String,
}

async fn create_collection(State(app): S, auth: Auth, Json(b): Json<CollectionIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    Ok(Json(json!(blocking(move || hub.create_collection(Viewer { user: user.as_ref() }, &b.title, &b.note)).await?)))
}

async fn collection(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let v = auth.viewer();
    let (c, items) = app.hub.collection(v, id)?;
    let items: Vec<Value> = items
        .iter()
        .map(|(rid, hit)| match hit {
            Some((r, n)) => json!({ "id": rid, "resource": resource_view(&app, r, n, &[], v) }),
            None => json!({ "id": rid, "gone": true }),
        })
        .collect();
    Ok(Json(json!({ "collection": c, "items": items })))
}

async fn update_collection(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<CollectionIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    Ok(Json(json!(blocking(move || hub.update_collection(Viewer { user: user.as_ref() }, id, &b.title, &b.note)).await?)))
}

async fn delete_collection(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_collection(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn collect(State(app): S, auth: Auth, Path((id, resource)): Path<(Id, Id)>, Json(b): Json<OnIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let on = blocking(move || hub.collect(Viewer { user: user.as_ref() }, id, resource, b.on)).await?;
    Ok(Json(json!({ "in": on })))
}

async fn share_collection(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<OnIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    Ok(Json(json!(blocking(move || hub.share_collection(Viewer { user: user.as_ref() }, id, b.on)).await?)))
}

#[derive(Deserialize)]
struct CollectionReviewIn {
    approve: bool,
    #[serde(default)]
    note: String,
}

async fn review_collection(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<CollectionReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.review_collection(Viewer { user: user.as_ref() }, id, b.approve, &b.note)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn pending_collections(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.pending_collections(auth.viewer())?)))
}

async fn collections_with(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.collections_with(auth.viewer(), id)?)))
}

async fn links(State(app): S) -> Json<Value> {
    Json(json!(app.hub.links()))
}

#[derive(Deserialize)]
struct LinkIn {
    title: String,
    url: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    sort: u32,
}

async fn add_link(State(app): S, auth: Auth, Json(b): Json<LinkIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let l = blocking(move || hub.add_link(Viewer { user: user.as_ref() }, &b.title, &b.url, &b.note, b.sort)).await?;
    Ok(Json(json!(l)))
}

async fn update_link(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<LinkIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let l = blocking(move || hub.update_link(Viewer { user: user.as_ref() }, id, &b.title, &b.url, &b.note, b.sort)).await?;
    Ok(Json(json!(l)))
}

#[derive(Deserialize)]
struct SuggestLinkIn {
    title: String,
    url: String,
    #[serde(default)]
    note: String,
}

async fn suggest_link(State(app): S, auth: Auth, Json(b): Json<SuggestLinkIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let s = blocking(move || hub.suggest_link(Viewer { user: user.as_ref() }, &b.title, &b.url, &b.note)).await?;
    Ok(Json(json!(s)))
}

/// Staff: the suggestions waiting for review. Anyone else: their own.
async fn link_suggestions(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.link_suggestions(auth.viewer())?)))
}

#[derive(Deserialize)]
struct LinkReviewIn {
    approve: bool,
    /// Why it's turned down.
    #[serde(default)]
    reason: String,
    /// The entry as the reviewer tidied it (all of title / url given), else as suggested.
    title: Option<String>,
    url: Option<String>,
    #[serde(default)]
    note: String,
    #[serde(default = "default_link_sort")]
    sort: u32,
}

fn default_link_sort() -> u32 {
    100
}

async fn review_link_suggestion(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<LinkReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let l = blocking(move || {
        let edit = match (&b.title, &b.url) {
            (Some(t), Some(u)) => Some((t.as_str(), u.as_str(), b.note.as_str(), b.sort)),
            _ => None,
        };
        hub.review_link_suggestion(Viewer { user: user.as_ref() }, id, b.approve, &b.reason, edit)
    })
    .await?;
    Ok(Json(json!({ "ok": true, "link": l })))
}

async fn delete_link(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_link(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct BatchQ {
    /// Only files under this section / college / group.
    within: Option<Id>,
}

fn batch_view(app: &App, v: Viewer, items: &[(Resource, Node)], within: Option<Id>) -> Value {
    json!({
        "items": items.iter().map(|(r, n)| resource_view(app, r, n, &[], v)).collect::<Vec<_>>(),
        "pool": app.hub.review_pool_size(within),
        "size": xmuhub_core::hub::BATCH_SIZE,
    })
}

/// The reviewer's current batch; also the page's heartbeat (a batch left alone for a few
/// minutes goes back to the pool).
async fn review_batch(State(app): S, auth: Auth, Query(q): Query<BatchQ>) -> R<Json<Value>> {
    let items = app.hub.review_batch(auth.viewer(), q.within, false)?;
    Ok(Json(batch_view(&app, auth.viewer(), &items, q.within)))
}

/// 「领取一批」: a fresh batch once the current one is done.
async fn take_review_batch(State(app): S, auth: Auth, Json(q): Json<BatchQ>) -> R<Json<Value>> {
    let items = app.hub.review_batch(auth.viewer(), q.within, true)?;
    Ok(Json(batch_view(&app, auth.viewer(), &items, q.within)))
}

async fn release_review_batch(State(app): S, auth: Auth) -> R<Json<Value>> {
    app.hub.release_batch(auth.viewer())?;
    Ok(Json(json!({ "ok": true })))
}

async fn skip_in_review_batch(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    app.hub.skip_in_batch(auth.viewer(), id)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TextIn {
    text: String,
}

async fn ask_uploader(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<TextIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let q = blocking(move || hub.ask_uploader(Viewer { user: user.as_ref() }, id, &b.text)).await?;
    Ok(Json(json!(q)))
}

async fn answer_question(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<TextIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let q = blocking(move || hub.answer_question(Viewer { user: user.as_ref() }, id, &b.text)).await?;
    Ok(Json(json!(q)))
}

async fn my_questions(State(app): S, auth: Auth) -> R<Json<Value>> {
    let items = app.hub.my_open_questions(auth.viewer())?;
    Ok(Json(json!(items.iter().map(|(q, title)| json!({ "question": q, "title": title })).collect::<Vec<_>>())))
}

async fn pending_nodes(State(app): S, auth: Auth) -> R<Json<Value>> {
    let v: Vec<Value> = app
        .hub
        .pending_nodes(auth.viewer())?
        .iter()
        .map(|(n, path, count)| json!({ "node": node_view(&app.hub, n, *count), "path": path.iter().map(node_brief).collect::<Vec<_>>() }))
        .collect();
    Ok(Json(json!(v)))
}

#[derive(Deserialize)]
struct ReportsQ {
    #[serde(default)]
    all: bool,
}

fn report_view(app: &App, r: &Report, res: &Option<(Resource, Node)>, v: Viewer) -> Value {
    json!({
        "id": r.id, "reason": r.reason, "contact": r.contact, "created_at": r.created_at, "handled": r.handled,
        "handled_note": r.handled_note,
        "handled_by": r.handled_by.map(|u| app.hub.uploader_name(u)),
        "resource": res.as_ref().map(|(x, n)| resource_view(app, x, n, &[], v)),
    })
}

async fn reports(State(app): S, auth: Auth, Query(q): Query<ReportsQ>) -> R<Json<Value>> {
    let v = auth.viewer();
    Ok(Json(json!(app.hub.reports(v, q.all)?.iter().map(|(r, res)| report_view(&app, r, res, v)).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct HandleIn {
    #[serde(default)]
    note: String,
}

async fn delete_report(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.delete_report(Viewer { user: user.as_ref() }, id)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn handle_report(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<HandleIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let user = auth.user.clone();
    blocking(move || hub.handle_report(Viewer { user: user.as_ref() }, id, &b.note)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn users(State(app): S, auth: Auth, Query(q): Query<Q>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.users(auth.viewer(), q.q.as_deref().unwrap_or(""))?.iter().map(user_view).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct UpdateUser {
    level: Option<u8>,
    banned: Option<bool>,
}

async fn update_user(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<UpdateUser>) -> R<Json<Value>> {
    let level = match b.level {
        Some(l) => Some(Level::from_u8(l).filter(|l| *l > Level::Guest).ok_or_else(|| bad("级别不合法"))?),
        None => None,
    };
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let u = blocking(move || hub.update_user(Viewer { user: user.as_ref() }, id, level, b.banned)).await?;
    Ok(Json(user_view(&u)))
}

async fn status(State(app): S, auth: Auth) -> R<Json<Value>> {
    if !auth.viewer().staff() {
        return Err(ApiError(Error::Forbidden));
    }
    Ok(Json(json!({
        "stats": app.hub.stats(),
        "mirrors": app.mirrors.stats(),
        "rss_bytes": crate::alloc::rss_bytes(),
        "relay_bytes_today": app.relay.as_ref().map(|r| r.used_today()),
        "mail": app.mailer.is_some(),
        "version": env!("CARGO_PKG_VERSION"),
        "statuses": [Status::Pending.as_str(), Status::Restricted.as_str()],
    })))
}

// ------------------------------------------------------------------ GitHub repository import

fn require_admin(auth: &Auth) -> R<()> {
    match &auth.user {
        Some(u) if u.level >= Level::Admin => Ok(()),
        Some(_) => Err(ApiError(Error::Forbidden)),
        None => Err(ApiError(Error::Unauthorized)),
    }
}

#[derive(Deserialize)]
struct ScanIn {
    url: String,
    #[serde(default)]
    depth: Option<usize>,
}

/// Lists a public repository (API calls only), groups its documents by folder and suggests
/// a category for each group.
async fn gh_scan(State(app): S, auth: Auth, Json(b): Json<ScanIn>) -> R<Json<Value>> {
    use xmuhub_core::hub::{group_of, is_doc};
    require_admin(&auth)?;
    let gh = app.github.as_ref().ok_or_else(|| bad("GitHub 存储未启用"))?;
    let (owner, repo, branch) = xmuhub_core::storage::github::parse_repo_url(&b.url)?;
    let scan = gh.scan_repo(&owner, &repo, branch.as_deref()).await?;
    let docs: Vec<_> = scan.files.iter().filter(|f| is_doc(&f.path)).collect();
    // Default grouping: two levels when the repo is organised as 学期/课程/…, else one.
    let deep = docs.iter().filter(|f| f.path.matches('/').count() >= 2).count();
    let depth = b.depth.unwrap_or(if deep * 2 > docs.len() { 2 } else { 1 }).clamp(1, 4);
    let mut groups: std::collections::BTreeMap<String, (usize, u64, Vec<String>)> = Default::default();
    for f in &docs {
        let g = groups.entry(group_of(&f.path, depth)).or_default();
        g.0 += 1;
        g.1 += f.size;
        if g.2.len() < 3 {
            g.2.push(f.path.rsplit('/').next().unwrap_or(&f.path).to_string());
        }
    }
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(key, (files, bytes, samples))| {
            // The course is usually named by a folder, not the files: try each folder of the
            // group from the deepest up (学期/课程/期末 → 期末, 课程, 学期), plain and with any
            // bracketed part dropped, and keep the first that finds something.
            let mut suggest = Vec::new();
            for seg in key.rsplit('/') {
                let simplified: String = seg.replace(['（', '('], " ").replace(['）', ')'], " ").split_whitespace().next().unwrap_or("").to_string();
                for q in [seg.to_string(), simplified] {
                    if suggest.is_empty() && !q.is_empty() {
                        suggest = app.hub.suggest_nodes(&q, 5);
                    }
                }
                if !suggest.is_empty() {
                    break;
                }
            }
            json!({
                "key": key, "files": files, "bytes": bytes, "samples": samples,
                "suggest": suggest.iter().map(|(n, p)| json!({ "node": node_brief(n), "path": p.iter().map(node_brief).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let id = format!("{}-{}", scan.commit.get(..8).unwrap_or(""), now());
    let out = json!({
        "scan_id": id, "owner": scan.owner, "repo": scan.repo, "branch": scan.branch, "commit": scan.commit,
        "license": scan.license, "total_files": scan.files.len(), "doc_files": docs.len(),
        "doc_bytes": docs.iter().map(|f| f.size).sum::<u64>(), "depth": depth, "groups": groups,
        "doc_exts": xmuhub_core::hub::DOC_EXTS,
    });
    let mut scans = app.scans.lock();
    if scans.len() > 8 {
        scans.clear();
    }
    scans.insert(id, scan);
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ImportIn {
    scan_id: String,
    depth: usize,
    /// group key → node id
    mappings: std::collections::HashMap<String, Id>,
    /// 专业 for every imported file (repos kept per major, e.g. XMU-SE).
    #[serde(default)]
    major: String,
}

async fn gh_import(State(app): S, auth: Auth, Json(b): Json<ImportIn>) -> R<Json<Value>> {
    require_admin(&auth)?;
    let scan = app.scans.lock().get(&b.scan_id).cloned().ok_or_else(|| bad("扫描结果已过期，请重新扫描"))?;
    let hub = app.hub.clone();
    let user = auth.user.clone();
    let depth = b.depth.clamp(1, 4);
    let report = blocking(move || {
        let map = b.mappings;
        hub.import_repo(Viewer { user: user.as_ref() }, &scan, depth, &b.major, &|g: &str| map.get(g).copied())
    })
    .await?;
    Ok(Json(json!(report)))
}

async fn gh_transfer(State(app): S, auth: Auth) -> R<Json<Value>> {
    if !auth.viewer().staff() {
        return Err(ApiError(Error::Forbidden));
    }
    let (refs, owned) = app.hub.transfer_counts();
    Ok(Json(json!({ "referenced": refs, "owned": owned, "current": crate::transfer::current_job(&app.hub) })))
}

async fn gh_transfer_kick(State(app): S, auth: Auth) -> R<Json<Value>> {
    require_admin(&auth)?;
    let gh = app.github.as_ref().ok_or_else(|| bad("GitHub 存储未启用"))?;
    crate::transfer::kick(&app.hub, gh).await.map_err(|e| ApiError(Error::Upstream(e.to_string())))?;
    Ok(Json(json!({ "current": crate::transfer::current_job(&app.hub) })))
}

async fn thumbs_status(State(app): S, auth: Auth) -> R<Json<Value>> {
    if !auth.viewer().staff() {
        return Err(ApiError(Error::Forbidden));
    }
    let (ok, todo, never) = app.hub.thumb_counts();
    Ok(Json(json!({ "done": ok, "todo": todo, "never": never, "current": crate::thumbs::current_job(&app.hub) })))
}

async fn thumbs_kick(State(app): S, auth: Auth) -> R<Json<Value>> {
    require_admin(&auth)?;
    let gh = app.github.as_ref().ok_or_else(|| bad("GitHub 存储未启用"))?;
    crate::thumbs::kick(&app.hub, gh).await.map_err(|e| ApiError(Error::Upstream(e.to_string())))?;
    Ok(Json(json!({ "current": crate::thumbs::current_job(&app.hub) })))
}

// ------------------------------------------------------------------ local backend (dev)

#[derive(Deserialize)]
struct TicketQ {
    t: String,
}

async fn local_upload(State(app): S, Query(q): Query<TicketQ>, body: axum::body::Body) -> R<Json<Value>> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let local = app.local.as_ref().ok_or(Error::NotFound("接口"))?;
    let t = xmuhub_core::ticket::verify(&local.secret, &q.t)?;
    let path = local.path_of(&t.u)?;
    let tmp = path.with_extension("part");
    let mut f = tokio::fs::File::create(&tmp).await.map_err(Error::from)?;
    let mut n = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::BadRequest(e.to_string()))?;
        n += chunk.len() as u64;
        if n > t.s {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(bad("文件比声明的大"));
        }
        f.write_all(&chunk).await.map_err(Error::from)?;
    }
    f.flush().await.map_err(Error::from)?;
    drop(f);
    if n != t.s {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(bad("文件不完整"));
    }
    tokio::fs::rename(&tmp, &path).await.map_err(Error::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn local_file(State(app): S, Path(name): Path<String>) -> R<Response> {
    let local = app.local.as_ref().ok_or(Error::NotFound("接口"))?;
    let bytes = tokio::fs::read(local.path_of(&name)?).await.map_err(|_| Error::NotFound("文件"))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}
