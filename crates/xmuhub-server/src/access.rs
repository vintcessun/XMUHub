//! Login is required for the site's data: the APIs, MCP and downloads. Pages themselves are
//! the same for everyone and hold no data (Cloudflare caches them, which keeps the site fast
//! and soaks up floods), so they're served to anyone; their script sends a guest to the login
//! page (see `layout` in app.js).

use axum::extract::Request;
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use crate::api::{ApiError, Auth};
use xmuhub_core::Error;

/// Requests that carry the site's data (everything else is a page or a static file).
fn data_request(path: &str) -> bool {
    path.starts_with("/api/") || path == "/mcp" || path.starts_with("/d/")
}

/// Data a guest may still have: the site's settings, who's signed in, and signing in.
fn public_data(method: &Method, path: &str) -> bool {
    let read = matches!(*method, Method::GET | Method::HEAD);
    (read && matches!(path, "/api/me" | "/api/meta"))
        || (*method == Method::POST && matches!(path, "/api/auth/code" | "/api/auth/login" | "/api/auth/register" | "/api/auth/reset" | "/api/auth/logout"))
}

pub async fn guard(auth: Auth, req: Request, next: Next) -> Response {
    respond(auth.user.is_some(), req, next).await
}

async fn respond(signed_in: bool, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !data_request(path) {
        return next.run(req).await;
    }
    let mut res = if signed_in || public_data(req.method(), path) {
        next.run(req).await
    } else if path.starts_with("/d/") {
        let target = req.uri().path_and_query().map_or("/", |v| v.as_str());
        let encoded: String = target.as_bytes().iter().map(|b| format!("%{b:02X}")).collect();
        Redirect::to(&format!("/login?next={encoded}")).into_response()
    } else {
        ApiError::from(Error::Unauthorized).into_response()
    };
    // A shared cache must never serve a signed-in answer to a guest.
    res.headers_mut().insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    res.headers_mut().append(header::VARY, "Cookie, Authorization".parse().unwrap());
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::{Request as HttpRequest, StatusCode}, middleware};
    use tower::ServiceExt;

    async fn request(signed_in: bool, method: Method, path: &str) -> Response {
        Router::new()
            .fallback(|| async { "content" })
            .layer(middleware::from_fn(move |req: Request, next: Next| respond(signed_in, req, next)))
            .oneshot(HttpRequest::builder().method(method).uri(path).body(Body::empty()).unwrap())
            .await.unwrap()
    }

    #[tokio::test]
    async fn guests_get_pages_and_can_sign_in_but_cannot_read_data() {
        for path in ["/", "/login", "/about", "/browse", "/r/1", "/n/1", "/search?q=math", "/api/me", "/api/meta", "/assets/app.js?v=1", "/vendor/library.js"] {
            assert_eq!(request(false, Method::GET, path).await.status(), StatusCode::OK, "{path}");
        }
        for path in ["/api/auth/code", "/api/auth/login", "/api/auth/register", "/api/auth/reset", "/api/auth/logout"] {
            assert_eq!(request(false, Method::POST, path).await.status(), StatusCode::OK, "{path}");
        }
        for path in ["/api/tree", "/api/search?q=math", "/api/recent", "/api/popular", "/api/resources/1", "/api/resources/1/download", "/api/local/file/test", "/api/collections/1", "/api/stats", "/api/bulletins", "/mcp"] {
            for method in [Method::GET, Method::HEAD, Method::POST] {
                let res = request(false, method, path).await;
                assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
                assert_eq!(res.headers()[header::CACHE_CONTROL], "private, no-store");
            }
        }
        assert_eq!(request(false, Method::PATCH, "/api/me").await.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_guest_download_goes_to_the_login_page_and_back() {
        let res = request(false, Method::GET, "/d/1?part=2").await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers()[header::LOCATION], "/login?next=%2F%64%2F%31%3F%70%61%72%74%3D%32");
    }

    #[tokio::test]
    async fn signed_in_data_is_never_shared_cached_but_pages_are_left_alone() {
        for path in ["/api/tree", "/api/resources/1/download", "/d/1", "/mcp"] {
            let res = request(true, Method::GET, path).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(res.headers()[header::CACHE_CONTROL], "private, no-store");
        }
        for path in ["/", "/browse", "/r/1", "/assets/app.js"] {
            assert!(!request(false, Method::GET, path).await.headers().contains_key(header::CACHE_CONTROL), "{path}");
        }
    }
}
