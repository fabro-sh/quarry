use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use crate::discovery::request_origin;

const HOME_HTML: &str = include_str!("../resources/home.html");
const HOME_JS: &str = include_str!("../resources/home.js");
const SETUP_MD: &str = include_str!("../resources/setup.md");
const PROMPT_MD: &str = include_str!("../resources/prompt.md");
const EXAMPLE_MD: &str = include_str!("../resources/example.md");

/// Replaced with the request origin at serve time so the same embedded
/// documents work on quarry.lithos.computer and on a local server alike.
const ORIGIN_PLACEHOLDER: &str = "__QUARRY_ORIGIN__";

pub(crate) async fn home_page(headers: HeaderMap) -> Response {
    origin_rendered_response("text/html; charset=utf-8", HOME_HTML, &headers)
}

pub(crate) async fn home_js() -> Response {
    text_response("text/javascript; charset=utf-8", HOME_JS.to_string())
}

pub(crate) async fn setup_md(headers: HeaderMap) -> Response {
    origin_rendered_response("text/markdown; charset=utf-8", SETUP_MD, &headers)
}

pub(crate) async fn prompt_md(headers: HeaderMap) -> Response {
    origin_rendered_response("text/markdown; charset=utf-8", PROMPT_MD, &headers)
}

pub(crate) async fn example_md() -> Response {
    text_response("text/markdown; charset=utf-8", EXAMPLE_MD.to_string())
}

fn origin_rendered_response(
    content_type: &'static str,
    template: &str,
    headers: &HeaderMap,
) -> Response {
    let body = template.replace(ORIGIN_PLACEHOLDER, &request_origin(headers));
    text_response(content_type, body)
}

fn text_response(content_type: &'static str, body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(
            header::HOST,
            HeaderValue::from_static("quarry.lithos.computer"),
        );
        headers
    }

    #[test]
    fn onboarding_documents_render_the_request_origin() {
        let response = origin_rendered_response(
            "text/markdown; charset=utf-8",
            "read __QUARRY_ORIGIN__/prompt.md",
            &forwarded_headers(),
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
    }

    #[test]
    fn no_embedded_document_leaves_the_placeholder_unreplaced() {
        for template in [HOME_HTML, HOME_JS, SETUP_MD, PROMPT_MD, EXAMPLE_MD] {
            let rendered = template.replace(ORIGIN_PLACEHOLDER, "https://quarry.example");
            assert!(!rendered.contains(ORIGIN_PLACEHOLDER));
        }
        // Only origin-rendered documents may use the placeholder; example.md
        // and home.js are served verbatim, so they must not contain it.
        assert!(!EXAMPLE_MD.contains(ORIGIN_PLACEHOLDER));
        assert!(!HOME_JS.contains(ORIGIN_PLACEHOLDER));
    }
}
