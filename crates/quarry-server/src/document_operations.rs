//! The agent operation API translates saved-version offsets into document commands. Offsets are read
//! against the caller's saved version, then become native commands. No editor
//! tree or JSON text snapshot is reconciled into the canonical document.
use super::{
    Attrs, BlockOp, DeterministicIds, GatewayError, GatewayFailure, LinkRange, MarkRun,
    PlanProvider, TransactionContext, TransactionReply, TransactionSettings, block_mutation_reply,
    commit_provenance, normalize_list_attrs, now_timestamp, parse_markdown_fragment,
    transaction_status, unquote_clock, utf16_slice, validate_attrs, validate_block_type,
    validate_inline_ranges,
};
use crate::{AppState, document_engine};
use quarry_core::QuarryError;
use quarry_document::{Command, CommandBuilder, Document, SeedBlock};
use quarry_storage::{BlockMutationState, DocumentScopeRef, DurableDocumentCommit};
use serde_json::json;

fn error(e: quarry_document::DocumentError) -> GatewayFailure {
    use super::GatewayErrorCode;
    use quarry_document::DocumentError;
    match &e {
        DocumentError::Invalid(_)
        | DocumentError::InvalidOffset(_)
        | DocumentError::Json(_)
        | DocumentError::AlreadyExists { .. } => {
            return GatewayError::invalid(e.to_string()).into();
        }
        DocumentError::NotFound {
            kind: "comments" | "proposals" | "conflicts",
            id,
        } => {
            return GatewayError::new(GatewayErrorCode::AnchorNotFound, e.to_string())
                .with_target("review_item", id)
                .into();
        }
        DocumentError::NotFound { kind: "blocks", id } => {
            return GatewayError::block_deleted(id).into();
        }
        DocumentError::Conflict(message) if message == "Proposal already decided" => {
            return GatewayError::new(GatewayErrorCode::SuggestionAlreadyResolved, e.to_string())
                .into();
        }
        DocumentError::Conflict(message)
            if message.starts_with("Proposed ") || message.starts_with("Proposal position") =>
        {
            return GatewayError::new(GatewayErrorCode::SuggestionInvalidated, e.to_string())
                .into();
        }
        _ => {}
    }
    document_engine::document_error(e).into()
}

pub(super) async fn apply_transaction(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    ctx: &TransactionContext,
    settings: &TransactionSettings,
    plan: PlanProvider<'_>,
) -> Result<TransactionReply, GatewayFailure> {
    let snapshot = state
        .store
        .block_mutation_state_for_scope(scope, path, &ctx.client_tx_id)
        .await?;
    if quarry_storage::document_kind(path, &snapshot.content_type)
        == quarry_storage::DocumentKind::RawDocument
    {
        return Err(GatewayError::new(
            super::GatewayErrorCode::UnsupportedBlockDocument,
            "Only Markdown documents support document commands",
        )
        .into());
    }
    let planned = plan(&snapshot)?;
    let fingerprint = json!({"client_tx_id":ctx.client_tx_id,"base_clock":ctx.base_clock,"actor":ctx.actor,
        "ops":planned.ops_json,"metadata":settings.metadata,"source":settings.source,
        "attribution":settings.transaction.actor,"message":settings.transaction.message,"provenance":settings.transaction.provenance});
    let hash = blake3::hash(fingerprint.to_string().as_bytes())
        .to_hex()
        .to_string();
    if let Some(record) = state
        .store
        .replay_durable_request_for_scope(scope, path, &ctx.client_tx_id, &hash)
        .await?
    {
        return Ok(TransactionReply::Replayed(record));
    }
    let saved = state
        .store
        .durable_document_for_scope(scope, path)
        .await?
        .ok_or_else(|| {
            QuarryError::PreconditionFailed("Native document state is missing".into())
        })?;
    if saved.version_id != snapshot.head_version_id {
        return Err(
            QuarryError::PreconditionFailed("Document changed while loading".into()).into(),
        );
    }
    let status = transaction_status(&ctx.base_clock, &snapshot)?;
    let mut authority = Document::load(&saved.bytes).map_err(error)?;
    let base_clock = if planned.uses_current_snapshot {
        None
    } else {
        ctx.base_clock.as_ref()
    };
    let base = match base_clock {
        Some(clock) => {
            let clock =
                unquote_clock(clock).ok_or_else(|| GatewayError::invalid("Invalid base clock"))?;
            state
                .store
                .durable_heads_for_scope(scope, path, &clock)
                .await?
                .ok_or_else(|| {
                    QuarryError::PreconditionFailed(
                        "Read the document again; this version predates its native history".into(),
                    )
                })?
        }
        None => authority.heads().iter().map(ToString::to_string).collect(),
    };
    let heads = base
        .iter()
        .map(|h| h.parse())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| GatewayError::invalid("Invalid native version"))?;
    let mut draft = Draft {
        document: authority
            .fork_at(&heads)
            .map_err(error)?
            .command_builder(
                &format!(
                    "agent_{}",
                    blake3::hash(ctx.client_tx_id.as_bytes()).to_hex()
                ),
                &now_timestamp(),
            )
            .map_err(error)?,
        ids: DeterministicIds::new(&snapshot.document_id, &ctx.client_tx_id),
        author: ctx.actor.display(),
        at: now_timestamp(),
    };
    for (index, op) in planned.ops.iter().enumerate() {
        draft
            .apply_op(op, &snapshot)
            .map_err(|failure| operation_error(failure, index, op))?;
    }
    let (candidate, request) = draft.document.finish().map_err(error)?;
    if !request.commands.is_empty() {
        let mut current = authority.heads();
        current.sort();
        let mut base = heads.clone();
        base.sort();
        if current == base {
            authority = candidate;
        } else {
            authority.apply_request(&request).map_err(error)?;
        }
    }
    let mut commit = document_engine::build_commit(
        &snapshot,
        &authority,
        &ctx.client_tx_id,
        &ctx.actor,
        fingerprint,
    )?;
    commit.metadata = settings
        .metadata
        .clone()
        .unwrap_or_else(|| snapshot.metadata.clone());
    commit.normalized_markdown = super::normalized_markdown(&commit.rows, &commit.metadata)?;
    commit.source = settings.source.clone();
    commit.origin_id = settings.origin_id.clone();
    commit.transaction_actor = settings
        .transaction
        .actor
        .clone()
        .or_else(|| Some(ctx.actor.display()));
    commit.transaction_message = settings.transaction.message.clone();
    commit.transaction_provenance = commit_provenance(settings);
    commit.recorded_ops["ops"] = planned.ops_json;
    commit.recorded_ops["ack"]["status"] = json!(status);
    let changed = serde_json::from_value(commit.recorded_ops["ack"]["changed_block_ids"].clone())
        .map_err(|e| GatewayError::invalid(e.to_string()))?;
    let conflicts = authority
        .conflicts()
        .map_err(error)?
        .iter()
        .filter(|c| !snapshot.review_items.iter().any(|r| r.id == c.id))
        .map(|c| c.id.clone())
        .collect();
    let outcome = state
        .store
        .commit_durable_document_for_scope(
            scope,
            commit,
            DurableDocumentCommit {
                bytes: authority.save(),
                request_hash: hash,
            },
        )
        .await?;
    Ok(block_mutation_reply(outcome, status, changed, conflicts))
}

struct Draft {
    document: CommandBuilder,
    ids: DeterministicIds,
    author: String,
    at: String,
}

impl Draft {
    fn apply(&mut self, commands: Vec<Command>) -> Result<(), GatewayFailure> {
        self.document.push(commands).map_err(error)
    }

    fn apply_op(
        &mut self,
        op: &BlockOp,
        snapshot: &BlockMutationState,
    ) -> Result<(), GatewayFailure> {
        if let Some(("block", id)) = op.primary_target()
            && !matches!(op, BlockOp::InsertBlock { .. })
            && (self.document.block(id).is_err()
                || self.document.block(id).is_ok_and(|b| b.deleted))
        {
            return Err(GatewayError::block_deleted(id).into());
        }
        match op {
            BlockOp::InsertBlock {
                block_id,
                parent_block_id,
                position,
                block_type,
                attrs,
                text,
                marks,
                links,
            } => {
                let id = block_id.clone().unwrap_or_else(|| self.ids.mint());
                self.insert(
                    SeedBlock {
                        id,
                        kind: block_type.clone(),
                        parent: parent_block_id.clone(),
                        position: *position as usize,
                        attrs: attrs.clone().into_iter().collect(),
                        text: text.clone(),
                    },
                    marks,
                    links,
                )
            }
            BlockOp::InsertMarkdown {
                after_block_id,
                markdown,
            } => self.insert_markdown(after_block_id.as_deref(), markdown),
            BlockOp::DeleteBlock { block_id } => self.apply(vec![Command::DeleteBlock {
                block: block_id.clone(),
            }]),
            BlockOp::MoveBlock {
                block_id,
                parent_block_id,
                position,
            } => {
                let siblings: Vec<_> = self
                    .document
                    .blocks()
                    .map_err(error)?
                    .into_iter()
                    .filter(|b| b.parent == *parent_block_id && b.id != *block_id)
                    .collect();
                let before = siblings.get(*position as usize).map(|b| b.id.clone());
                self.apply(vec![Command::MoveBlock {
                    block: block_id.clone(),
                    parent: parent_block_id.clone(),
                    before,
                }])
            }
            BlockOp::ReplaceBlockContent {
                block_id,
                text,
                marks,
                links,
            } => self.replace(block_id, text, marks.as_deref(), links.as_deref()),
            BlockOp::SetBlockAttrs { block_id, attrs } => {
                validate_attrs(attrs)?;
                let block = self.document.block(block_id).map_err(error)?;
                let attrs = normalize_list_attrs(&block.kind, attrs)?;
                if block.kind == "raw_markdown" {
                    super::validate_raw_markdown_attrs(&attrs)?;
                }
                self.apply(vec![Command::SetBlock {
                    block: block_id.clone(),
                    kind: block.kind,
                    attrs: attrs.into_iter().collect(),
                }])
            }
            BlockOp::SetBlockType {
                block_id,
                block_type,
                attrs,
            } => {
                validate_block_type(block_type)?;
                match attrs {
                    Some(attrs) => self.apply(vec![Command::SetBlock {
                        block: block_id.clone(),
                        kind: block_type.clone(),
                        attrs: normalize_list_attrs(block_type, attrs)?
                            .into_iter()
                            .collect(),
                    }]),
                    None => self.apply(vec![Command::Edit {
                        mode: quarry_document::EditMode::Direct,
                        action: quarry_document::EditAction::ConvertBlock {
                            block: block_id.clone(),
                            proposal: None,
                            target: quarry_document::BlockConversion::plain(block_type),
                        },
                    }]),
                }
            }
            BlockOp::AddMark {
                block_id,
                start,
                end,
                marks,
            } => self.marks(block_id, *start, *end, marks),
            BlockOp::RemoveMark {
                block_id,
                start,
                end,
                marks,
            } => self.marks(
                block_id,
                *start,
                *end,
                &marks
                    .iter()
                    .map(|name| (name.clone(), serde_json::Value::Null))
                    .collect(),
            ),
            BlockOp::SetLink {
                block_id,
                start,
                end,
                url,
            } => self.marks(
                block_id,
                *start,
                *end,
                &[(
                    "link".into(),
                    url.clone().map_or(serde_json::Value::Null, Into::into),
                )]
                .into_iter()
                .collect(),
            ),
            BlockOp::CommentAdd {
                block_id,
                start,
                end,
                body,
                quote,
            } => {
                let ranges = self.range(block_id, *start, *end, quote.as_deref())?;
                let id = self.ids.mint();
                self.apply(vec![Command::AddComment {
                    id,
                    author: self.author.clone(),
                    body: body.clone(),
                    ranges,
                }])
            }
            BlockOp::CommentReply { item_id, body } => {
                let id = self.ids.mint();
                self.apply(vec![Command::ReplyComment {
                    id,
                    parent: item_id.clone(),
                    author: self.author.clone(),
                    body: body.clone(),
                }])
            }
            BlockOp::CommentEdit { item_id, body } => self.apply(vec![Command::EditComment {
                id: item_id.clone(),
                body: body.clone(),
            }]),
            BlockOp::CommentResolve { item_id } => self.apply(vec![Command::ResolveComment {
                id: item_id.clone(),
                resolved: true,
            }]),
            BlockOp::CommentDelete { item_id } => self.apply(vec![Command::DeleteComment {
                id: item_id.clone(),
            }]),
            BlockOp::SuggestionAdd {
                block_id,
                start,
                end,
                replacement,
                body,
                quote,
            } => {
                let ranges = self.range(block_id, *start, *end, quote.as_deref())?;
                let at = self
                    .document
                    .point(block_id, *start as usize)
                    .map_err(error)?;
                let id = self.ids.mint();
                self.apply(vec![Command::ProposeReplacement {
                    id: id.clone(),
                    author: self.author.clone(),
                    block: block_id.clone(),
                    at,
                    ranges,
                    text: replacement.clone(),
                }])?;
                self.proposal_body(&id, body.as_deref())
            }
            BlockOp::SuggestionAddBlockDelete { block_id, body, .. } => {
                let id = self.ids.mint();
                self.apply(vec![Command::ProposeBlockDelete {
                    id: id.clone(),
                    author: self.author.clone(),
                    block: block_id.clone(),
                }])?;
                self.proposal_body(&id, body.as_deref())
            }
            BlockOp::SuggestionAddMarkdown {
                after_block_id,
                markdown,
                body,
            } => {
                let position = self.insert_position(after_block_id.as_deref())?;
                let before = self
                    .document
                    .blocks()
                    .map_err(error)?
                    .into_iter()
                    .filter(|b| b.parent.is_none())
                    .nth(position)
                    .map(|b| b.id);
                let rows = parse_markdown_fragment(markdown, || self.ids.mint())?;
                let id = self.ids.mint();
                let blocks = rows
                    .iter()
                    .map(|r| SeedBlock {
                        id: r.block_id.clone(),
                        kind: r.block_type.clone(),
                        parent: r.parent_block_id.clone(),
                        position: r.position as usize,
                        attrs: r.attrs.clone().into_iter().collect(),
                        text: r.text.clone(),
                    })
                    .collect();
                self.apply(vec![Command::ProposeBlocks {
                    id: id.clone(),
                    author: self.author.clone(),
                    parent: None,
                    before,
                    blocks,
                }])?;
                let mut offset = 0;
                for row in rows {
                    for mark in row.marks {
                        let ranges = self
                            .document
                            .proposal_selection(
                                &id,
                                offset + mark.start as usize,
                                offset + mark.end as usize,
                            )
                            .map_err(error)?;
                        self.apply(
                            mark.marks
                                .into_iter()
                                .map(|(name, value)| Command::Format {
                                    ranges: ranges.clone(),
                                    name,
                                    value,
                                })
                                .collect(),
                        )?;
                    }
                    for link in row.links {
                        let ranges = self
                            .document
                            .proposal_selection(
                                &id,
                                offset + link.start as usize,
                                offset + link.end as usize,
                            )
                            .map_err(error)?;
                        self.apply(vec![Command::Format {
                            ranges,
                            name: "link".into(),
                            value: link.url.into(),
                        }])?;
                    }
                    offset += row.text.encode_utf16().count();
                }
                self.proposal_body(&id, body.as_deref())
            }
            BlockOp::SuggestionAccept { item_id } => self.apply(vec![Command::AcceptProposal {
                id: item_id.clone(),
            }]),
            BlockOp::SuggestionReject { item_id } => self.apply(vec![Command::RejectProposal {
                id: item_id.clone(),
            }]),
            BlockOp::ConflictAdd {
                after_block_id,
                base_markdown,
                incoming_markdown,
                canonical_markdown,
            } => {
                let id = self.ids.mint();
                self.apply(vec![Command::AddConflict {
                    conflict: quarry_document::Conflict {
                        id,
                        after: after_block_id.clone(),
                        base: base_markdown.clone(),
                        incoming: incoming_markdown.clone(),
                        canonical: canonical_markdown.clone(),
                        resolved: false,
                        author: self.author.clone(),
                        metadata: quarry_document::ReviewMetadata {
                            created_at: self.at.clone(),
                            updated_at: self.at.clone(),
                            ..Default::default()
                        },
                    },
                }])
            }
            BlockOp::ConflictKeepCanonical { item_id } => {
                self.apply(vec![Command::ResolveConflict {
                    id: item_id.clone(),
                }])
            }
            BlockOp::ConflictAcceptIncoming { item_id } => self.accept_conflict(item_id, snapshot),
        }
    }

    fn range(
        &self,
        block: &str,
        start: u32,
        end: u32,
        quote: Option<&str>,
    ) -> Result<Vec<quarry_document::TextRange>, GatewayFailure> {
        let ranges = self
            .document
            .selection(block, start as usize, end as usize)
            .map_err(error)?;
        if let Some(quote) = quote {
            let text = self.document.block_view(block).map_err(error)?.text;
            if utf16_slice(&text, start, end) != quote {
                return Err(GatewayError::invalid(
                    "The supplied quote does not match this version's range",
                )
                .into());
            }
        }
        Ok(ranges)
    }
    fn marks(
        &mut self,
        block: &str,
        start: u32,
        end: u32,
        marks: &Attrs,
    ) -> Result<(), GatewayFailure> {
        let ranges = self
            .document
            .selection(block, start as usize, end as usize)
            .map_err(error)?;
        self.apply(
            marks
                .iter()
                .map(|(name, value)| Command::Format {
                    ranges: ranges.clone(),
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        )
    }
    fn insert(
        &mut self,
        mut block: SeedBlock,
        marks: &[MarkRun],
        links: &[LinkRange],
    ) -> Result<(), GatewayFailure> {
        validate_block_type(&block.kind)?;
        validate_inline_ranges(&block.text, marks, links)?;
        let attrs: Attrs = block.attrs.into_iter().collect();
        validate_attrs(&attrs)?;
        block.attrs = normalize_list_attrs(&block.kind, &attrs)?
            .into_iter()
            .collect();
        let id = block.id.clone();
        self.apply(vec![Command::InsertBlock { block }])?;
        for mark in marks {
            self.marks(&id, mark.start, mark.end, &mark.marks)?;
        }
        for link in links {
            self.marks(
                &id,
                link.start,
                link.end,
                &[("link".into(), link.url.clone().into())]
                    .into_iter()
                    .collect(),
            )?;
        }
        Ok(())
    }
    fn insert_position(&self, after: Option<&str>) -> Result<usize, GatewayFailure> {
        let roots: Vec<_> = self
            .document
            .blocks()
            .map_err(error)?
            .into_iter()
            .filter(|b| b.parent.is_none())
            .collect();
        match after {
            None => Ok(0),
            Some(id) => roots
                .iter()
                .position(|b| b.id == id)
                .map(|i| i + 1)
                .ok_or_else(|| {
                    GatewayError::invalid("Insertion destination must be a live top-level block")
                        .into()
                }),
        }
    }
    fn insert_markdown(
        &mut self,
        after: Option<&str>,
        markdown: &str,
    ) -> Result<(), GatewayFailure> {
        let position = self.insert_position(after)?;
        let rows = parse_markdown_fragment(markdown, || self.ids.mint())?;
        for row in rows {
            self.insert(
                SeedBlock {
                    id: row.block_id,
                    kind: row.block_type,
                    position: row.position as usize
                        + if row.parent_block_id.is_none() {
                            position
                        } else {
                            0
                        },
                    parent: row.parent_block_id,
                    attrs: row.attrs.into_iter().collect(),
                    text: row.text,
                },
                &row.marks,
                &row.links,
            )?;
        }
        Ok(())
    }
    fn replace(
        &mut self,
        block: &str,
        text: &str,
        marks: Option<&[MarkRun]>,
        links: Option<&[LinkRange]>,
    ) -> Result<(), GatewayFailure> {
        validate_inline_ranges(text, marks.unwrap_or(&[]), links.unwrap_or(&[]))?;
        let previous = self.document.block_view(block).map_err(error)?.text;
        for hunk in quarry_markdown::utf16_text_diff_hunks(&previous, text) {
            let ranges = self
                .document
                .selection(block, hunk.prefix as usize, hunk.old_mid_end as usize)
                .map_err(error)?;
            let at = self
                .document
                .point(block, hunk.prefix as usize)
                .map_err(error)?;
            let inserted = utf16_slice(text, hunk.prefix, hunk.new_mid_end);
            let mut commands = Vec::new();
            if !ranges.is_empty() {
                commands.push(Command::DeleteText { ranges });
            }
            if !inserted.is_empty() {
                commands.push(Command::InsertText { at, text: inserted });
            }
            self.apply(commands)?;
        }
        let length = text.encode_utf16().count() as u32;
        if let Some(marks) = marks {
            let names: std::collections::BTreeSet<_> = self
                .document
                .block_view(block)
                .map_err(error)?
                .runs
                .into_iter()
                .flat_map(|r| r.marks.into_keys())
                .filter(|n| n != "link")
                .collect();
            self.marks(
                block,
                0,
                length,
                &names
                    .into_iter()
                    .map(|n| (n, serde_json::Value::Null))
                    .collect(),
            )?;
            for mark in marks {
                self.marks(block, mark.start, mark.end, &mark.marks)?;
            }
        }
        if let Some(links) = links {
            self.marks(
                block,
                0,
                length,
                &[("link".into(), serde_json::Value::Null)]
                    .into_iter()
                    .collect(),
            )?;
            for link in links {
                self.marks(
                    block,
                    link.start,
                    link.end,
                    &[("link".into(), link.url.clone().into())]
                        .into_iter()
                        .collect(),
                )?;
            }
        }
        Ok(())
    }
    fn proposal_body(&mut self, id: &str, body: Option<&str>) -> Result<(), GatewayFailure> {
        if let Some(body) = body {
            self.apply(vec![Command::EditProposal {
                id: id.into(),
                body: body.into(),
            }])?;
        }
        Ok(())
    }
    fn accept_conflict(
        &mut self,
        id: &str,
        _snapshot: &BlockMutationState,
    ) -> Result<(), GatewayFailure> {
        let conflict = self
            .document
            .conflicts()
            .map_err(error)?
            .into_iter()
            .find(|c| c.id == id && !c.resolved)
            .ok_or_else(|| GatewayError::invalid("Conflict is missing or resolved"))?;
        if conflict.base.trim().is_empty() && conflict.canonical.trim().is_empty() {
            return Err(
                GatewayError::invalid("Conflict marker warning can only be dismissed").into(),
            );
        }
        let position = self.insert_position(conflict.after.as_deref())?;
        let (count, expected) = super::normalize_conflict_fragment(&conflict.canonical)?;
        let rows = quarry_storage::document_projection(&self.document)?;
        let roots: Vec<_> = rows
            .iter()
            .filter(|r| r.parent_block_id.is_none())
            .collect();
        let region = roots
            .get(position..position + count)
            .ok_or_else(|| super::conflict_hunk_changed(id))?;
        let mut ids: std::collections::BTreeSet<_> =
            region.iter().map(|r| r.block_id.clone()).collect();
        for row in &rows {
            if row
                .parent_block_id
                .as_ref()
                .is_some_and(|p| ids.contains(p))
            {
                ids.insert(row.block_id.clone());
            }
        }
        let selected: Vec<_> = rows
            .iter()
            .filter(|r| ids.contains(&r.block_id))
            .cloned()
            .collect();
        if quarry_markdown::block_rows_to_markdown(&selected).map_err(QuarryError::from)?
            != expected
        {
            return Err(super::conflict_hunk_changed(id).into());
        }
        for row in region {
            self.apply(vec![Command::DeleteBlock {
                block: row.block_id.clone(),
            }])?;
        }
        if !conflict.incoming.trim().is_empty() {
            self.insert_markdown(conflict.after.as_deref(), &conflict.incoming)?;
        }
        self.apply(vec![Command::ResolveConflict { id: id.into() }])
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct TestProjection {
    pub rows: Vec<quarry_storage::BlockRow>,
    pub review_items: Vec<quarry_storage::BlockReviewItem>,
    pub changed_block_ids: Vec<String>,
}

#[cfg(test)]
pub(super) fn apply_test_ops(
    snapshot: &BlockMutationState,
    ops: &[BlockOp],
    actor: &super::BlockTransactionActor,
    request_id: &str,
) -> Result<TestProjection, GatewayError> {
    let document = quarry_storage::import_document(snapshot)
        .map_err(|e| GatewayError::invalid(e.to_string()))?;
    let mut draft = Draft {
        document: document
            .command_builder(
                &format!("test_{}", blake3::hash(request_id.as_bytes()).to_hex()),
                &now_timestamp(),
            )
            .map_err(|e| GatewayError::invalid(e.to_string()))?,
        ids: DeterministicIds::new(&snapshot.document_id, request_id),
        author: actor.display(),
        at: now_timestamp(),
    };
    for (index, op) in ops.iter().enumerate() {
        draft
            .apply_op(op, snapshot)
            .map_err(|failure| operation_error(failure, index, op))?;
    }
    let (document, _) = draft
        .document
        .finish()
        .map_err(|e| GatewayError::invalid(e.to_string()))?;
    let rows = quarry_storage::document_projection(&document)
        .map_err(|e| GatewayError::invalid(e.to_string()))?;
    let review_items = quarry_storage::document_review_projection(&document)
        .map_err(|e| GatewayError::invalid(e.to_string()))?;
    let changed_block_ids = rows
        .iter()
        .filter(|row| !snapshot.rows.contains(row))
        .map(|r| r.block_id.clone())
        .collect();
    Ok(TestProjection {
        rows,
        review_items,
        changed_block_ids,
    })
}

fn operation_error(failure: GatewayFailure, index: usize, op: &BlockOp) -> GatewayError {
    let error = match failure {
        GatewayFailure::Typed(error) => error,
        GatewayFailure::Api(error) => {
            if matches!(op, BlockOp::MoveBlock { .. }) {
                GatewayError::new(super::GatewayErrorCode::BlockMoveConflict, error.message())
            } else {
                GatewayError::invalid(error.message())
            }
        }
    }
    .with_operation(index, op.name());
    match op.primary_target() {
        Some((kind, id)) if !error.has_target() => error.with_target(kind, id),
        _ => error,
    }
}
