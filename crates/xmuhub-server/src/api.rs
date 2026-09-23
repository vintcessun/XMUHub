//! JSON API under `/api`, the `/d/{id}` download redirect and HTML page routes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use xmuhub_core::hub::{CoursePatch, NewCourse, PartSpec, ResourceInput, SearchItem, Viewer};
use xmuhub_core::model::{Course, CourseStatus, Id, Kind, Level, Resource, Status, Token, now};
use xmuhub_core::search::{DocType, Filter};
use xmuhub_core::storage::Receipt;
use xmuhub_core::storage::local::LocalBackend;
use xmuhub_core::storage::mirrors::Mirrors;
use xmuhub_core::{Error, Hub};

use crate::web::Site;

pub struct App {
    pub hub: Arc<Hub>,
    pub site: Arc<Site>,
    pub mirrors: Arc<Mirrors>,
    pub local: Option<Arc<LocalBackend>>,
    pub worker_url: String,
    pub relay: Option<crate::relay::Relay>,
    /// ip → (day, claims) for self-service token claims.
    pub claims: Mutex<HashMap<String, (i64, u32)>>,
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

/// Runs a (possibly fsync-ing) write off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> xmuhub_core::Result<T> + Send + 'static) -> R<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(Error::Internal(e.to_string())))?
        .map_err(ApiError)
}

// ------------------------------------------------------------------ auth

/// The caller's token, if any. A malformed or revoked token is treated as a guest.
pub struct Auth(pub Option<Token>);

impl Auth {
    pub fn viewer(&self) -> Viewer<'_> {
        Viewer { token: self.0.as_ref() }
    }
}

impl FromRequestParts<Arc<App>> for Auth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(|secret| app.hub.authenticate(secret.trim()));
        Ok(Auth(token))
    }
}

/// Client IP as seen by the reverse proxy (we only ever listen on loopback).
fn client_ip(h: &HeaderMap) -> String {
    h.get("x-real-ip")
        .or_else(|| h.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "local".into())
}

// ------------------------------------------------------------------ views

#[derive(Serialize)]
struct CourseView {
    id: Id,
    code: String,
    name: String,
    aliases: Vec<String>,
    college: String,
    status: &'static str,
    count: usize,
}

fn course_view(c: &Course, count: usize) -> CourseView {
    CourseView {
        id: c.id,
        code: c.code.clone(),
        name: c.name.clone(),
        aliases: c.aliases.clone(),
        college: c.college.clone(),
        status: match c.status {
            CourseStatus::Pending => "pending",
            CourseStatus::Active => "active",
            CourseStatus::Merged(_) => "merged",
        },
        count,
    }
}

#[derive(Serialize)]
struct ResourceView {
    id: Id,
    course: Value,
    title: String,
    kind: &'static str,
    kind_label: &'static str,
    year: Option<u16>,
    term: Option<u8>,
    teacher: String,
    description: String,
    filename: String,
    size: u64,
    mime: String,
    status: &'static str,
    needs_review: bool,
    review_note: String,
    created_at: i64,
    updated_at: i64,
    downloads: u64,
    mine: bool,
}

fn resource_view(r: &Resource, c: &Course, me: Option<&Token>) -> ResourceView {
    let mine = me.is_some_and(|t| t.id == r.uploader);
    let insider = mine || me.is_some_and(|t| t.level >= Level::Reviewer);
    ResourceView {
        id: r.id,
        course: json!({ "id": c.id, "name": c.name, "code": c.code, "college": c.college }),
        title: r.title.clone(),
        kind: r.kind.as_str(),
        kind_label: r.kind.label(),
        year: r.year,
        term: r.term,
        teacher: r.teacher.clone(),
        description: r.description.clone(),
        filename: r.filename.clone(),
        size: r.size,
        mime: r.mime.clone(),
        status: r.status.as_str(),
        // "Awaiting re-review" is internal; guests just see a published resource.
        needs_review: r.needs_review && insider,
        review_note: r.review_note.clone(),
        created_at: r.created_at,
        updated_at: r.updated_at,
        downloads: r.downloads,
        mine,
    }
}

fn token_view(t: &Token) -> Value {
    json!({
        "id": t.id, "level": t.level as u8, "label": t.label, "banned": t.banned,
        "created_at": t.created_at, "created_ip": t.created_ip, "uploads": t.uploads,
    })
}

// ------------------------------------------------------------------ routes

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/meta", get(meta))
        .route("/me", get(me))
        .route("/token/claim", post(claim_token))
        .route("/colleges", get(colleges))
        .route("/courses", get(courses))
        .route("/courses/suggest", get(suggest))
        .route("/courses/{id}", get(course).patch(patch_course))
        .route("/courses/{id}/merge", post(merge_course))
        .route("/search", get(search))
        .route("/recent", get(recent))
        .route("/popular", get(popular))
        .route("/mine", get(mine))
        .route("/resources", post(create_resource))
        .route("/resources/{id}", get(resource).patch(patch_resource))
        .route("/resources/{id}/review", post(review))
        .route("/resources/{id}/download", get(download_plan))
        .route("/uploads", post(begin_upload))
        .route("/uploads/{id}", get(upload_plan))
        .route("/uploads/{id}/parts/{index}", post(confirm_part))
        .route("/uploads/{id}/parts/{index}/renew", post(renew_part))
        .route("/review", get(review_queue))
        .route("/admin/tokens", get(list_tokens).post(create_token))
        .route("/admin/tokens/{id}", axum::routing::patch(update_token))
        .route("/admin/status", get(status))
        .route("/relay/upload", post(crate::relay::upload).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/local/upload", put(local_upload).layer(axum::extract::DefaultBodyLimit::disable()))
        .route("/local/file/{name}", get(local_file))
        .fallback(|| async { ApiError(Error::NotFound("接口")) });

    Router::new()
        .nest("/api", api)
        .route("/d/{id}", get(download_redirect))
        .route("/", get(page_home))
        .route("/c/{id}", get(page_course))
        .route("/r/{id}", get(page_resource))
        .route("/{*path}", get(static_file))
        .with_state(app)
}

// ------------------------------------------------------------------ pages

async fn page_home(State(app): S, h: HeaderMap) -> Response {
    app.site.respond("index.html", &h, StatusCode::OK)
}
async fn page_course(State(app): S, h: HeaderMap) -> Response {
    app.site.respond("course.html", &h, StatusCode::OK)
}
async fn page_resource(State(app): S, h: HeaderMap) -> Response {
    app.site.respond("resource.html", &h, StatusCode::OK)
}

async fn static_file(State(app): S, Path(path): Path<String>, h: HeaderMap) -> Response {
    // Clean URLs: /search → search.html.
    let path = if app.site.get(&path).is_none() && app.site.get(&format!("{path}.html")).is_some() {
        format!("{path}.html")
    } else {
        path
    };
    app.site.respond(&path, &h, StatusCode::OK)
}

// ------------------------------------------------------------------ public reads

async fn meta(State(app): S) -> Json<Value> {
    let l = &app.hub.limits;
    Json(json!({
        "kinds": Kind::ALL.iter().map(|k| json!({"key": k.as_str(), "label": k.label()})).collect::<Vec<_>>(),
        "terms": [{"key":1,"label":"秋季学期"},{"key":2,"label":"春季学期"},{"key":3,"label":"夏季学期"}],
        "levels": ["访客","贡献者","可信贡献者","审核员","管理员"],
        "limits": { "max_file": l.max_file, "max_part": l.max_part },
        "stats": app.hub.stats(),
        "upload_via": if app.worker_url.is_empty() { "relay" } else { "worker" },
    }))
}

async fn me(auth: Auth) -> Json<Value> {
    Json(match &auth.0 {
        Some(t) => json!({ "id": t.id, "level": t.level as u8, "label": t.label, "uploads": t.uploads }),
        None => json!({ "level": 0 }),
    })
}

async fn claim_token(State(app): S, h: HeaderMap, auth: Auth) -> R<Json<Value>> {
    if auth.0.is_some() {
        return Err(Error::Conflict("你已经有令牌了".into()).into());
    }
    let ip = client_ip(&h);
    {
        let day = now() / 86400;
        let mut claims = app.claims.lock();
        let e = claims.entry(ip.clone()).or_insert((day, 0));
        if e.0 != day {
            *e = (day, 0);
        }
        if e.1 >= 3 {
            return Err(Error::TooMany("同一网络今天领取的令牌太多了".into()).into());
        }
        e.1 += 1;
    }
    let hub = app.hub.clone();
    let (secret, t) = blocking(move || hub.issue_token(Level::Contributor, "自助领取", &ip)).await?;
    Ok(Json(json!({ "token": secret, "level": t.level as u8, "id": t.id })))
}

async fn colleges(State(app): S) -> Json<Value> {
    Json(json!(app.hub.colleges().into_iter().map(|(n, c)| json!({"name": n, "count": c})).collect::<Vec<_>>()))
}

#[derive(Deserialize)]
struct CoursesQ {
    college: Option<String>,
}

async fn courses(State(app): S, Query(q): Query<CoursesQ>) -> Json<Value> {
    let v: Vec<CourseView> = app.hub.courses(q.college.as_deref()).iter().map(|(c, n)| course_view(c, *n)).collect();
    Json(json!(v))
}

#[derive(Deserialize)]
struct Q {
    q: Option<String>,
}

async fn suggest(State(app): S, Query(q): Query<Q>) -> Json<Value> {
    let v: Vec<CourseView> = app.hub.suggest_courses(q.q.as_deref().unwrap_or(""), 10).iter().map(|c| course_view(c, 0)).collect();
    Json(json!(v))
}

async fn course(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let (c, n) = app.hub.course(auth.viewer(), id)?;
    let rs: Vec<ResourceView> = app.hub.course_resources(auth.viewer(), c.id).iter().map(|r| resource_view(r, &c, auth.0.as_ref())).collect();
    Ok(Json(json!({ "course": course_view(&c, n), "resources": rs })))
}

#[derive(Deserialize)]
struct SearchQ {
    q: Option<String>,
    #[serde(rename = "type")]
    ty: Option<String>,
    kind: Option<String>,
    course: Option<Id>,
    page: Option<usize>,
}

async fn search(State(app): S, auth: Auth, Query(q): Query<SearchQ>) -> R<Json<Value>> {
    const PAGE: usize = 20;
    let filter = Filter {
        ty: match q.ty.as_deref() {
            Some("course") => Some(DocType::Course),
            Some("resource") => Some(DocType::Resource),
            _ => None,
        },
        course: q.course,
        kind: q.kind.as_deref().and_then(Kind::parse).map(|k| k as u64),
    };
    let page = q.page.unwrap_or(1).clamp(1, 50);
    let (items, total) = app.hub.search(auth.viewer(), q.q.as_deref().unwrap_or(""), filter, PAGE, (page - 1) * PAGE)?;
    let items: Vec<Value> = items
        .iter()
        .map(|i| match i {
            SearchItem::Course { course, count } => json!({ "type": "course", "course": course_view(course, *count) }),
            SearchItem::Resource { resource, course } => {
                json!({ "type": "resource", "resource": resource_view(resource, course, auth.0.as_ref()) })
            }
        })
        .collect();
    Ok(Json(json!({ "items": items, "total": total, "page": page, "page_size": PAGE })))
}

fn list(v: Vec<(Resource, Course)>, me: Option<&Token>) -> Json<Value> {
    Json(json!(v.iter().map(|(r, c)| resource_view(r, c, me)).collect::<Vec<_>>()))
}

async fn recent(State(app): S) -> Json<Value> {
    list(app.hub.recent(12), None)
}

async fn popular(State(app): S) -> Json<Value> {
    list(app.hub.popular(12), None)
}

async fn mine(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(list(app.hub.my_resources(auth.viewer())?, auth.0.as_ref()))
}

async fn resource(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    let (r, c) = app.hub.resource(auth.viewer(), id)?;
    Ok(Json(json!(resource_view(&r, &c, auth.0.as_ref()))))
}

async fn download_plan(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.download(auth.viewer(), id)?)))
}

/// Plain link for sharing / wget: 302 to the best mirror of a single-part file.
async fn download_redirect(State(app): S, auth: Auth, Path(id): Path<Id>) -> R<Response> {
    let plan = app.hub.download(auth.viewer(), id)?;
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

async fn confirm_part(State(app): S, auth: Auth, Path((id, index)): Path<(Id, usize)>, Json(r): Json<ReceiptIn>) -> R<Json<Value>> {
    let finished = app.hub.confirm_part(auth.viewer(), id, index, Receipt { asset_id: r.asset_id }).await?;
    Ok(Json(json!({ "finished": finished })))
}

async fn renew_part(State(app): S, auth: Auth, Path((id, index)): Path<(Id, usize)>) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.renew_part(auth.viewer(), id, index).await?)))
}

#[derive(Deserialize)]
struct ReceiptIn {
    asset_id: Option<u64>,
}

#[derive(Deserialize)]
struct CreateResource {
    upload_id: Id,
    course_id: Option<Id>,
    new_course: Option<NewCourse>,
    #[serde(flatten)]
    input: ResourceInput,
}

async fn create_resource(State(app): S, auth: Auth, Json(b): Json<CreateResource>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    let r = blocking(move || hub.create_resource(Viewer { token: tok.as_ref() }, b.upload_id, b.course_id, b.new_course, b.input)).await?;
    let (r, c) = app.hub.resource(auth.viewer(), r.id)?;
    Ok(Json(json!(resource_view(&r, &c, auth.0.as_ref()))))
}

#[derive(Deserialize)]
struct PatchResource {
    course_id: Option<Id>,
    #[serde(flatten)]
    input: ResourceInput,
}

async fn patch_resource(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<PatchResource>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    blocking(move || hub.update_resource(Viewer { token: tok.as_ref() }, id, b.input, b.course_id)).await?;
    let (r, c) = app.hub.resource(auth.viewer(), id)?;
    Ok(Json(json!(resource_view(&r, &c, auth.0.as_ref()))))
}

// ------------------------------------------------------------------ review

#[derive(Deserialize)]
struct ReviewIn {
    action: String,
    #[serde(default)]
    note: String,
}

async fn review(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<ReviewIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    let (r, garbage) = blocking(move || hub.review(Viewer { token: tok.as_ref() }, id, &b.action, &b.note)).await?;
    delete_later(&app, garbage);
    Ok(Json(json!({ "status": r.status.as_str() })))
}

/// Deletes storage replicas in the background; failures are logged, not surfaced.
pub fn delete_later(app: &Arc<App>, locs: Vec<xmuhub_core::model::Location>) {
    if locs.is_empty() {
        return;
    }
    let hub = app.hub.clone();
    tokio::spawn(async move {
        for l in locs {
            match hub.storage.for_location(&l) {
                Ok(b) => {
                    if let Err(e) = b.delete(&l).await {
                        tracing::warn!("delete {l:?}: {e}");
                    }
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
    });
}

async fn review_queue(State(app): S, auth: Auth) -> R<Json<Value>> {
    let q = app.hub.review_queue(auth.viewer())?;
    let courses = app.hub.pending_courses(auth.viewer())?;
    Ok(Json(json!({
        "resources": q.iter().map(|(r, c)| resource_view(r, c, auth.0.as_ref())).collect::<Vec<_>>(),
        "courses": courses.iter().map(|(c, n)| course_view(c, *n)).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct PatchCourse {
    #[serde(flatten)]
    patch: CoursePatchIn,
    #[serde(default)]
    approve: bool,
}

#[derive(Deserialize)]
struct CoursePatchIn {
    code: Option<String>,
    name: Option<String>,
    college: Option<String>,
    aliases: Option<Vec<String>>,
}

async fn patch_course(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<PatchCourse>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    let p = CoursePatch { code: b.patch.code, name: b.patch.name, college: b.patch.college, aliases: b.patch.aliases };
    let c = blocking(move || hub.update_course(Viewer { token: tok.as_ref() }, id, p, b.approve)).await?;
    Ok(Json(json!(course_view(&c, 0))))
}

#[derive(Deserialize)]
struct MergeIn {
    into: Id,
}

async fn merge_course(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<MergeIn>) -> R<Json<Value>> {
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    let c = blocking(move || hub.merge_course(Viewer { token: tok.as_ref() }, id, b.into)).await?;
    Ok(Json(json!(course_view(&c, 0))))
}

// ------------------------------------------------------------------ admin

async fn list_tokens(State(app): S, auth: Auth) -> R<Json<Value>> {
    Ok(Json(json!(app.hub.tokens(auth.viewer())?.iter().map(token_view).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct NewToken {
    level: u8,
    #[serde(default)]
    label: String,
}

async fn create_token(State(app): S, auth: Auth, h: HeaderMap, Json(b): Json<NewToken>) -> R<Json<Value>> {
    let me = auth.0.as_ref().ok_or(Error::Unauthorized)?;
    let level = Level::from_u8(b.level).filter(|l| *l > Level::Guest).ok_or_else(|| xmuhub_core::error::bad("级别不合法"))?;
    // Reviewers may hand out tokens below their own tier; admins anything.
    if me.level < Level::Reviewer || (me.level < Level::Admin && level >= Level::Reviewer) {
        return Err(Error::Forbidden.into());
    }
    let hub = app.hub.clone();
    let ip = client_ip(&h);
    let (secret, t) = blocking(move || hub.issue_token(level, &b.label, &ip)).await?;
    Ok(Json(json!({ "token": secret, "info": token_view(&t) })))
}

#[derive(Deserialize)]
struct UpdateToken {
    level: Option<u8>,
    banned: Option<bool>,
    label: Option<String>,
}

async fn update_token(State(app): S, auth: Auth, Path(id): Path<Id>, Json(b): Json<UpdateToken>) -> R<Json<Value>> {
    let level = match b.level {
        Some(l) => Some(Level::from_u8(l).filter(|l| *l > Level::Guest).ok_or_else(|| xmuhub_core::error::bad("级别不合法"))?),
        None => None,
    };
    let hub = app.hub.clone();
    let tok = auth.0.clone();
    let t = blocking(move || hub.update_token(Viewer { token: tok.as_ref() }, id, level, b.banned, b.label)).await?;
    Ok(Json(token_view(&t)))
}

async fn status(State(app): S, auth: Auth) -> R<Json<Value>> {
    match &auth.0 {
        Some(t) if t.level >= Level::Reviewer => {}
        _ => return Err(Error::Forbidden.into()),
    }
    Ok(Json(json!({
        "stats": app.hub.stats(),
        "mirrors": app.mirrors.stats(),
        "rss_bytes": crate::alloc::rss_bytes(),
        "relay_bytes_today": app.relay.as_ref().map(|r| r.used_today()),
        "version": env!("CARGO_PKG_VERSION"),
        "pending_statuses": [Status::Pending.as_str()],
    })))
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
            return Err(xmuhub_core::error::bad("文件比声明的大").into());
        }
        f.write_all(&chunk).await.map_err(Error::from)?;
    }
    f.flush().await.map_err(Error::from)?;
    drop(f);
    if n != t.s {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(xmuhub_core::error::bad("文件不完整").into());
    }
    tokio::fs::rename(&tmp, &path).await.map_err(Error::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn local_file(State(app): S, Path(name): Path<String>) -> R<Response> {
    let local = app.local.as_ref().ok_or(Error::NotFound("接口"))?;
    let bytes = tokio::fs::read(local.path_of(&name)?).await.map_err(|_| Error::NotFound("文件"))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}
