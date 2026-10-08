//! Login is required for reading pages and APIs; the landing and account-entry pages stay public.

use axum::extract::Request;
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use crate::api::{ApiError, Auth};
use xmuhub_core::Error;

fn public_request(method: &Method, path: &str) -> bool {
    let read = matches!(*method, Method::GET | Method::HEAD);
    if read && matches!(path, "/" | "/index.html" | "/login" | "/login.html" | "/about" | "/about.html" | "/privacy" | "/privacy.html" | "/api/me" | "/api/meta") {
        return true;
    }
    if *method == Method::POST && matches!(path, "/api/auth/code" | "/api/auth/login" | "/api/auth/register" | "/api/auth/reset" | "/api/auth/logout") {
        return true;
    }
    read && (path.starts_with("/assets/") || path.starts_with("/vendor/")
        || matches!(path, "/favicon.ico" | "/apple-touch-icon.png" | "/manifest.webmanifest" | "/robots.txt"))
}

pub async fn guard(auth: Auth, req: Request, next: Next) -> Response {
    respond(auth.user.is_some(), req, next).await
}

async fn respond(signed_in: bool, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let public = public_request(req.method(), path);
    let asset = path.starts_with("/assets/") || path.starts_with("/vendor/")
        || matches!(path, "/favicon.ico" | "/apple-touch-icon.png" | "/manifest.webmanifest" | "/robots.txt");
    let mut res = if signed_in || public {
        next.run(req).await
    } else if path.starts_with("/api/") || path == "/mcp" {
        ApiError::from(Error::Unauthorized).into_response()
    } else {
        let target = req.uri().path_and_query().map_or("/", |v| v.as_str());
        let encoded: String = target.as_bytes().iter().map(|b| format!("%{b:02X}")).collect();
        Redirect::to(&format!("/login?next={encoded}")).into_response()
    };
    // A shared cache must never serve a signed-in page or API response to a guest.
    if !asset {
        res.headers_mut().insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
        res.headers_mut().append(header::VARY, "Cookie, Authorization".parse().unwrap());
    }
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
    async fn guests_can_enter_and_register_but_cannot_read_content() {
        for path in ["/", "/index.html", "/login", "/login.html", "/about", "/privacy", "/api/me", "/api/meta", "/assets/app.js?v=1", "/vendor/library.js"] {
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
    async fn pages_and_download_redirects_preserve_the_return_url() {
        for path in ["/browse", "/browse.html", "/node.html", "/resource.html", "/n/1", "/r/1", "/d/1", "/help", "/stats", "/search?q=math&page=2"] {
            let res = request(false, Method::GET, path).await;
            assert_eq!(res.status(), StatusCode::SEE_OTHER, "{path}");
            let encoded: String = path.as_bytes().iter().map(|b| format!("%{b:02X}")).collect();
            assert_eq!(res.headers()[header::LOCATION], format!("/login?next={encoded}"));
        }
    }

    #[tokio::test]
    async fn signed_in_requests_continue_and_content_is_never_shared_cached() {
        for path in ["/browse", "/r/1", "/d/1", "/api/tree", "/api/resources/1/download", "/mcp"] {
            let res = request(true, Method::GET, path).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(res.headers()[header::CACHE_CONTROL], "private, no-store");
        }
        let res = request(false, Method::GET, "/assets/app.js").await;
        assert!(!res.headers().contains_key(header::CACHE_CONTROL));
    }
}
