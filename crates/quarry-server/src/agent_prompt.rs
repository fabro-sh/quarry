use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Mirrors JS `encodeURIComponent`, which leaves `A-Z a-z 0-9 - _ . ! ~ * ' ( )`
/// unescaped, so prompt URLs match the ones the browser UI builds.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub(crate) enum AgentPromptScope<'a> {
    Tmp {
        secret: &'a str,
    },
    #[cfg_attr(
        not(feature = "lib-documents"),
        expect(
            dead_code,
            reason = "library prompts require the lib-documents feature"
        )
    )]
    Library {
        library: &'a str,
        path: &'a str,
        token: &'a str,
    },
}

/// Connect instructions handed to an AI agent joining a document. This is the
/// single source of truth for the prompt; the browser UI and the CLI both
/// fetch it from the `agent-prompt` endpoints.
pub(crate) fn agent_prompt(origin: &str, scope: &AgentPromptScope<'_>) -> String {
    let origin = origin.trim_end_matches('/');
    let api_base = format!("{origin}/v1");
    let (locator_url, document_api, pending_api, scope_line, document_path, auth_notice) =
        match scope {
            AgentPromptScope::Tmp { secret } => {
                let secret_segment = encode_component(secret);
                (
                    format!("{origin}/tmp/{secret_segment}"),
                    format!("{api_base}/tmp/documents/{secret_segment}"),
                    None,
                    "Scope: tmp document".to_string(),
                    (*secret).to_string(),
                    "Tmp document URLs are bearer capabilities. Anyone with this URL can access this tmp document; do not treat the secret as an agent identity.",
                )
            }
            AgentPromptScope::Library {
                library,
                path,
                token,
            } => {
                let library_segment = encode_component(library);
                let path_segments = encode_path_segments(path);
                (
                    format!(
                        "{origin}/lib/{library_segment}/documents/{path_segments}?token={token}",
                        token = encode_component(token)
                    ),
                    format!("{api_base}/libraries/{library_segment}/documents/{path_segments}"),
                    Some(format!(
                        "{api_base}/libraries/{library_segment}/events/pending?after=<last-seen-id>"
                    )),
                    format!("Library: {library}"),
                    (*path).to_string(),
                    "Quarry local REST APIs are trusted-localhost for now. The token in the URL identifies the shared document for browser/collab join, but REST agent endpoints on this host do not currently enforce bearer-token auth.",
                )
            }
        };
    let fallback_monitoring = match &pending_api {
        None => format!(
            "   If you cannot keep a stream open, periodically re-read GET {document_api}/blocks and GET {document_api}/review. Document API calls carrying X-Agent-Id refresh your presence automatically (it expires 60 seconds after your last one)."
        ),
        Some(pending_api) => format!(
            "   If you cannot keep a stream open, poll GET {pending_api} for activity. After activity, re-read GET {document_api}/blocks and GET {document_api}/review. Document API calls carrying X-Agent-Id refresh your presence automatically (it expires 60 seconds after your last one); if you go quiet for close to a minute, make any document call or re-POST {document_api}/presence."
        ),
    };
    let transaction_operations = crate::gateway::PUBLIC_TRANSACTION_OPERATIONS.join(", ");

    format!(
        r#"Quarry is a local-first collaborative Markdown editor with presence, comments, suggestions, and block edit APIs.

Join this Quarry document using this locator URL:
{locator_url}

{auth_notice}

API base: {api_base}
{scope_line}
Document path: {document_path}

1. Identify yourself on every request.
   Choose an agent id like ai:codex:<short-id> or ai:claude:<short-id> and send it as the X-Agent-Id header on every API call. Any document call carrying it registers and refreshes your presence automatically; presence expires 60 seconds after your last call.
   Announce your status and display name:
   POST {document_api}/presence
   Headers:
   - Content-Type: application/json
   - X-Agent-Id: <agent-id>
   Body:
   {{"status":"reading","by":"<agent name>"}}

2. Read the current document.
   Prefer GET {document_api}/blocks (stable block_ids plus the current document_clock)
   Fallback GET {document_api}

3. After reading, reply to the user with exactly this shape:
   Connected in Quarry and ready.
   <one-sentence summary of the document>
   I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?

4. Read the skill document BEFORE your first edit, comment, or suggestion.
   Skill: {origin}/quarry.SKILL.md
   It defines the transaction op shapes, the block-type vocabulary (there is no list type — a list item is a "p" block with list attrs), and the mark-run shape. Do not guess these.
   Docs: {origin}/agent-docs
   Discovery: {origin}/.well-known/agent.json

5. While working, monitor document activity.
   Prefer GET {document_api}/events/stream with header X-Agent-Id: <agent-id> — the open stream also keeps your presence fresh.
{fallback_monitoring}
   When an event arrives, re-read both GET {document_api}/blocks and GET {document_api}/review before replying or editing. Events are sparse wake signals; the block tree does not contain comment or suggestion bodies.

6. Do not edit until the user gives further instructions.
   For surgical edits and review operations, POST {document_api}/transactions with headers Content-Type: application/json and X-Agent-Id: <agent-id>, and body {{"client_tx_id":"<unique-id>","base_clock":"<document_clock>","actor":{{"kind":"agent","id":"<agent-id>","label":"<agent name>"}},"ops":[...]}}.
   Public ops: {transaction_operations}.
   To author or restructure the whole document, instead PUT {document_api} with a plain Markdown body and headers Content-Type: text/markdown, If-Match: "<document_clock>", X-Agent-Id: <agent-id>, and X-Quarry-Transaction-Actor: <agent name> — concurrent edits diff3-merge rather than being overwritten (details in the skill). A 200 response is not enough: inspect changed and conflicts. If conflicts is non-zero, re-read GET {document_api}/blocks and GET {document_api}/review, incorporate any canonical edits that should survive, and only then re-PUT the reconciled Markdown with the fresh clock. Do not blindly resend the old file.
   To read existing comments, suggestions, and merge conflicts, GET {document_api}/review.
   Every /v1 HTTP failure uses {{code, retryable, message, details?}}. Treat message as human-readable; when details is present, use op_index/op/target/field/value/current_value/allowed_values instead of parsing prose. retryable means the code-specific recovery may succeed, not that the identical request is always safe to replay. For STALE_BASE, BLOCK_MOVE_CONFLICT, or PRECONDITION_FAILED, refresh GET {document_api}/blocks, rebuild with the new document_clock and a NEW client_tx_id, and resubmit once. For SERVICE_BUSY, honor Retry-After and retry the unchanged idempotent request, preserving client_tx_id. Never retry destructive writes without a bounded, code-specific recovery."#
    )
}

fn encode_component(value: &str) -> String {
    utf8_percent_encode(value, URI_COMPONENT).to_string()
}

fn encode_path_segments(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "72cb58585aa73e35758bc1141f79e32e";

    fn library_prompt() -> String {
        agent_prompt(
            "http://127.0.0.1:5173",
            &AgentPromptScope::Library {
                library: "team notes",
                path: "folder/live doc.md",
                token: "invite-token",
            },
        )
    }

    #[test]
    fn tmp_prompt_uses_secret_locator_and_bearer_capability_notice() {
        let prompt = agent_prompt(
            "http://127.0.0.1:5173",
            &AgentPromptScope::Tmp { secret: SECRET },
        );

        assert!(prompt.contains(&format!(
            "Join this Quarry document using this locator URL:\nhttp://127.0.0.1:5173/tmp/{SECRET}"
        )));
        assert!(prompt.contains("Scope: tmp document"));
        assert!(!prompt.contains("Library:"));
        assert!(prompt.contains("Tmp document URLs are bearer capabilities"));
        assert!(prompt.contains("do not treat the secret as an agent identity"));
        assert!(prompt.contains(&format!(
            "POST http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/presence"
        )));
        assert!(prompt.contains(&format!(
            "GET http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/events/stream"
        )));
        assert!(prompt.contains(&format!(
            "GET http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/blocks"
        )));
        assert!(prompt.contains(&format!(
            "POST http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/transactions"
        )));
        assert!(prompt.contains(&format!(
            "GET http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/review"
        )));
        assert!(prompt.contains(&format!(
            "periodically re-read GET http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/blocks and GET http://127.0.0.1:5173/v1/tmp/documents/{SECRET}/review"
        )));
        assert!(prompt.contains(
            "Document API calls carrying X-Agent-Id refresh your presence automatically"
        ));
    }

    #[test]
    fn library_prompt_advertises_all_required_quarry_endpoints() {
        let prompt = library_prompt();

        assert!(prompt.contains("Quarry is a local-first collaborative Markdown editor"));
        assert!(prompt.contains(
            "http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token"
        ));
        assert!(prompt.contains("trusted-localhost"));
        assert!(prompt.contains(
            "REST agent endpoints on this host do not currently enforce bearer-token auth"
        ));
        assert!(prompt.contains("API base: http://127.0.0.1:5173/v1"));
        assert!(prompt.contains("Library: team notes"));
        assert!(prompt.contains("Document path: folder/live doc.md"));
        assert!(prompt.contains("1. Identify yourself on every request."));
        assert!(prompt.contains(
            "POST http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/presence"
        ));
        assert!(!prompt.contains("/snapshot"));
        assert!(
            prompt.contains(
                "Connected in Quarry and ready.\n   <one-sentence summary of the document>"
            )
        );
        assert!(prompt.contains(
            "GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/events/stream"
        ));
        assert!(prompt.contains(
            "GET http://127.0.0.1:5173/v1/libraries/team%20notes/events/pending?after=<last-seen-id>"
        ));
        assert!(prompt.contains(
            "GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/blocks"
        ));
        assert!(prompt.contains(
            "POST http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/transactions"
        ));
        assert!(prompt.contains(
            "GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/review"
        ));
        assert!(prompt.contains("client_tx_id"));
        assert!(prompt.contains("base_clock"));
        assert!(prompt.contains(r#""label":"<agent name>""#));
        assert!(prompt.contains("set_block_type"));
        assert!(prompt.contains("add_mark, remove_mark, set_link"));
        assert!(prompt.contains("suggestion.accept, suggestion.reject"));
        // The whole-document Markdown PUT is advertised as a write path.
        assert!(prompt.contains(
            "PUT http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md with a plain Markdown body"
        ));
        assert!(prompt.contains("If-Match: \"<document_clock>\""));
        assert!(prompt.contains("Content-Type: text/markdown"));
        assert!(prompt.contains("X-Quarry-Transaction-Actor: <agent name>"));
        // A 200 PUT can park hunks in review; the prompt says how to notice.
        assert!(prompt.contains("A 200 response is not enough: inspect changed and conflicts"));
        assert!(prompt.contains("Do not blindly resend the old file"));
        assert!(prompt.contains("{code, retryable, message, details?}"));
        assert!(prompt.contains("use op_index/op/target/field/value/current_value/allowed_values"));
        assert!(prompt.contains("SERVICE_BUSY"));
        assert!(prompt.contains("honor Retry-After"));
        assert!(prompt.contains("a NEW client_tx_id"));
        // The quarantined legacy facades are no longer advertised.
        assert!(!prompt.contains("/edit"));
        assert!(!prompt.contains("/ops"));
        assert!(!prompt.contains("baseToken"));
        assert!(prompt.contains("Skill: http://127.0.0.1:5173/quarry.SKILL.md"));
        assert!(prompt.contains("Docs: http://127.0.0.1:5173/agent-docs"));
        assert!(prompt.contains("Discovery: http://127.0.0.1:5173/.well-known/agent.json"));
        assert!(prompt.contains(
            "re-read both GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/blocks and GET http://127.0.0.1:5173/v1/libraries/team%20notes/documents/folder/live%20doc.md/review"
        ));
    }

    #[test]
    fn skill_reading_is_a_numbered_prerequisite_to_writing() {
        let prompt = library_prompt();

        assert!(prompt.contains(
            "4. Read the skill document BEFORE your first edit, comment, or suggestion."
        ));
        assert!(prompt.contains("Do not guess these."));
        // The vocabulary agents guess wrong without the skill is called out inline.
        assert!(prompt.contains("there is no list type"));
        assert!(!prompt.contains("If you need setup details"));
        // The skill step comes before the monitoring and editing steps.
        let skill_step = prompt
            .find("Read the skill document")
            .expect("prompt should mention the skill document");
        let edit_step = prompt
            .find("Do not edit until the user gives further instructions")
            .expect("prompt should defer edits to the user");
        assert!(skill_step < edit_step);
    }

    #[test]
    fn trailing_origin_slash_is_normalized() {
        let prompt = agent_prompt(
            "http://127.0.0.1:5173/",
            &AgentPromptScope::Tmp { secret: SECRET },
        );
        assert!(prompt.contains(&format!("http://127.0.0.1:5173/tmp/{SECRET}")));
        assert!(!prompt.contains("5173//"));
    }
}
