//! Import Markdown and its review syntax once, at the native identity boundary.
use crate::{Attrs, Node, ReviewMeta, ReviewMetaEntry, Unsupported};
use quarry_document::{
    Comment, DiscussionState, Document, Proposal, ProposalAction, ProposalState, ReviewMetadata,
    SeedBlock, TextRange,
};
use std::collections::BTreeMap;

#[derive(Clone)]
enum Owner {
    Block(String),
    Proposal(String),
}
struct Range {
    owner: Owner,
    start: usize,
    end: usize,
}
struct Formatting {
    range: Range,
    marks: Attrs,
}
#[derive(Default)]
struct Suggestion {
    block: String,
    start: usize,
    removed: Vec<Range>,
    inserted: String,
    delete_block: bool,
}
#[derive(Default)]
struct Import {
    seeds: Vec<SeedBlock>,
    comments: BTreeMap<String, Vec<Range>>,
    suggestions: BTreeMap<String, Suggestion>,
    formatting: Vec<Formatting>,
}

fn err(error: impl std::fmt::Display) -> Unsupported {
    Unsupported::new(error.to_string())
}
fn metadata(entry: &ReviewMetaEntry) -> ReviewMetadata {
    ReviewMetadata {
        created_at: entry.at.clone(),
        updated_at: entry.edited_at.clone().unwrap_or_else(|| entry.at.clone()),
        unattached_reason: None,
        legacy_record: Some(serde_json::to_value(entry).expect("review metadata serializes")),
    }
}

pub fn import_markdown_document(
    id: &str,
    markdown: &str,
    mint_id: &mut impl FnMut() -> String,
) -> Result<Document, Unsupported> {
    let (nodes, meta) = review_nodes(markdown, mint_id)?;
    import_review_nodes(id, &nodes, &meta, mint_id)
}

fn review_nodes(
    markdown: &str,
    mint_id: &mut impl FnMut() -> String,
) -> Result<(Vec<Node>, ReviewMeta), Unsupported> {
    let markdown = crate::review::identify_markers(markdown, mint_id);
    let parsed = crate::parse_review_document(&markdown);
    let (_, mut meta) = crate::split_review_endmatter(&markdown);
    meta.comments.extend(parsed.meta.comments);
    meta.suggestions.extend(parsed.meta.suggestions);
    let nodes = if parsed.markers.comments.is_empty() && parsed.markers.suggestions.is_empty() {
        crate::block_rows_to_nodes(&crate::markdown_to_block_rows(&parsed.body, &mut *mint_id)?)?
    } else {
        crate::review_block_to_nodes(&parsed.body, &meta)?
    };
    Ok((nodes, meta))
}

/// Reuse existing block IDs only when the entire content and structure agree.
/// On disagreement, return a separate import for preserving review metadata;
/// the caller must not transplant its text targets into the existing document.
pub fn import_markdown_with_existing_blocks(
    id: &str,
    markdown: &str,
    existing: &[crate::BlockRow],
    mint_id: &mut impl FnMut() -> String,
) -> Result<(Document, bool), Unsupported> {
    let (nodes, meta) = review_nodes(markdown, mint_id)?;
    let imported = import_review_nodes(id, &nodes, &meta, mint_id)?;
    let projected = crate::document_to_block_rows(&imported)?;
    if projected.len() != existing.len() {
        return Ok((imported, false));
    }
    let mapping: BTreeMap<_, _> = projected
        .iter()
        .zip(existing)
        .map(|(a, b)| (&a.block_id, &b.block_id))
        .collect();
    for (mut candidate, row) in projected.iter().cloned().zip(existing) {
        candidate.block_id = row.block_id.clone();
        candidate.parent_block_id = candidate
            .parent_block_id
            .as_ref()
            .and_then(|id| mapping.get(id).map(|id| (*id).clone()));
        if candidate != *row {
            return Ok((imported, false));
        }
    }
    let mut ids = existing.iter();
    let mut missing_id = false;
    let document = import_review_nodes(id, &nodes, &meta, &mut || match ids.next() {
        Some(row) => row.block_id.clone(),
        None => {
            missing_id = true;
            mint_id()
        }
    })?;
    if missing_id || ids.next().is_some() {
        return Err(err(
            "Review import block traversal disagrees with the verified projection",
        ));
    }
    Ok((document, true))
}

/// Also used for structured clipboard imports. Review ranges remain attached
/// to the proposal source when their text has not been accepted into the body.
pub fn import_review_nodes(
    id: &str,
    nodes: &[Node],
    meta: &ReviewMeta,
    mint_id: &mut impl FnMut() -> String,
) -> Result<Document, Unsupported> {
    let mut import = Import::default();
    for (position, node) in nodes.iter().enumerate() {
        import.block(node, None, position, mint_id)?;
    }
    let mut doc = Document::with_id(id).map_err(err)?;
    doc.import_blocks(&import.seeds).map_err(err)?;
    for (id, suggestion) in &import.suggestions {
        let entry = meta
            .suggestions
            .get(id)
            .cloned()
            .unwrap_or_else(default_entry);
        if suggestion.delete_block {
            doc.propose_block_delete(id, &entry.by, &suggestion.block)
                .map_err(err)?;
        } else {
            let at = doc
                .point(&suggestion.block, suggestion.start)
                .map_err(err)?;
            let mut ranges = Vec::new();
            for range in &suggestion.removed {
                ranges.extend(range.resolve(&doc)?);
            }
            doc.propose_replacement(
                id,
                &entry.by,
                &suggestion.block,
                &at,
                &ranges,
                &suggestion.inserted,
            )
            .map_err(err)?;
        }
        doc.set_proposal_details(
            id,
            entry.body.as_deref().unwrap_or_default(),
            metadata(&entry),
        )
        .map_err(err)?;
    }
    if !import.formatting.is_empty() {
        let mut formatted = doc.command_builder("import_format", "").map_err(err)?;
        for formatting in &import.formatting {
            let ranges = formatting.range.resolve(&formatted)?;
            formatted
                .push(
                    formatting
                        .marks
                        .iter()
                        .map(|(name, value)| quarry_document::Command::Format {
                            ranges: ranges.clone(),
                            name: name.clone(),
                            value: value.clone(),
                        })
                        .collect(),
                )
                .map_err(err)?;
        }
        doc = formatted.finish().map_err(err)?.0;
    }
    // Retain closed or missing proposals as explicit records. Do not invent a
    // target from a matching quote or silently discard endmatter entries.
    for (id, entry) in &meta.suggestions {
        if !import.suggestions.contains_key(id) {
            doc.import_unavailable_proposal(Proposal {
                id: id.clone(),
                author: entry.by.clone(),
                body: entry.body.clone().unwrap_or_default(),
                action: ProposalAction::Unavailable {
                    reason: "Imported suggestion has no marked text".into(),
                },
                segments: Vec::new(),
                state: if entry.resolved.is_some() {
                    ProposalState::Closed
                } else {
                    ProposalState::Open
                },
                metadata: metadata(entry),
            })
            .map_err(err)?;
        }
    }
    let mut entries = meta.comments.clone();
    for id in import.comments.keys() {
        entries.entry(id.clone()).or_insert_with(default_entry);
    }
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by_key(|(_, entry)| entry.re.is_some());
    for (id, entry) in entries {
        let body = entry.body.as_deref().unwrap_or_default();
        let mut info = metadata(&entry);
        if let Some(parent) = &entry.re {
            if doc.reply_comment(&id, parent, &entry.by, body).is_ok() {
                doc.set_comment_metadata(&id, info).map_err(err)?;
                continue;
            }
            info.unattached_reason = Some("Imported reply parent is unavailable".into());
        }
        let mut ranges = Vec::new();
        if let Some(target) = import.comments.get(&id) {
            for range in target {
                ranges.extend(range.resolve(&doc)?);
            }
        }
        if !ranges.is_empty() && info.unattached_reason.is_none() {
            doc.add_comment(&id, &entry.by, body, &ranges)
                .map_err(err)?;
            doc.set_comment_metadata(&id, info).map_err(err)?;
            if entry.resolved.is_some() || entry.status.as_deref() == Some("resolved") {
                doc.resolve_comment(&id, true).map_err(err)?;
            }
        } else {
            info.unattached_reason
                .get_or_insert_with(|| "Imported comment has no marked text".into());
            doc.import_unattached_comment(Comment {
                id,
                author: entry.by,
                body: body.into(),
                target: Vec::new(),
                original_quote: String::new(),
                state: if entry.resolved.is_some() {
                    DiscussionState::Resolved
                } else {
                    DiscussionState::Open
                },
                parent_id: None,
                deleted: false,
                metadata: info,
            })
            .map_err(err)?;
        }
    }
    Ok(doc)
}

impl Range {
    fn resolve(&self, doc: &Document) -> Result<Vec<TextRange>, Unsupported> {
        match &self.owner {
            Owner::Block(id) => doc.selection(id, self.start, self.end),
            Owner::Proposal(id) => doc.proposal_selection(id, self.start, self.end),
        }
        .map_err(err)
    }
}
fn default_entry() -> ReviewMetaEntry {
    ReviewMetaEntry {
        by: "unknown".into(),
        at: String::new(),
        kind: None,
        edited_at: None,
        body: None,
        re: None,
        status: None,
        resolved: None,
    }
}

impl Import {
    fn block(
        &mut self,
        node: &Node,
        parent: Option<String>,
        position: usize,
        mint: &mut impl FnMut() -> String,
    ) -> Result<(), Unsupported> {
        let Node::Element {
            ty,
            attrs,
            children,
        } = node
        else {
            return Err(err("Bare text at block level"));
        };
        let id = mint();
        let mut attrs = attrs.clone();
        attrs.shift_remove("id");
        if let Some(suggestion) = attrs.shift_remove("suggestion") {
            if suggestion["type"] == "remove" {
                let proposal = suggestion["id"]
                    .as_str()
                    .ok_or_else(|| err("Block deletion is missing its id"))?;
                self.suggestions.insert(
                    proposal.into(),
                    Suggestion {
                        block: id.clone(),
                        delete_block: true,
                        ..Default::default()
                    },
                );
            } else {
                return Err(err("Unsupported block suggestion"));
            }
        }
        let container = children.iter().any(|child| matches!(child, Node::Element {ty,..} if !matches!(ty.as_str(), "a" | "wikilink" | "img")));
        let index = self.seeds.len();
        self.seeds.push(SeedBlock {
            id: id.clone(),
            kind: ty.clone(),
            parent,
            position,
            attrs: attrs.into_iter().collect(),
            text: String::new(),
        });
        if container {
            for (position, child) in children.iter().enumerate() {
                self.block(child, Some(id.clone()), position, mint)?;
            }
        } else if ty != "raw_markdown" && ty != "img" && ty != "hr" && ty != "mermaid" {
            let mut text = String::new();
            self.inline(&id, children, &Attrs::new(), &mut text)?;
            self.seeds[index].text = text;
        }
        Ok(())
    }

    fn inline(
        &mut self,
        block: &str,
        nodes: &[Node],
        inherited: &Attrs,
        text: &mut String,
    ) -> Result<(), Unsupported> {
        for node in nodes {
            match node {
                Node::Text { text: chunk, marks } => {
                    let mut formatting = inherited.clone();
                    for (key, value) in marks {
                        if crate::is_known_inline_mark(key) {
                            formatting.insert(key.clone(), value.clone());
                        }
                    }
                    let inserted: Vec<_> = marks
                        .iter()
                        .filter_map(|(key, value)| {
                            key.strip_prefix("suggestion_")
                                .filter(|_| value["type"] == "insert")
                        })
                        .collect();
                    if inserted.len() > 1 {
                        return Err(err("Text belongs to multiple inserted suggestions"));
                    }
                    let offset = text.encode_utf16().count();
                    let (owner, start, end) = if let Some(id) = inserted.first() {
                        let suggestion =
                            self.suggestions
                                .entry((*id).into())
                                .or_insert_with(|| Suggestion {
                                    block: block.into(),
                                    start: offset,
                                    ..Default::default()
                                });
                        let start = suggestion.inserted.encode_utf16().count();
                        suggestion.inserted.push_str(chunk);
                        (
                            Owner::Proposal((*id).into()),
                            start,
                            suggestion.inserted.encode_utf16().count(),
                        )
                    } else {
                        text.push_str(chunk);
                        let end = text.encode_utf16().count();
                        for (key, value) in marks {
                            if let Some(id) = key
                                .strip_prefix("suggestion_")
                                .filter(|_| value["type"] == "remove")
                            {
                                let suggestion =
                                    self.suggestions.entry(id.into()).or_insert_with(|| {
                                        Suggestion {
                                            block: block.into(),
                                            start: offset,
                                            ..Default::default()
                                        }
                                    });
                                if !suggestion.delete_block && offset < end {
                                    suggestion.removed.push(Range {
                                        owner: Owner::Block(block.into()),
                                        start: offset,
                                        end,
                                    });
                                }
                            }
                        }
                        (Owner::Block(block.into()), offset, end)
                    };
                    if start < end {
                        for (key, value) in marks {
                            if let Some(id) = key
                                .strip_prefix("comment_")
                                .filter(|_| value == &serde_json::Value::Bool(true))
                            {
                                self.comments.entry(id.into()).or_default().push(Range {
                                    owner: owner.clone(),
                                    start,
                                    end,
                                });
                            }
                        }
                        if !formatting.is_empty() {
                            self.formatting.push(Formatting {
                                range: Range { owner, start, end },
                                marks: formatting,
                            });
                        }
                    }
                }
                Node::Element {
                    ty,
                    attrs,
                    children,
                } if ty == "a" => {
                    let mut marks = inherited.clone();
                    marks.insert(
                        "link".into(),
                        attrs
                            .get("url")
                            .cloned()
                            .ok_or_else(|| err("Link has no URL"))?,
                    );
                    self.inline(block, children, &marks, text)?;
                }
                Node::Element {
                    ty,
                    attrs,
                    children,
                } if ty == "wikilink" => {
                    let mut marks = Attrs::new();
                    marks.insert("wikilink".into(), serde_json::Value::Bool(true));
                    for child in children {
                        if let Node::Text {
                            marks: child_marks, ..
                        } = child
                        {
                            marks.extend(child_marks.clone());
                        }
                    }
                    self.inline(
                        block,
                        &[Node::text(
                            crate::markdown_writer::render_wikilink(attrs),
                            marks,
                        )],
                        inherited,
                        text,
                    )?;
                }
                Node::Element { ty, .. } => {
                    return Err(err(format!("Unsupported inline element: {ty}")));
                }
            }
        }
        Ok(())
    }
}
