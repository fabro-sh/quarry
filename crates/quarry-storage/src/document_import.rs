//! One-time import from legacy rows. The exact legacy review record is kept;
//! an uncertain or previously missing anchor is never repaired by text search.
use crate::{
    BlockMutationState, BlockReviewItem, BlockReviewKind, BlockReviewState,
    MARKDOWN_INSERT_SUGGESTION_CONTEXT,
    document_state::{document_error, project_block_views},
};
use quarry_core::Result;
use quarry_document::{
    Comment, Conflict, DiscussionState, Document, Proposal, ProposalAction, ProposalState,
    ReviewMetadata, SeedBlock, TargetOwner,
};

pub fn import_document(state: &BlockMutationState) -> Result<Document> {
    let mut document = Document::with_id(&state.document_id).map_err(document_error)?;
    let seeds: Vec<_> = state
        .rows
        .iter()
        .map(|row| SeedBlock {
            id: row.block_id.clone(),
            kind: row.block_type.clone(),
            parent: row.parent_block_id.clone(),
            position: row.position as usize,
            attrs: row.attrs.clone().into_iter().collect(),
            text: row.text.clone(),
        })
        .collect();
    document.import_blocks(&seeds).map_err(document_error)?;
    for row in &state.rows {
        for mark in &row.marks {
            let ranges = document
                .selection(&row.block_id, mark.start as usize, mark.end as usize)
                .map_err(document_error)?;
            for (name, value) in &mark.marks {
                document
                    .format(&ranges, name, value)
                    .map_err(document_error)?;
            }
        }
        for link in &row.links {
            let ranges = document
                .selection(&row.block_id, link.start as usize, link.end as usize)
                .map_err(document_error)?;
            document
                .format(&ranges, "link", &link.url.clone().into())
                .map_err(document_error)?;
        }
    }
    import_review_items(&mut document, &state.review_items)?;
    quarry_markdown::block_rows_to_markdown(&crate::document_projection(&document)?)?;
    Ok(document)
}

pub fn import_document_with_markdown(
    state: &BlockMutationState,
    markdown: &str,
) -> Result<Document> {
    let mut document = import_document(state)?;
    let parsed = quarry_markdown::parse_review_document(markdown);
    if parsed.markers.comments.is_empty()
        && parsed.markers.suggestions.is_empty()
        && !quarry_markdown::has_review_endmatter(markdown)
    {
        return Ok(document);
    }
    let existing = crate::document_projection(&document)?;
    let (mut imported, matches) = quarry_markdown::import_markdown_with_existing_blocks(
        &state.document_id,
        markdown,
        &existing,
        &mut || uuid::Uuid::new_v4().to_string(),
    )?;
    if matches {
        import_review_items(&mut imported, &state.review_items)?;
        return Ok(imported);
    }
    let reason = "Markdown and stored blocks disagreed at import; the original review is retained without guessing a target";
    // Proposals precede replies. Their complete preview is kept even though
    // their destination cannot be established from the old database.
    for view in imported.view().map_err(document_error)?.proposals {
        let mut proposal = view.proposal.clone();
        if let Ok(prior) = document.proposal(&proposal.id) {
            let mut metadata = prior.metadata;
            metadata.legacy_record = Some(
                serde_json::json!({"stored_review":metadata.legacy_record,"markdown_review":view}),
            );
            document
                .set_proposal_details(&prior.id, &prior.body, metadata)
                .map_err(document_error)?;
            continue;
        }
        proposal.metadata.legacy_record =
            Some(serde_json::to_value(&view).map_err(document_error)?);
        proposal.metadata.unattached_reason = Some(reason.into());
        proposal.action = ProposalAction::Unavailable {
            reason: reason.into(),
        };
        proposal.segments.clear();
        document
            .import_unavailable_proposal(proposal)
            .map_err(document_error)?;
    }
    let mut comments = imported.comments().map_err(document_error)?;
    comments.sort_by_key(|comment| comment.parent_id.is_some());
    for mut comment in comments {
        if let Ok(prior) = document.comment(&comment.id) {
            let mut metadata = prior.metadata;
            metadata.legacy_record = Some(
                serde_json::json!({"stored_review":metadata.legacy_record,"markdown_review":comment}),
            );
            document
                .set_comment_metadata(&prior.id, metadata)
                .map_err(document_error)?;
            continue;
        }
        comment.metadata.legacy_record =
            Some(serde_json::to_value(&comment).map_err(document_error)?);
        comment.metadata.unattached_reason = Some(reason.into());
        comment.target.clear();
        document
            .import_unattached_comment(comment)
            .map_err(document_error)?;
    }
    Ok(document)
}

pub(crate) fn import_review_items(
    document: &mut Document,
    review_items: &[BlockReviewItem],
) -> Result<()> {
    let mut items = review_items.to_vec();
    items.sort_by_key(|i| i.parent_item_id.is_some());
    for item in &items {
        // Review syntax can already have imported the same durable id.
        if document.comment(&item.id).is_ok()
            || document.proposal(&item.id).is_ok()
            || document.conflict(&item.id).is_ok()
        {
            // Keep both representations when the old stores disagreed.
            if let Ok(comment) = document.comment(&item.id) {
                let mut metadata = comment.metadata;
                metadata.legacy_record = Some(
                    serde_json::json!({"markdown_review":metadata.legacy_record,"stored_review":item}),
                );
                if item.updated_at >= metadata.updated_at
                    && let Some(body) = &item.body
                {
                    document
                        .edit_comment(&item.id, body)
                        .map_err(document_error)?;
                    metadata.updated_at = item.updated_at.clone();
                }
                if item.state == BlockReviewState::Resolved && comment.parent_id.is_none() {
                    document
                        .resolve_comment(&item.id, true)
                        .map_err(document_error)?;
                }
                document
                    .set_comment_metadata(&item.id, metadata)
                    .map_err(document_error)?;
            } else if let Ok(proposal) = document.proposal(&item.id) {
                let mut metadata = proposal.metadata;
                metadata.legacy_record = Some(
                    serde_json::json!({"markdown_review":metadata.legacy_record,"stored_review":item}),
                );
                let body = if item.updated_at >= metadata.updated_at {
                    metadata.updated_at = item.updated_at.clone();
                    item.body.as_deref().unwrap_or(&proposal.body)
                } else {
                    &proposal.body
                };
                document
                    .set_proposal_details(&item.id, body, metadata)
                    .map_err(document_error)?;
                if item.state == BlockReviewState::Resolved {
                    document
                        .close_imported_proposal(&item.id)
                        .map_err(document_error)?;
                }
            }
            continue;
        }
        let metadata = legacy_metadata(item)?;
        match item.kind {
            BlockReviewKind::Comment => import_comment(document, item, metadata)?,
            BlockReviewKind::Suggestion => import_proposal(document, item, metadata)?,
            BlockReviewKind::Conflict => document
                .add_conflict(Conflict {
                    id: item.id.clone(),
                    after: (!item.block_id.is_empty()).then(|| item.block_id.clone()),
                    base: item.context_before.clone().unwrap_or_default(),
                    incoming: item.body.clone().unwrap_or_default(),
                    canonical: item.quote.clone().unwrap_or_default(),
                    resolved: item.state == BlockReviewState::Resolved,
                    author: item.author.clone().unwrap_or_default(),
                    metadata,
                })
                .map_err(document_error)?,
        }
    }
    Ok(())
}

fn legacy_metadata(item: &BlockReviewItem) -> Result<ReviewMetadata> {
    Ok(ReviewMetadata {
        created_at: item.created_at.clone(),
        updated_at: item.updated_at.clone(),
        unattached_reason: None,
        legacy_record: Some(serde_json::to_value(item).map_err(document_error)?),
    })
}

fn import_comment(
    d: &mut Document,
    item: &BlockReviewItem,
    mut metadata: ReviewMetadata,
) -> Result<()> {
    let author = item.author.as_deref().unwrap_or_default();
    let body = item.body.as_deref().unwrap_or_default();
    if let Some(parent) = &item.parent_item_id {
        if d.reply_comment(&item.id, parent, author, body).is_ok() {
            metadata.unattached_reason = d
                .comment(parent)
                .ok()
                .and_then(|c| c.metadata.unattached_reason);
            return d
                .set_comment_metadata(&item.id, metadata)
                .map_err(document_error);
        }
        metadata.unattached_reason = Some("Legacy reply parent is unavailable".into());
    }
    let attached =
        if metadata.unattached_reason.is_none() && item.state != BlockReviewState::Orphaned {
            match d.selection(
                &item.block_id,
                item.start_offset as usize,
                item.end_offset as usize,
            ) {
                Ok(ranges) if !ranges.is_empty() => {
                    d.add_comment(&item.id, author, body, &ranges)
                        .map_err(document_error)?;
                    true
                }
                _ => false,
            }
        } else {
            false
        };
    if attached {
        d.set_comment_metadata(&item.id, metadata)
            .map_err(document_error)?;
        if item.state == BlockReviewState::Resolved {
            d.resolve_comment(&item.id, true).map_err(document_error)?;
        }
    } else {
        metadata
            .unattached_reason
            .get_or_insert_with(|| "Legacy comment had no verified live range".into());
        d.import_unattached_comment(Comment {
            id: item.id.clone(),
            author: author.into(),
            body: body.into(),
            target: Vec::new(),
            original_quote: item.quote.clone().unwrap_or_default(),
            state: if item.state == BlockReviewState::Resolved {
                DiscussionState::Resolved
            } else {
                DiscussionState::Open
            },
            parent_id: None,
            deleted: false,
            metadata,
        })
        .map_err(document_error)?;
    }
    Ok(())
}

fn import_proposal(
    d: &mut Document,
    item: &BlockReviewItem,
    metadata: ReviewMetadata,
) -> Result<()> {
    let author = item.author.as_deref().unwrap_or_default();
    let imported = if item.state == BlockReviewState::Open {
        if item.is_markdown_insert_suggestion() {
            let rows = quarry_markdown::markdown_to_block_rows(
                item.replacement.as_deref().unwrap_or_default(),
                || uuid::Uuid::new_v4().to_string(),
            )?;
            let seeds: Vec<_> = rows
                .iter()
                .map(|row| SeedBlock {
                    id: row.block_id.clone(),
                    kind: row.block_type.clone(),
                    parent: row.parent_block_id.clone(),
                    position: row.position as usize,
                    attrs: row.attrs.clone().into_iter().collect(),
                    text: row.text.clone(),
                })
                .collect();
            let roots: Vec<_> = d
                .blocks()
                .map_err(document_error)?
                .into_iter()
                .filter(|b| b.parent.is_none())
                .collect();
            let before = if item.block_id.is_empty() {
                roots.first().map(|b| b.id.clone())
            } else {
                roots
                    .iter()
                    .position(|b| b.id == item.block_id)
                    .and_then(|i| roots.get(i + 1).map(|b| b.id.clone()))
            };
            if !item.block_id.is_empty() && !roots.iter().any(|b| b.id == item.block_id) {
                false
            } else {
                let proposed = d
                    .propose_blocks(&item.id, author, None, before, &seeds)
                    .is_ok();
                if proposed {
                    let mut offset = 0;
                    for row in &rows {
                        for mark in &row.marks {
                            let ranges = d
                                .proposal_selection(
                                    &item.id,
                                    offset + mark.start as usize,
                                    offset + mark.end as usize,
                                )
                                .map_err(document_error)?;
                            for (name, value) in &mark.marks {
                                d.format(&ranges, name, value).map_err(document_error)?;
                            }
                        }
                        for link in &row.links {
                            let ranges = d
                                .proposal_selection(
                                    &item.id,
                                    offset + link.start as usize,
                                    offset + link.end as usize,
                                )
                                .map_err(document_error)?;
                            d.format(&ranges, "link", &link.url.clone().into())
                                .map_err(document_error)?;
                        }
                        offset += row.text.encode_utf16().count();
                    }
                }
                proposed
            }
        } else if item.is_block_delete_suggestion() {
            d.propose_block_delete(&item.id, author, &item.block_id)
                .is_ok()
        } else if let (Ok(at), Ok(ranges)) = (
            d.point(&item.block_id, item.start_offset as usize),
            d.selection(
                &item.block_id,
                item.start_offset as usize,
                item.end_offset as usize,
            ),
        ) {
            d.propose_replacement(
                &item.id,
                author,
                &item.block_id,
                &at,
                &ranges,
                item.replacement.as_deref().unwrap_or_default(),
            )
            .is_ok()
        } else {
            false
        }
    } else {
        false
    };
    if imported {
        d.set_proposal_details(&item.id, item.body.as_deref().unwrap_or_default(), metadata)
            .map_err(document_error)?;
    } else {
        d.import_unavailable_proposal(Proposal {
            id: item.id.clone(),
            author: author.into(),
            body: item.body.clone().unwrap_or_default(),
            action: ProposalAction::Unavailable {
                reason: "Legacy proposal has no live native target or was already closed".into(),
            },
            segments: Vec::new(),
            state: if item.state == BlockReviewState::Resolved {
                ProposalState::Closed
            } else {
                ProposalState::Open
            },
            metadata,
        })
        .map_err(document_error)?;
    }
    Ok(())
}

/// Compatibility rows are derived, and may expose only one fragment. The
/// document view remains the complete source for multi-block review targets.
pub fn document_review_projection(d: &Document) -> Result<Vec<BlockReviewItem>> {
    let document_id = d.id().map_err(document_error)?;
    let mut items = Vec::new();
    for comment in d.comments().map_err(document_error)? {
        let target = d.comment_target(&comment.id).map_err(document_error)?;
        let mut item = empty_item(
            &document_id,
            &comment.id,
            BlockReviewKind::Comment,
            &comment.metadata,
        );
        item.author = Some(comment.author);
        item.body = Some(comment.body);
        item.quote = Some(comment.original_quote);
        item.parent_item_id = comment.parent_id;
        if let Some(attachment) = target
            .attachments
            .iter()
            .find(|a| matches!(a.owner, TargetOwner::Block(_)))
        {
            if let TargetOwner::Block(id) = &attachment.owner {
                item.block_id = id.clone();
            }
            item.start_offset = attachment.start.try_into().map_err(document_error)?;
            item.end_offset = attachment.end.try_into().map_err(document_error)?;
        } else {
            item.state = BlockReviewState::Orphaned;
        }
        if comment.state == DiscussionState::Resolved {
            item.state = BlockReviewState::Resolved;
        }
        items.push(item);
    }
    for proposal in d.proposals().map_err(document_error)? {
        let mut item = empty_item(
            &document_id,
            &proposal.id,
            BlockReviewKind::Suggestion,
            &proposal.metadata,
        );
        let target = d.proposal_target(&proposal.id).map_err(document_error)?;
        item.author = Some(proposal.author.clone());
        item.body = Some(proposal.body.clone());
        if let Some(attachment) = target
            .attachments
            .iter()
            .find(|a| matches!(a.owner, TargetOwner::Block(_)))
        {
            if let TargetOwner::Block(id) = &attachment.owner {
                item.block_id = id.clone();
            }
            item.start_offset = attachment.start.try_into().map_err(document_error)?;
            item.end_offset = attachment.end.try_into().map_err(document_error)?;
        }
        match &proposal.action {
            ProposalAction::Text { original_quote, .. } => {
                item.quote = Some(original_quote.clone());
                item.replacement = d
                    .view()
                    .map_err(document_error)?
                    .proposals
                    .into_iter()
                    .find(|p| p.proposal.id == proposal.id)
                    .map(|p| p.text);
                if item.block_id.is_empty() {
                    item.state = BlockReviewState::Orphaned;
                }
            }
            ProposalAction::Format { expected, .. } => {
                let quote: String = expected.iter().map(|(text, _)| text.as_str()).collect();
                item.quote = Some(quote.clone());
                item.replacement = Some(quote);
                if item.block_id.is_empty() {
                    item.state = BlockReviewState::Orphaned;
                }
            }
            ProposalAction::UpdateBlock { .. }
            | ProposalAction::ConvertBlock { .. }
            | ProposalAction::DeleteBlock { .. }
            | ProposalAction::MoveBlock { .. } => {
                if item.block_id.is_empty() {
                    item.state = BlockReviewState::Orphaned;
                }
            }
            ProposalAction::InsertBlocks { parent, before, .. } => {
                let siblings: Vec<_> = d
                    .blocks()
                    .map_err(document_error)?
                    .into_iter()
                    .filter(|b| b.parent == *parent)
                    .collect();
                let index = before
                    .as_ref()
                    .and_then(|id| siblings.iter().position(|b| b.id == *id))
                    .unwrap_or(siblings.len());
                item.block_id = index
                    .checked_sub(1)
                    .and_then(|i| siblings.get(i))
                    .map(|b| b.id.clone())
                    .unwrap_or_default();
                item.context_after = Some(MARKDOWN_INSERT_SUGGESTION_CONTEXT.into());
                item.replacement = Some(quarry_markdown::block_rows_to_markdown(
                    &project_block_views(
                        d.proposed_blocks_view(&proposal.id)
                            .map_err(document_error)?,
                    )?,
                )?);
            }
            ProposalAction::Unavailable { .. } => {
                if let Some(legacy) = proposal.metadata.legacy_record.clone()
                    && let Ok(record) = serde_json::from_value::<BlockReviewItem>(legacy)
                {
                    item = record;
                }
                item.state = BlockReviewState::Invalidated;
            }
        }
        if proposal.state == ProposalState::Open
            && d.validate_proposal_acceptance(&proposal.id).is_err()
        {
            item.state = BlockReviewState::Invalidated;
        } else if proposal.state != ProposalState::Open {
            item.state = BlockReviewState::Resolved;
        }
        items.push(item);
    }
    for conflict in d.conflicts().map_err(document_error)? {
        let mut item = empty_item(
            &document_id,
            &conflict.id,
            BlockReviewKind::Conflict,
            &conflict.metadata,
        );
        item.block_id = conflict.after.unwrap_or_default();
        item.author = Some(conflict.author);
        item.context_before = Some(conflict.base);
        item.body = Some(conflict.incoming);
        item.quote = Some(conflict.canonical);
        if conflict.resolved {
            item.state = BlockReviewState::Resolved;
        }
        items.push(item);
    }
    items.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
    Ok(items)
}

fn empty_item(
    document: &str,
    id: &str,
    kind: BlockReviewKind,
    metadata: &ReviewMetadata,
) -> BlockReviewItem {
    BlockReviewItem {
        id: id.into(),
        document_id: document.into(),
        block_id: String::new(),
        kind,
        start_offset: 0,
        end_offset: 0,
        body: None,
        replacement: None,
        author: None,
        state: BlockReviewState::Open,
        quote: None,
        context_before: None,
        context_after: None,
        parent_item_id: None,
        created_at: metadata.created_at.clone(),
        updated_at: metadata.updated_at.clone(),
    }
}
