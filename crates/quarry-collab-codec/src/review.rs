//! Review-aware markdown → Slate conversion.
//!
//! The editor/collab codec (`block_markdown_to_slate`) deliberately rejects
//! CriticMarkup. The browser, however, loads a review document through
//! `markdownToReview` (`ui/src/features/review/rfm-codec.ts`), which splits the
//! trailing YAML endmatter and rewrites CriticMarkup markers into Plate review
//! marks via `applyCriticMarkup`. To inject an agent edit into a *live* room
//! whose content already carries those marks, the server must reproduce that
//! exact Slate tree. This module is the Rust port of that read path, pinned to
//! the TS oracle by the `fixtures/slate-yjs-compat` review fixtures.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::markdown::block_markdown_to_slate_raw;
use crate::slate::{Attrs, Node};
use crate::Unsupported;

/// Slate element types whose text is literal code; CriticMarkup inside them is
/// left untouched. Mirrors `CODE_BLOCK_TYPES` in `apply-critic-markup.ts`.
const CODE_BLOCK_TYPES: [&str; 2] = ["code_block", "code_line"];

/// One combined matcher for every CriticMarkup family, ported from the `TOKEN`
/// regex in `apply-critic-markup.ts`. The TS source uses greedy-with-negative-
/// lookahead spans (`(?:(?!==\}).)*`); the `regex` crate has no lookahead, but a
/// lazy dot-all span (`(?s:(.*?))`) stops at the same first closing delimiter, so
/// the two are equivalent. Branch order matches the TS alternation.
fn token() -> &'static Regex {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    TOKEN.get_or_init(|| {
        Regex::new(concat!(
            r"\{==(?s:(.*?))==\}(?:\{>>(?s:(.*?))<<\})?(?:\{#([A-Za-z0-9_-]+)\})?",
            r"|\{~~(?s:(.*?))~>(?s:(.*?))~~\}(?:\{#([A-Za-z0-9_-]+)\})?",
            r"|\{\+\+(?s:(.*?))\+\+\}(?:\{#([A-Za-z0-9_-]+)\})?",
            r"|\{--(?s:(.*?))--\}(?:\{#([A-Za-z0-9_-]+)\})?",
            r"|\{>>(?s:(.*?))<<\}(?:\{#([A-Za-z0-9_-]+)\})?",
        ))
        .expect("review token regex is valid")
    })
}

/// Parsed review endmatter (the trailing `---\ncomments:`/`suggestions:` YAML).
/// Mirrors the TS `ReviewMeta` and the server's at-rest representation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewMeta {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub comments: BTreeMap<String, ReviewMetaEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub suggestions: BTreeMap<String, ReviewMetaEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewMetaEntry {
    #[serde(default = "unknown_review_author")]
    pub by: String,
    #[serde(default)]
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub re: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewMetaPatch {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub comments: BTreeMap<String, ReviewMetaEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub suggestions: BTreeMap<String, ReviewMetaEntry>,
    #[serde(
        default,
        rename = "removeComments",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub remove_comments: Vec<String>,
    #[serde(
        default,
        rename = "removeSuggestions",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub remove_suggestions: Vec<String>,
}

impl ReviewMetaPatch {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
            && self.suggestions.is_empty()
            && self.remove_comments.is_empty()
            && self.remove_suggestions.is_empty()
    }
}

fn unknown_review_author() -> String {
    "unknown".to_string()
}

/// Convert one review-document body block to Slate nodes, rewriting CriticMarkup
/// into review marks (comment / suggestion) exactly as the browser's
/// `applyCriticMarkup` does. `meta` supplies suggestion `by`/`at` for the marks.
///
/// Returns `Unsupported` when the markup can't be reproduced deterministically
/// (e.g. a marker without `{#id}`, which the browser fills with a random
/// `nanoid`). Callers fall back to the non-injecting write path in that case.
pub fn review_block_to_slate(markdown: &str, meta: &ReviewMeta) -> Result<Vec<Node>, Unsupported> {
    let expanded = expand_substitutions(markdown)?;
    let nodes = block_markdown_to_slate_raw(&expanded)?;
    apply_critic_markup(nodes, false, meta)
}

/// Convert a whole review document (body + trailing YAML endmatter) to Slate
/// nodes, mirroring the browser's `markdownToReview`. The endmatter is consumed
/// into the marks (suggestion `by`/`at`) and never becomes nodes.
pub fn review_markdown_to_slate(markdown: &str) -> Result<Vec<Node>, Unsupported> {
    let (body, meta) = split_review_endmatter(markdown);
    review_block_to_slate(&body, &meta)
}

/// Per-block Slate nodes for a review document's blocks (as produced by the
/// server's block splitter, so `blocks.concat()` is the whole document). The
/// trailing endmatter block, when present, yields no nodes because it has no
/// live representation in the Yjs room.
pub fn review_blocks_to_slate(blocks: &[String]) -> Result<Vec<Vec<Node>>, Unsupported> {
    let markdown = blocks.concat();
    let (_, meta) = split_review_endmatter(&markdown);
    let endmatter_ordinal = has_review_endmatter(&markdown)
        .then(|| blocks.len().checked_sub(1))
        .flatten();
    blocks
        .iter()
        .enumerate()
        .map(|(ordinal, block)| {
            if Some(ordinal) == endmatter_ordinal {
                Ok(Vec::new())
            } else {
                review_block_to_slate(block, &meta)
                    .map_err(|error| error.context(format!("block {ordinal}")))
            }
        })
        .collect()
}

/// Split a trailing review endmatter (`---\ncomments:`/`suggestions:` YAML) off
/// the document body. Only the final `\n---\n` block counts, and only when it
/// parses to a mapping with `comments` or `suggestions`. Port of `splitEndmatter`
/// (and the server's `split_review_endmatter`).
pub fn split_review_endmatter(markdown: &str) -> (String, ReviewMeta) {
    let Some((delimiter_start, delimiter_end)) = last_endmatter_delimiter(markdown) else {
        return (markdown.to_string(), ReviewMeta::default());
    };
    let yaml = &markdown[delimiter_end..];
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return (markdown.to_string(), ReviewMeta::default());
    };
    if !is_review_meta_value(&value) {
        return (markdown.to_string(), ReviewMeta::default());
    }
    let meta = serde_yaml::from_value::<ReviewMeta>(value).unwrap_or_default();
    (markdown[..delimiter_start].trim_end().to_string(), meta)
}

pub fn review_meta_with_inline_comment_bodies(markdown: &str) -> (String, ReviewMeta) {
    let (body, mut meta) = split_review_endmatter(markdown);
    hydrate_inline_comment_bodies(&body, &mut meta);
    (body, meta)
}

pub fn hydrate_inline_comment_bodies(body: &str, meta: &mut ReviewMeta) {
    for captures in inline_comment_body().captures_iter(body) {
        let Some(comment_body) = captures.get(2) else {
            continue;
        };
        let Some(id) = captures.get(3) else {
            continue;
        };
        if let Some(entry) = meta.comments.get_mut(id.as_str()) {
            entry.body = Some(comment_body.as_str().to_string());
        }
    }
}

/// Whether the document ends with a review endmatter block.
pub fn has_review_endmatter(markdown: &str) -> bool {
    let Some((_, delimiter_end)) = last_endmatter_delimiter(markdown) else {
        return false;
    };
    serde_yaml::from_str::<serde_yaml::Value>(&markdown[delimiter_end..])
        .is_ok_and(|value| is_review_meta_value(&value))
}

fn last_endmatter_delimiter(markdown: &str) -> Option<(usize, usize)> {
    let bytes = markdown.as_bytes();
    let mut index = 0usize;
    let mut last = None;
    while let Some(offset) = markdown[index..].find("\n---") {
        let start = index + offset;
        let mut cursor = start + 4;
        while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
            cursor += 1;
        }
        let end =
            if cursor + 1 < bytes.len() && bytes[cursor] == b'\r' && bytes[cursor + 1] == b'\n' {
                Some(cursor + 2)
            } else if cursor < bytes.len() && bytes[cursor] == b'\n' {
                Some(cursor + 1)
            } else {
                None
            };
        if let Some(end) = end {
            last = Some((start, end));
        }
        index = start + 1;
    }
    last
}

fn is_review_meta_value(value: &serde_yaml::Value) -> bool {
    let serde_yaml::Value::Mapping(mapping) = value else {
        return false;
    };
    mapping.contains_key(serde_yaml::Value::String("comments".to_string()))
        || mapping.contains_key(serde_yaml::Value::String("suggestions".to_string()))
}

fn inline_comment_body() -> &'static Regex {
    static INLINE_COMMENT_BODY: OnceLock<Regex> = OnceLock::new();
    INLINE_COMMENT_BODY.get_or_init(|| {
        Regex::new(r"\{==(?s:(.*?))==\}\{>>(?s:(.*?))<<\}\{#([A-Za-z0-9_-]+)\}")
            .expect("inline comment body regex is valid")
    })
}

/// Rewrite `{~~old~>new~~}{#id}` into the id-paired delete+insert form
/// `{--old--}{#id}{++new++}{#id}` outside code regions, before markdown parsing
/// (so `~~` is never read as GFM strikethrough). Port of `expandSubstitutions`.
fn expand_substitutions(markdown: &str) -> Result<String, Unsupported> {
    let mut out = String::with_capacity(markdown.len());
    let mut last = 0;
    for region in code_region().find_iter(markdown) {
        out.push_str(&expand_substitutions_segment(
            &markdown[last..region.start()],
        )?);
        out.push_str(region.as_str());
        last = region.end();
    }
    out.push_str(&expand_substitutions_segment(&markdown[last..])?);
    Ok(out)
}

fn expand_substitutions_segment(segment: &str) -> Result<String, Unsupported> {
    let mut out = String::with_capacity(segment.len());
    let mut last = 0;
    for caps in substitution().captures_iter(segment) {
        let whole = caps.get(0).expect("group 0 always present");
        out.push_str(&segment[last..whole.start()]);
        last = whole.end();
        let old = caps.get(1).map_or("", |m| m.as_str());
        let new = caps.get(2).map_or("", |m| m.as_str());
        let id = require_id(caps.get(3))?;
        out.push_str(&format!("{{--{old}--}}{{#{id}}}{{++{new}++}}{{#{id}}}"));
    }
    out.push_str(&segment[last..]);
    Ok(out)
}

/// Fenced (```) and inline (`) code spans, left untouched by substitution
/// expansion. Mirrors the split regex in `collapse-substitutions.ts`.
fn code_region() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)```.*?```|`[^`\n]*`").expect("code region regex is valid"))
}

fn substitution() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"\{~~(?s:(.*?))~>(?s:(.*?))~~\}(?:\{#([A-Za-z0-9_-]+)\})?")
            .expect("substitution regex is valid")
    })
}

/// Walk the tree splitting non-code text leaves into review-marked leaves.
/// Port of `walkChildren` in `apply-critic-markup.ts`.
fn apply_critic_markup(
    nodes: Vec<Node>,
    in_code: bool,
    meta: &ReviewMeta,
) -> Result<Vec<Node>, Unsupported> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            Node::Element {
                ty,
                attrs,
                children,
            } => {
                let next_in_code = in_code || CODE_BLOCK_TYPES.contains(&ty.as_str());
                out.push(Node::Element {
                    children: apply_critic_markup(children, next_in_code, meta)?,
                    ty,
                    attrs,
                });
            }
            Node::Text { text, marks } if !in_code && marks.get("code") != Some(&json!(true)) => {
                out.extend(expand_leaf(&text, &marks, meta)?);
            }
            text => out.push(text),
        }
    }
    Ok(out)
}

/// Split one text leaf around CriticMarkup tokens, carrying the leaf's other
/// marks (`rest`) onto each produced segment. Port of `expandLeaf`.
fn expand_leaf(text: &str, rest: &Attrs, meta: &ReviewMeta) -> Result<Vec<Node>, Unsupported> {
    let mut out = Vec::new();
    let mut last = 0usize;
    let mut matched = false;

    for caps in token().captures_iter(text) {
        matched = true;
        let whole = caps.get(0).expect("group 0 always present");
        push_plain(&mut out, &text[last..whole.start()], rest);
        last = whole.end();

        if let Some(hl) = caps.get(1) {
            let cbody = caps.get(2);
            let cid = caps.get(3);
            if cbody.is_some() || cid.is_some() {
                let id = require_id(cid)?;
                out.push(comment_leaf(rest, id, hl.as_str()));
            } else {
                out.push(plain_leaf(rest, hl.as_str()));
            }
        } else if let (Some(old), Some(new)) = (caps.get(4), caps.get(5)) {
            let id = require_id(caps.get(6))?;
            let entry = suggestion_entry(meta, id)?;
            out.push(suggestion_leaf(rest, id, "remove", entry, old.as_str()));
            out.push(suggestion_leaf(rest, id, "insert", entry, new.as_str()));
        } else if let Some(ins) = caps.get(7) {
            let id = require_id(caps.get(8))?;
            let entry = suggestion_entry(meta, id)?;
            out.push(suggestion_leaf(rest, id, "insert", entry, ins.as_str()));
        } else if let Some(del) = caps.get(9) {
            let id = require_id(caps.get(10))?;
            let entry = suggestion_entry(meta, id)?;
            out.push(suggestion_leaf(rest, id, "remove", entry, del.as_str()));
        } else if caps.get(11).is_some() {
            let id = require_id(caps.get(12))?;
            out.push(comment_leaf(rest, id, " "));
        }
    }

    if !matched {
        return Ok(vec![Node::text(text, rest.clone())]);
    }
    push_plain(&mut out, &text[last..], rest);
    Ok(out)
}

fn require_id(group: Option<regex::Match<'_>>) -> Result<&str, Unsupported> {
    group
        .map(|m| m.as_str())
        .ok_or_else(|| Unsupported::new("review marker without {#id}"))
}

fn push_plain(out: &mut Vec<Node>, slice: &str, rest: &Attrs) {
    if !slice.is_empty() {
        out.push(Node::text(slice, rest.clone()));
    }
}

fn comment_leaf(rest: &Attrs, id: &str, text: &str) -> Node {
    let mut marks = rest.clone();
    marks.insert("comment".to_string(), json!(true));
    marks.insert(format!("comment_{id}"), json!(true));
    Node::text(text, marks)
}

fn plain_leaf(rest: &Attrs, text: &str) -> Node {
    Node::text(text, rest.clone())
}

fn suggestion_entry<'a>(
    meta: &'a ReviewMeta,
    id: &str,
) -> Result<&'a ReviewMetaEntry, Unsupported> {
    // A marker with no endmatter entry would make the browser synthesize a
    // non-deterministic `new Date()`; bail so we fall back instead of guessing.
    meta.suggestions
        .get(id)
        .ok_or_else(|| Unsupported::new("suggestion marker without endmatter entry"))
}

fn suggestion_leaf(rest: &Attrs, id: &str, ty: &str, entry: &ReviewMetaEntry, text: &str) -> Node {
    let mut marks = rest.clone();
    marks.insert("suggestion".to_string(), json!(true));
    marks.insert(
        format!("suggestion_{id}"),
        json!({
            "id": id,
            "type": ty,
            "userId": entry.by,
            "createdAt": created_at_ms(&entry.at),
        }),
    );
    Node::text(text, marks)
}

/// `Date.parse(at)` in milliseconds, `NaN → 0`. Port of `createdAtFromEntry`.
/// `now_timestamp()` (and the TS writer) emit RFC3339 with millis + `Z`.
fn created_at_ms(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AT: &str = "2026-06-05T02:41:00.480Z";
    const AT_MS: i64 = 1_780_627_260_480; // Date.parse(AT)

    fn review(markdown: &str) -> serde_json::Value {
        review_with(markdown, &ReviewMeta::default())
    }

    fn review_with(markdown: &str, meta: &ReviewMeta) -> serde_json::Value {
        serde_json::to_value(review_block_to_slate(markdown, meta).expect("review block converts"))
            .unwrap()
    }

    fn suggestion_meta(id: &str, by: &str, at: &str) -> ReviewMeta {
        let mut meta = ReviewMeta::default();
        meta.suggestions.insert(
            id.to_string(),
            ReviewMetaEntry {
                by: by.to_string(),
                at: at.to_string(),
                body: None,
                re: None,
                status: None,
                resolved: None,
            },
        );
        meta
    }

    #[test]
    fn rewrites_inline_comment_into_comment_marks() {
        assert_eq!(
            review("See {==here==}{>>note<<}{#c1}.\n"),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "See " },
                        { "text": "here", "comment": true, "comment_c1": true },
                        { "text": "." }
                    ]
                }
            ])
        );
    }

    #[test]
    fn comment_only_marker_becomes_a_single_spaced_comment_leaf() {
        assert_eq!(
            review("Hi {>>note<<}{#c2} there\n"),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "Hi " },
                        { "text": " ", "comment": true, "comment_c2": true },
                        { "text": " there" }
                    ]
                }
            ])
        );
    }

    #[test]
    fn highlight_without_comment_is_plain_text() {
        assert_eq!(
            review("a {==hi==} b\n"),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "a " },
                        { "text": "hi" },
                        { "text": " b" }
                    ]
                }
            ])
        );
    }

    #[test]
    fn carries_surrounding_marks_onto_comment_segments() {
        assert_eq!(
            review("**bold {==x==}{#c3} end**\n"),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "bold": true, "text": "bold " },
                        { "bold": true, "comment": true, "comment_c3": true, "text": "x" },
                        { "bold": true, "text": " end" }
                    ]
                }
            ])
        );
    }

    #[test]
    fn leaves_critic_markup_inside_inline_code_literal() {
        assert_eq!(
            review("`{==x==}{#c4}`\n"),
            json!([
                {
                    "type": "p",
                    "children": [{ "code": true, "text": "{==x==}{#c4}" }]
                }
            ])
        );
    }

    #[test]
    fn rewrites_insert_suggestion_with_meta_user_and_timestamp() {
        assert_eq!(
            review_with(
                "Add {++word++}{#s1}.\n",
                &suggestion_meta("s1", "ai:codex", AT)
            ),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "Add " },
                        {
                            "text": "word",
                            "suggestion": true,
                            "suggestion_s1": { "id": "s1", "type": "insert", "userId": "ai:codex", "createdAt": AT_MS }
                        },
                        { "text": "." }
                    ]
                }
            ])
        );
    }

    #[test]
    fn rewrites_delete_suggestion_as_remove_mark() {
        assert_eq!(
            review_with(
                "Drop {--gone--}{#s2}!\n",
                &suggestion_meta("s2", "ai:claude", AT)
            ),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "Drop " },
                        {
                            "text": "gone",
                            "suggestion": true,
                            "suggestion_s2": { "id": "s2", "type": "remove", "userId": "ai:claude", "createdAt": AT_MS }
                        },
                        { "text": "!" }
                    ]
                }
            ])
        );
    }

    #[test]
    fn expands_substitution_into_paired_remove_and_insert() {
        assert_eq!(
            review_with(
                "Use {~~old~>new~~}{#s3} please\n",
                &suggestion_meta("s3", "ai:claude", AT)
            ),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "Use " },
                        {
                            "text": "old",
                            "suggestion": true,
                            "suggestion_s3": { "id": "s3", "type": "remove", "userId": "ai:claude", "createdAt": AT_MS }
                        },
                        {
                            "text": "new",
                            "suggestion": true,
                            "suggestion_s3": { "id": "s3", "type": "insert", "userId": "ai:claude", "createdAt": AT_MS }
                        },
                        { "text": " please" }
                    ]
                }
            ])
        );
    }

    #[test]
    fn unparseable_timestamp_becomes_zero_created_at() {
        assert_eq!(
            review_with(
                "Add {++x++}{#s4}\n",
                &suggestion_meta("s4", "ai:codex", "not-a-date")
            ),
            json!([
                {
                    "type": "p",
                    "children": [
                        { "text": "Add " },
                        {
                            "text": "x",
                            "suggestion": true,
                            "suggestion_s4": { "id": "s4", "type": "insert", "userId": "ai:codex", "createdAt": 0 }
                        }
                    ]
                }
            ])
        );
    }

    #[test]
    fn suggestion_without_id_is_unsupported() {
        assert!(review_block_to_slate("Add {++x++} now\n", &ReviewMeta::default()).is_err());
        assert!(review_block_to_slate("Use {~~a~>b~~} now\n", &ReviewMeta::default()).is_err());
    }

    #[test]
    fn suggestion_without_endmatter_entry_is_unsupported() {
        assert!(review_block_to_slate("Add {++x++}{#s9} now\n", &ReviewMeta::default()).is_err());
    }

    #[test]
    fn block_conversion_preserves_unsupported_reason() {
        let blocks = vec!["Add {++x++} now\n".to_string()];

        let error = review_blocks_to_slate(&blocks).unwrap_err();

        assert_eq!(error.0, "block 0: review marker without {#id}");
    }
}
