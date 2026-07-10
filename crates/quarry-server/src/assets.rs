use crate::{ApiError, ApiErrorCode};
use axum::Json;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
struct BrowserAssets;

/// The serving decision for a non-API request, factored out so every branch is
/// unit-testable without touching the embed, the router, or any shared state.
#[derive(Debug, PartialEq, Eq)]
enum BrowserResponse {
    /// Serve the exact requested asset.
    Asset,
    /// Serve `index.html` so the SPA can resolve a client-side route.
    IndexHtml,
    /// The embed is empty; the UI was never built.
    NotBuilt,
    /// Clean 404 (missing asset, source map, or unmatched API path).
    NotFound,
}

pub(crate) fn browser_ui_bundle_embedded() -> bool {
    BrowserAssets::get("index.html").is_some()
}

fn classify_browser_request(
    path: &str,
    accepts_html: bool,
    requested_exists: bool,
    index_exists: bool,
) -> BrowserResponse {
    // Unmatched API paths must 404 rather than fall through to the SPA shell.
    if path.starts_with("/v1/") || path == "/v1" {
        return BrowserResponse::NotFound;
    }
    // Source maps are never served from the embedded bundle.
    if path.ends_with(".map") {
        return BrowserResponse::NotFound;
    }
    if requested_exists {
        return BrowserResponse::Asset;
    }
    // A miss is a deep-link only for real browser navigations; a fetch/curl for a
    // missing asset gets a clean 404 instead of a confusing HTML body.
    if accepts_html {
        if index_exists {
            BrowserResponse::IndexHtml
        } else {
            BrowserResponse::NotBuilt
        }
    } else {
        BrowserResponse::NotFound
    }
}

fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

pub(crate) async fn browser_asset(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    let requested_path = path.trim_start_matches('/');
    let asset_path = if requested_path.is_empty() {
        "index.html"
    } else {
        requested_path
    };
    // Fetch the requested asset once and reuse it on the hot path so a large
    // bundle file is not read twice.
    let requested_asset = BrowserAssets::get(asset_path);

    match classify_browser_request(
        path,
        accepts_html(&headers),
        requested_asset.is_some(),
        browser_ui_bundle_embedded(),
    ) {
        BrowserResponse::Asset => match requested_asset {
            Some(asset) => embedded_asset_response(asset_path, asset),
            None => browser_not_found(),
        },
        BrowserResponse::IndexHtml => match BrowserAssets::get("index.html") {
            Some(asset) => embedded_asset_response("index.html", asset),
            None => browser_ui_not_built(),
        },
        BrowserResponse::NotBuilt => browser_ui_not_built(),
        BrowserResponse::NotFound if path.starts_with("/v1/") || path == "/v1" => {
            ApiError::new(ApiErrorCode::NotFound, "not found").into_response()
        }
        BrowserResponse::NotFound => browser_not_found(),
    }
}

fn embedded_asset_response(asset_path: &str, asset: rust_embed::EmbeddedFile) -> Response {
    let mut response = Response::new(Body::from(asset.data.into_owned()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(
            mime_guess::from_path(asset_path)
                .first_or_octet_stream()
                .essence_str(),
        )
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(browser_cache_control(asset_path)),
    );
    response.headers_mut().insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn browser_not_found() -> Response {
    #[derive(Serialize)]
    struct BrowserErrorResponse {
        error: String,
    }

    (
        StatusCode::NOT_FOUND,
        Json(BrowserErrorResponse {
            error: "not found".to_string(),
        }),
    )
        .into_response()
}

fn browser_ui_not_built() -> Response {
    #[derive(Serialize)]
    struct BrowserErrorResponse {
        error: String,
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(BrowserErrorResponse {
            error: "browser UI not built — run `bun run build` in ui/ or use the Vite dev server on :5173"
                .to_string(),
        }),
    )
        .into_response()
}

fn browser_cache_control(path: &str) -> &'static str {
    if path == "index.html" {
        "no-cache"
    } else if is_hashed_browser_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    }
}

fn is_hashed_browser_asset(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with("assets/")
        && file_name.contains('-')
        && file_name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| !ext.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_asset_cache_policy_distinguishes_index_and_hashed_assets() {
        assert_eq!(browser_cache_control("index.html"), "no-cache");
        assert_eq!(
            browser_cache_control("assets/index-abc123.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(browser_cache_control("favicon.ico"), "public, max-age=300");
    }

    #[test]
    fn unmatched_api_paths_never_fall_through_to_the_spa_shell() {
        assert_eq!(
            classify_browser_request("/v1/bogus", true, false, true),
            BrowserResponse::NotFound
        );
        assert_eq!(
            classify_browser_request("/v1", true, false, true),
            BrowserResponse::NotFound
        );
    }

    #[test]
    fn source_maps_are_never_served() {
        assert_eq!(
            classify_browser_request("/assets/index-abc123.js.map", true, true, true),
            BrowserResponse::NotFound
        );
    }

    #[test]
    fn existing_asset_is_served_directly() {
        assert_eq!(
            classify_browser_request("/assets/index-abc123.js", false, true, true),
            BrowserResponse::Asset
        );
    }

    #[test]
    fn browser_navigation_miss_serves_the_spa_shell() {
        assert_eq!(
            classify_browser_request("/lib/notes", true, false, true),
            BrowserResponse::IndexHtml
        );
    }

    #[test]
    fn browser_asset_responses_disable_referrers() {
        let response = embedded_asset_response(
            "assets/index-abc123.js",
            test_embedded_asset(b"console.log('hello')"),
        );
        assert_eq!(
            response.headers()[HeaderName::from_static("referrer-policy")],
            "no-referrer"
        );

        let response = embedded_asset_response("index.html", test_embedded_asset(b"<html></html>"));
        assert_eq!(
            response.headers()[HeaderName::from_static("referrer-policy")],
            "no-referrer"
        );
    }

    fn test_embedded_asset(data: &'static [u8]) -> rust_embed::EmbeddedFile {
        rust_embed::EmbeddedFile {
            data: std::borrow::Cow::Borrowed(data),
            metadata: rust_embed::Metadata::__rust_embed_new([0; 32], None, None),
        }
    }

    #[test]
    fn navigation_miss_without_a_bundle_reports_ui_not_built() {
        assert_eq!(
            classify_browser_request("/lib/notes", true, false, false),
            BrowserResponse::NotBuilt
        );
    }

    #[test]
    fn non_navigation_miss_is_a_clean_not_found() {
        assert_eq!(
            classify_browser_request("/assets/missing.js", false, false, true),
            BrowserResponse::NotFound
        );
    }

    #[test]
    fn accepts_html_detects_browser_navigations() {
        let mut navigation = HeaderMap::new();
        navigation.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml"),
        );
        assert!(accepts_html(&navigation));

        let mut fetch = HeaderMap::new();
        fetch.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!accepts_html(&fetch));

        assert!(!accepts_html(&HeaderMap::new()));
    }
}
