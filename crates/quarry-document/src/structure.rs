use crate::{
    BLOCKS, Block, COMMENTS, Comment, Document, DocumentError, PROPOSALS, Proposal, ProposalAction,
    ProposalState, Result, SCHEMA_VERSION, SOURCES, SeedBlock, SegmentRef, TextPoint,
};
use automerge::{ObjType, ROOT, ReadDoc, ScalarValue, Value, transaction::Transactable};
use std::collections::{BTreeMap, BTreeSet};

impl Document {
    pub(crate) fn create_source(&mut self, text: &str) -> Result<SegmentRef> {
        let reference = SegmentRef {
            source: self.fresh_id(),
            segment: self.fresh_id(),
        };
        self.invalidate_source(&reference.source);
        let source =
            self.crdt
                .put_object(self.root_map(SOURCES)?, &reference.source, ObjType::Text)?;
        let marker = self.crdt.insert_object(&source, 0, ObjType::Map)?;
        self.crdt
            .put(marker, "segment", reference.segment.as_str())?;
        self.crdt.splice_text(source, 1, 0, text)?;
        Ok(reference)
    }

    pub fn insert_block(&mut self, seed: SeedBlock) -> Result<()> {
        self.atomic(|d| {
            d.require_new(BLOCKS, &seed.id)?;
            for mut sibling in d.blocks()? {
                if sibling.parent == seed.parent && sibling.position >= seed.position {
                    sibling.position += 1;
                    d.put_record(BLOCKS, &sibling.id, &sibling)?;
                }
            }
            let segment = d.create_source(&seed.text)?;
            let block = Block {
                id: seed.id,
                kind: seed.kind.clone(),
                attrs: crate::schema::normalize_attrs(&seed.kind, seed.attrs)?,
                parent: seed.parent,
                position: seed.position,
                segments: vec![segment],
                deleted: false,
            };
            d.put_record(BLOCKS, &block.id, &block)?;
            Ok(())
        })
    }

    pub fn set_block(
        &mut self,
        id: &str,
        kind: &str,
        attrs: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut block = d.active_block(id)?;
            if block.kind != kind && (block.kind == "raw_markdown" || kind == "raw_markdown") {
                return Err(DocumentError::Invalid(
                    "Replace a raw Markdown block explicitly to change its content model".into(),
                ));
            }
            block.kind = kind.into();
            block.attrs = crate::schema::normalize_attrs(kind, attrs)?;
            d.put_record(BLOCKS, id, &block)
        })
    }

    pub(crate) fn active_block(&self, id: &str) -> Result<Block> {
        let block = self.block(id)?;
        if block.deleted {
            return Err(DocumentError::Conflict(format!("Block was deleted: {id}")));
        }
        Ok(block)
    }

    pub(crate) fn validate_block_move(
        &self,
        id: &str,
        parent: Option<&str>,
        before: Option<&str>,
    ) -> Result<()> {
        let block = self.active_block(id)?;
        let mut ancestor = parent.map(str::to_owned);
        let mut visited = BTreeSet::from([id.to_string()]);
        while let Some(parent_id) = ancestor {
            if !visited.insert(parent_id.clone()) {
                return Err(DocumentError::Conflict(
                    "Move would create a block parent cycle".into(),
                ));
            }
            ancestor = self.active_block(&parent_id)?.parent;
        }
        if let Some(before) = before
            && self.active_block(before)?.parent.as_deref() != parent
        {
            return Err(DocumentError::Conflict(
                "Move destination is not a sibling".into(),
            ));
        }
        let parent_kind = parent
            .map(|id| self.active_block(id).map(|block| block.kind))
            .transpose()?;
        crate::schema::validate_parent(&block.kind, parent_kind.as_deref())?;
        Ok(())
    }

    pub fn move_block(
        &mut self,
        id: &str,
        parent: Option<String>,
        before: Option<&str>,
    ) -> Result<()> {
        self.atomic(|d| {
            d.validate_block_move(id, parent.as_deref(), before)?;
            let mut block = d.active_block(id)?;
            if before == Some(id) {
                return Ok(());
            }
            let mut siblings: Vec<_> = d
                .blocks()?
                .into_iter()
                .filter(|b| b.parent == parent && b.id != id)
                .collect();
            let index = match before {
                Some(before) => siblings
                    .iter()
                    .position(|b| b.id == before)
                    .ok_or_else(|| {
                        DocumentError::Conflict("Move destination is not a sibling".into())
                    })?,
                None => siblings.len(),
            };
            block.parent = parent;
            siblings.insert(index, block);
            for (position, mut sibling) in siblings.into_iter().enumerate() {
                sibling.position = position;
                d.put_record(BLOCKS, &sibling.id, &sibling)?;
            }
            Ok(())
        })
    }

    /// Split at an address from the caller's version. If an earlier split
    /// moved that address out of this block, report a conflict without mutation.
    pub fn split_block(&mut self, id: &str, at: &TextPoint, new_block_id: &str) -> Result<()> {
        self.atomic(|d| {
            d.require_new(BLOCKS, new_block_id)?;
            let mut block = d.active_block(id)?;
            let (prefix, suffix) = d.partition_segments(&block.segments, at)?;
            let mut next = block.clone();
            block.segments = prefix;
            next.id = new_block_id.into();
            next.segments = suffix;
            next.position = block.position + 1;
            for mut sibling in d.blocks()? {
                if sibling.parent == block.parent && sibling.position > block.position {
                    sibling.position += 1;
                    d.put_record(BLOCKS, &sibling.id, &sibling)?;
                }
            }
            d.put_record(BLOCKS, &block.id, &block)?;
            d.put_record(BLOCKS, &next.id, &next)?;
            Ok(())
        })
    }

    pub(crate) fn partition_segments(
        &mut self,
        references: &[SegmentRef],
        at: &TextPoint,
    ) -> Result<(Vec<SegmentRef>, Vec<SegmentRef>)> {
        let offset = self.resolve_point(at)?;
        for (index, reference) in references.iter().enumerate() {
            if reference.source != at.source {
                continue;
            }
            let segment = self.segment(reference)?;
            if offset < segment.start || offset > segment.end {
                continue;
            }
            let next = SegmentRef {
                source: reference.source.clone(),
                segment: self.fresh_id(),
            };
            self.invalidate_source(&at.source);
            let marker = self
                .crdt
                .insert_object(self.source(&at.source)?, offset, ObjType::Map)?;
            self.crdt.put(marker, "segment", next.segment.as_str())?;
            let prefix = references[..=index].to_vec();
            let mut suffix = vec![next];
            suffix.extend_from_slice(&references[index + 1..]);
            return Ok((prefix, suffix));
        }
        Err(DocumentError::Conflict(
            "Split position no longer belongs to the block".into(),
        ))
    }

    pub fn join_blocks(&mut self, left: &str, right: &str) -> Result<()> {
        self.atomic(|d| {
            if left == right {
                return Err(DocumentError::Invalid(
                    "Cannot join a block to itself".into(),
                ));
            }
            let mut left = d.active_block(left)?;
            let mut right = d.active_block(right)?;
            let siblings: Vec<_> = d
                .blocks()?
                .into_iter()
                .filter(|b| b.parent == left.parent)
                .map(|b| b.id)
                .collect();
            if !siblings
                .windows(2)
                .any(|p| p == [left.id.as_str(), right.id.as_str()])
            {
                return Err(DocumentError::Conflict(
                    "Join requires adjacent sibling blocks".into(),
                ));
            }
            if d.blocks()?
                .iter()
                .any(|b| b.parent.as_deref() == Some(&right.id))
            {
                return Err(DocumentError::Conflict(
                    "Cannot join a block with children".into(),
                ));
            }
            left.segments.append(&mut right.segments);
            right.deleted = true;
            d.put_record(BLOCKS, &left.id, &left)?;
            d.put_record(BLOCKS, &right.id, &right)?;
            Ok(())
        })
    }

    pub fn delete_block(&mut self, id: &str) -> Result<()> {
        self.atomic(|d| {
            d.active_block(id)?;
            let blocks = d.blocks()?;
            let mut removed = BTreeSet::from([id.to_string()]);
            loop {
                let prior = removed.len();
                for b in &blocks {
                    if b.parent.as_ref().is_some_and(|p| removed.contains(p)) {
                        removed.insert(b.id.clone());
                    }
                }
                if prior == removed.len() {
                    break;
                }
            }
            for mut b in blocks {
                if removed.contains(&b.id) {
                    b.deleted = true;
                    d.put_record(BLOCKS, &b.id, &b)?;
                }
            }
            Ok(())
        })
    }

    pub fn validate(&self) -> Result<()> {
        self.id()?;
        if self.crdt.get(ROOT, "schema_version")?.map(|v| v.0)
            != Some(Value::Scalar(std::borrow::Cow::Owned(ScalarValue::Uint(
                SCHEMA_VERSION,
            ))))
        {
            return Err(DocumentError::Invalid("Unsupported document schema".into()));
        }
        for name in [SOURCES, BLOCKS, COMMENTS, PROPOSALS, crate::CONFLICTS] {
            self.root_map(name)?;
        }
        let mut segment_ids = BTreeSet::new();
        let mut segments_by_id = BTreeMap::new();
        // Hold shared source snapshots for this complete validation pass. Every
        // source is checked, without copying all unchanged runs per keypress.
        let sources = self
            .crdt
            .keys(self.root_map(SOURCES)?)
            .map(|source| self.source_segments(&source))
            .collect::<Result<Vec<_>>>()?;
        for segments in &sources {
            if segments.is_empty() {
                return Err(DocumentError::Invalid(
                    "Source has no segment marker".into(),
                ));
            }
            for segment in segments.iter() {
                if !segment_ids.insert(&segment.reference.segment) {
                    return Err(DocumentError::Invalid("Duplicate segment identity".into()));
                }
                segments_by_id.insert(
                    (&segment.reference.source, &segment.reference.segment),
                    segment,
                );
            }
        }
        let checked_segment = |reference: &crate::SegmentRef| {
            segments_by_id
                .get(&(&reference.source, &reference.segment))
                .copied()
                .ok_or_else(|| DocumentError::NotFound {
                    kind: "segment",
                    id: reference.segment.clone(),
                })
        };
        let has_text = |block: &crate::Block| -> Result<bool> {
            let mut present = false;
            for reference in &block.segments {
                // Check every reference, including ones after non-empty text.
                present |= checked_segment(reference)?
                    .runs
                    .iter()
                    .any(|run| !run.text.is_empty());
            }
            Ok(present)
        };
        let cached = self.cached_blocks()?;
        let blocks = &cached.ordered;
        let by_id: BTreeMap<_, _> = blocks.iter().map(|b| (&b.id, b)).collect();
        let mut owners = BTreeSet::new();
        let mut placements = BTreeSet::new();
        for block in blocks {
            crate::schema::validate_block(block, has_text(block)?)?;
            crate::schema::validate_parent(
                &block.kind,
                block
                    .parent
                    .as_ref()
                    .and_then(|id| by_id.get(id).map(|parent| parent.kind.as_str())),
            )?;
            if block.id.is_empty() || block.kind.is_empty() {
                return Err(DocumentError::Invalid("Empty block ID or kind".into()));
            }
            if !placements.insert((&block.parent, block.position)) {
                return Err(DocumentError::Invalid("Duplicate block position".into()));
            }
            if block.segments.is_empty() {
                return Err(DocumentError::Invalid("Active block has no segment".into()));
            }
            let mut path = BTreeSet::from([&block.id]);
            let mut parent = block.parent.as_ref();
            while let Some(id) = parent {
                if !path.insert(id) {
                    return Err(DocumentError::Invalid("Block parent cycle".into()));
                }
                parent = by_id
                    .get(id)
                    .ok_or_else(|| DocumentError::Invalid("Missing parent block".into()))?
                    .parent
                    .as_ref();
            }
            for segment in &block.segments {
                checked_segment(segment)?;
                if !owners.insert(segment.segment.as_str()) {
                    return Err(DocumentError::Conflict(
                        "A text segment has multiple owners".into(),
                    ));
                }
            }
        }
        let proposals = self.records::<Proposal>(PROPOSALS)?;
        for proposal in &proposals {
            if let ProposalAction::InsertBlocks { parent, blocks, .. } = &proposal.action {
                for view in blocks {
                    crate::schema::validate_block(view, has_text(view)?)?;
                    let parent_kind = match &view.parent {
                        Some(id) => blocks
                            .iter()
                            .find(|block| &block.id == id)
                            .map(|block| block.kind.clone()),
                        // Placement is checked when proposing and accepting. Historical
                        // proposals must remain readable after their destination changes.
                        None if parent.is_some() => continue,
                        None => None,
                    };
                    crate::schema::validate_parent(&view.kind, parent_kind.as_deref())?;
                }
            }
            if let ProposalAction::PasteBlocks {
                block,
                at,
                delete_target,
                delete_ranges,
                delete_blocks,
                joins,
                blocks,
                ..
            } = &proposal.action
            {
                self.block(block)?;
                self.resolve_point(at)?;
                for target in delete_target {
                    self.target_offsets(target)?;
                }
                for range in delete_ranges {
                    self.resolve_point(&TextPoint {
                        source: range.source.clone(),
                        cursor: range.start.clone(),
                    })?;
                    self.resolve_point(&TextPoint {
                        source: range.source.clone(),
                        cursor: range.end.clone(),
                    })?;
                }
                if delete_blocks.iter().any(|deleted| deleted.block.is_empty())
                    || joins
                        .iter()
                        .any(|join| join.left.is_empty() || join.right.is_empty())
                {
                    return Err(DocumentError::Invalid(
                        "Structural paste proposal has an invalid removal plan".into(),
                    ));
                }
                if blocks.len() < 2 {
                    return Err(DocumentError::Invalid(
                        "Structural paste proposal has fewer than two blocks".into(),
                    ));
                }
                let mut ids = BTreeSet::new();
                for pasted in blocks {
                    crate::schema::validate_block(pasted, has_text(pasted)?)?;
                    if pasted.parent.is_some()
                        || !ids.insert(&pasted.id)
                        || !crate::carries_inline_content(&pasted.kind)
                    {
                        return Err(DocumentError::Invalid(
                            "Invalid structural paste block".into(),
                        ));
                    }
                }
            }
            if let ProposalAction::Text {
                at, delete_target, ..
            } = &proposal.action
            {
                self.resolve_point(at)?;
                for target in delete_target {
                    self.target_offsets(target)?;
                }
            }
            if let ProposalAction::Format {
                target,
                name,
                value,
                ..
            } = &proposal.action
            {
                crate::schema::validate_format(name, value)?;
                if target.is_empty() {
                    return Err(DocumentError::Invalid(
                        "Formatting proposal has no target".into(),
                    ));
                }
                for fragment in target {
                    self.target_offsets(fragment)?;
                }
            }
            if let ProposalAction::UpdateBlock {
                block,
                block_kind: kind,
                attrs,
                expected_kind,
                expected_attrs,
            } = &proposal.action
            {
                self.block(block)?;
                crate::schema::normalize_attrs(kind, attrs.clone())?;
                crate::schema::normalize_attrs(expected_kind, expected_attrs.clone())?;
            }
            if let ProposalAction::MoveBlock { block, .. } = &proposal.action {
                self.block(block)?;
            }
            if let ProposalAction::SplitBlock {
                block,
                at,
                new_block,
            } = &proposal.action
            {
                self.block(block)?;
                self.resolve_point(at)?;
                if new_block.is_empty() {
                    return Err(DocumentError::Invalid(
                        "Split proposal has an empty block ID".into(),
                    ));
                }
            }
            if let ProposalAction::JoinBlocks { left, right } = &proposal.action {
                self.block(left)?;
                self.block(right)?;
            }
            if let ProposalAction::ConvertBlock {
                block,
                target,
                expected,
            } = &proposal.action
            {
                self.block(block)?;
                let previous = expected.iter().find(|b| &b.id == block).ok_or_else(|| {
                    DocumentError::Invalid("Conversion proposal has no original block".into())
                })?;
                target.attributes(previous)?;
            }
            for segment in &proposal.segments {
                checked_segment(segment)?;
                if proposal.state != ProposalState::Accepted
                    && !owners.insert(segment.segment.as_str())
                {
                    return Err(DocumentError::Conflict(
                        "Proposal text has multiple owners".into(),
                    ));
                }
            }
        }
        for comment in self.records::<Comment>(COMMENTS)? {
            if comment.target.is_empty()
                && comment.parent_id.is_none()
                && comment.metadata.unattached_reason.is_none()
            {
                return Err(DocumentError::Invalid(
                    "Root comment has no native target or import reason".into(),
                ));
            }
            for target in comment.target {
                self.target_offsets(&target)?;
            }
            if let Some(parent) = &comment.parent_id {
                if let Ok(root) = self.comment(parent) {
                    if root.parent_id.is_some() || root.id == comment.id {
                        return Err(DocumentError::Invalid("Invalid comment parent".into()));
                    }
                } else if self.proposal(parent).is_err() && self.conflict(parent).is_err() {
                    return Err(DocumentError::Invalid("Missing review parent".into()));
                }
            }
        }
        // The cache retains native map keys, including tombstones. Borrowing
        // these records checks the same invariant without copying every block.
        for (id, block) in &cached.by_id {
            if &block.id != id {
                return Err(DocumentError::Invalid(
                    "Block ID does not match its key".into(),
                ));
            }
        }
        let mut review_ids = BTreeSet::new();
        for collection in [COMMENTS, PROPOSALS, crate::CONFLICTS] {
            for id in self.crdt.keys(self.root_map(collection)?) {
                if !review_ids.insert(id) {
                    return Err(DocumentError::Invalid("Repeated review ID".into()));
                }
            }
        }
        for id in self.crdt.keys(self.root_map(COMMENTS)?) {
            if self.comment(&id)?.id != id {
                return Err(DocumentError::Invalid(
                    "Comment ID does not match its key".into(),
                ));
            }
        }
        for id in self.crdt.keys(self.root_map(PROPOSALS)?) {
            if self.proposal(&id)?.id != id {
                return Err(DocumentError::Invalid(
                    "Proposal ID does not match its key".into(),
                ));
            }
        }
        for id in self.crdt.keys(self.root_map(crate::CONFLICTS)?) {
            if self.conflict(&id)?.id != id {
                return Err(DocumentError::Invalid(
                    "Conflict ID does not match its key".into(),
                ));
            }
        }
        Ok(())
    }
}
