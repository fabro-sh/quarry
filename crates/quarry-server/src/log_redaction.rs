use quarry_storage::{TMP_DOCUMENT_SECRET_LEN, is_tmp_document_secret};
use std::borrow::Cow;

pub(crate) const TMP_SECRET_PLACEHOLDER: &str = "<tmp-secret>";

const TMP_BROWSER_PREFIX: &str = "/tmp/";
const TMP_COLLAB_PREFIX: &str = "/v1/tmp/collab/";
const TMP_DOCUMENT_PREFIX: &str = "/v1/tmp/documents/";

pub(crate) fn redact_path(path: &str) -> Cow<'_, str> {
    [TMP_DOCUMENT_PREFIX, TMP_COLLAB_PREFIX, TMP_BROWSER_PREFIX]
        .into_iter()
        .find_map(|prefix| redact_segment_after_prefix(path, prefix))
        .map_or(Cow::Borrowed(path), Cow::Owned)
}

pub(crate) fn redact_tmp_document_identifier(identifier: &str) -> Cow<'_, str> {
    if is_tmp_document_secret(identifier) {
        Cow::Borrowed(TMP_SECRET_PLACEHOLDER)
    } else {
        Cow::Borrowed(identifier)
    }
}

/// Replaces any whitespace-delimited token that is a tmp document secret with
/// [`TMP_SECRET_PLACEHOLDER`]. Storage error `Display` text such as
/// `not found: <secret>` or `gone: <secret>` embeds the raw capability; this
/// strips it before the message reaches a log field or an HTTP error body.
/// Returns the input borrowed and unchanged when it carries no secret.
pub(crate) fn redact_secret_tokens(message: &str) -> Cow<'_, str> {
    if !message.split(' ').any(is_tmp_document_secret) {
        return Cow::Borrowed(message);
    }
    let redacted = message
        .split(' ')
        .map(|token| {
            if is_tmp_document_secret(token) {
                TMP_SECRET_PLACEHOLDER
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Cow::Owned(redacted)
}

fn redact_segment_after_prefix(path: &str, prefix: &str) -> Option<String> {
    let suffix = path.strip_prefix(prefix)?;
    let secret = suffix.get(..TMP_DOCUMENT_SECRET_LEN)?;
    if !is_tmp_document_secret(secret) {
        return None;
    }
    let rest = &suffix[TMP_DOCUMENT_SECRET_LEN..];
    if !rest.is_empty() && !rest.starts_with(['/', '?', '#']) {
        return None;
    }

    let mut redacted =
        String::with_capacity(path.len() + TMP_SECRET_PLACEHOLDER.len() - TMP_DOCUMENT_SECRET_LEN);
    redacted.push_str(prefix);
    redacted.push_str(TMP_SECRET_PLACEHOLDER);
    redacted.push_str(rest);
    Some(redacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdefABCDEF0123456789";

    #[test]
    fn redacts_browser_tmp_routes() {
        assert_eq!(redact_path(&format!("/tmp/{SECRET}")), "/tmp/<tmp-secret>");
        assert_eq!(
            redact_path(&format!("/tmp/{SECRET}/preview")),
            "/tmp/<tmp-secret>/preview"
        );
    }

    #[test]
    fn redacts_rest_tmp_document_routes_with_suffixes() {
        assert_eq!(
            redact_path(&format!("/v1/tmp/documents/{SECRET}")),
            "/v1/tmp/documents/<tmp-secret>"
        );
        assert_eq!(
            redact_path(&format!("/v1/tmp/documents/{SECRET}/presence")),
            "/v1/tmp/documents/<tmp-secret>/presence"
        );
        assert_eq!(
            redact_path(&format!("/v1/tmp/documents/{SECRET}/events/stream")),
            "/v1/tmp/documents/<tmp-secret>/events/stream"
        );
    }

    #[test]
    fn redacts_tmp_collab_routes() {
        assert_eq!(
            redact_path(&format!("/v1/tmp/collab/{SECRET}/content")),
            "/v1/tmp/collab/<tmp-secret>/content"
        );
    }

    #[test]
    fn redacts_query_or_fragment_terminated_tmp_secrets() {
        assert_eq!(
            redact_path(&format!("/v1/tmp/documents/{SECRET}?after=0")),
            "/v1/tmp/documents/<tmp-secret>?after=0"
        );
        assert_eq!(
            redact_path(&format!("/v1/tmp/documents/{SECRET}#events")),
            "/v1/tmp/documents/<tmp-secret>#events"
        );
    }

    #[test]
    fn leaves_invalid_or_non_tmp_paths_unchanged() {
        assert_eq!(
            redact_path("/v1/tmp/documents/not-a-capability-secret/presence"),
            "/v1/tmp/documents/not-a-capability-secret/presence"
        );
        assert_eq!(
            redact_path("/v1/tmp/documents/0123456789abcdefABCDEF0123456789x"),
            "/v1/tmp/documents/0123456789abcdefABCDEF0123456789x"
        );
        assert_eq!(
            redact_path("/v1/documents/0123456789abcdefABCDEF0123456789"),
            "/v1/documents/0123456789abcdefABCDEF0123456789"
        );
    }

    #[test]
    fn leaves_library_document_paths_unchanged() {
        assert_eq!(
            redact_path(&format!("/v1/libraries/lib/documents/{SECRET}")),
            format!("/v1/libraries/lib/documents/{SECRET}")
        );
        assert_eq!(
            redact_path(&format!("/v1/libraries/lib/documents/folder/{SECRET}.md")),
            format!("/v1/libraries/lib/documents/folder/{SECRET}.md")
        );
    }

    #[test]
    fn redacts_bare_identifier_only_when_tmp_scope_is_known() {
        assert_eq!(redact_tmp_document_identifier(SECRET), "<tmp-secret>");
        assert_eq!(
            redact_tmp_document_identifier("scratch/note.txt"),
            "scratch/note.txt"
        );
    }

    #[test]
    fn redacts_secret_tokens_in_storage_error_messages() {
        assert_eq!(
            redact_secret_tokens(&format!("not found: {SECRET}")),
            "not found: <tmp-secret>"
        );
        assert_eq!(
            redact_secret_tokens(&format!("gone: {SECRET}")),
            "gone: <tmp-secret>"
        );
    }

    #[test]
    fn leaves_secret_free_messages_borrowed_and_unchanged() {
        assert!(matches!(
            redact_secret_tokens("unsupported media type: text/plain"),
            Cow::Borrowed("unsupported media type: text/plain")
        ));
    }
}
