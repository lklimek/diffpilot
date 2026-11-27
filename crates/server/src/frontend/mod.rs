//! Serving the web UI (embedded with feature `embed-ui`): files of `web/dist`, and `index.html` for every other
//! browser path (client-side routes). API, auth and runner paths are never answered with the UI.

use axum::Router;
use contracts::common::PrRef;
use contracts::ids::AnalysisId;

use crate::state::AppState;

/// Security headers of every browser-facing response, web UI and API (threat model T7b); each adds its own CSP.
pub(crate) const SECURITY_HEADERS: [(&str, &str); 3] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "same-origin"),
    ("x-frame-options", "DENY"),
];

/// Browser path to a stored pull request.
pub(crate) fn pr_path(pr: &PrRef) -> String {
    format!("/pr/{}/{}/{}", pr.repo.owner, pr.repo.name, pr.number)
}

/// Browser path to the analysis of a stored pull request.
pub(crate) fn analysis_path(pr: &PrRef, id: AnalysisId) -> String {
    format!("{}/analyses/{id}", pr_path(pr))
}

/// Web UI routes.
#[cfg(not(feature = "embed-ui"))]
pub fn routes() -> Router<AppState> {
    Router::new()
}

/// Web UI routes.
#[cfg(feature = "embed-ui")]
pub fn routes() -> Router<AppState> {
    Router::new().fallback(embedded::serve)
}

#[cfg(feature = "embed-ui")]
mod embedded {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
    use axum::http::{HeaderValue, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use rust_embed::Embed;

    use super::SECURITY_HEADERS;

    /// The built web UI (path relative to this crate).
    #[derive(Embed)]
    #[folder = "../../web/dist"]
    struct Assets;

    const INDEX: &str = "index.html";
    /// Prefixes the UI never answers.
    const NOT_UI: [&str; 5] = ["/api/", "/auth/", "/runner/", "/webhooks/", "/healthz"];
    const CSP: (&str, &str) = (
        "content-security-policy",
        "default-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
    );

    /// What a request path resolves to.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum Target {
        /// Not a UI path.
        NotFound,
        /// An embedded file.
        Asset(String),
        /// The SPA entry page.
        Index,
    }

    pub(super) fn target(path: &str, exists: impl Fn(&str) -> bool) -> Target {
        if NOT_UI
            .iter()
            .any(|prefix| path.starts_with(prefix) || path == prefix.trim_end_matches('/'))
        {
            return Target::NotFound;
        }
        let file = path.trim_start_matches('/');
        if !file.is_empty() && exists(file) {
            Target::Asset(file.to_owned())
        } else if file.starts_with("assets/") {
            Target::NotFound
        } else {
            Target::Index
        }
    }

    pub(super) async fn serve(method: Method, uri: Uri) -> Response {
        if method != Method::GET && method != Method::HEAD {
            return StatusCode::NOT_FOUND.into_response();
        }
        let file = match target(uri.path(), |file| Assets::get(file).is_some()) {
            Target::NotFound => return StatusCode::NOT_FOUND.into_response(),
            Target::Asset(file) => file,
            Target::Index => INDEX.to_owned(),
        };
        let Some(content) = Assets::get(&file) else {
            tracing::error!(
                file,
                "the embedded web UI lacks this file; rebuild web/dist before the server"
            );
            return StatusCode::NOT_FOUND.into_response();
        };
        let mime = mime_guess::from_path(&file).first_or_octet_stream();
        // Hashed bundle files never change; the entry page must be revalidated.
        let cache = if file.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let mut response = content.data.into_owned().into_response();
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
            headers.insert(CONTENT_TYPE, value);
        }
        headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
        for (name, value) in SECURITY_HEADERS.into_iter().chain([CSP]) {
            headers.insert(name, HeaderValue::from_static(value));
        }
        response
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn security_headers_present() {
            let response = serve(Method::GET, Uri::from_static("/analyses/1/tour")).await;
            assert_eq!(response.status(), StatusCode::OK);
            let headers = response.headers();
            for (name, value) in SECURITY_HEADERS.into_iter().chain([CSP]) {
                assert_eq!(
                    headers.get(name).and_then(|v| v.to_str().ok()),
                    Some(value),
                    "{name}"
                );
            }
            assert_eq!(
                headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
                Some("text/html")
            );
            let api = serve(Method::GET, Uri::from_static("/api/v1/unknown")).await;
            assert_eq!(api.status(), StatusCode::NOT_FOUND);
        }

        #[test]
        fn browser_paths_get_files_or_the_entry_page_and_api_paths_never() {
            let exists = |file: &str| matches!(file, "assets/app-1a2b.js" | "favicon.svg");
            assert_eq!(
                target("/assets/app-1a2b.js", exists),
                Target::Asset("assets/app-1a2b.js".to_owned())
            );
            assert_eq!(
                target("/favicon.svg", exists),
                Target::Asset("favicon.svg".to_owned())
            );
            for spa in ["/", "/analyses/1/tour", "/settings"] {
                assert_eq!(target(spa, exists), Target::Index, "{spa}");
            }
            for other in [
                "/api/v1/nothing",
                "/auth/github/other",
                "/runner/v1/claim",
                "/webhooks/github",
                "/webhooks",
                "/healthz",
                "/assets/missing.js",
            ] {
                assert_eq!(target(other, exists), Target::NotFound, "{other}");
            }
        }
    }
}
