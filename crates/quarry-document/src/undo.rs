//! Undo reverses one observed transition. It retains characters and rejects
//! conflicting later decisions instead of overwriting work the caller did not see.
use crate::{
    BLOCKS, Block, COMMENTS, CONFLICTS, Document, DocumentError, PROPOSALS, Result, SOURCES,
};
use automerge::{
    ChangeHash, ReadDoc, ScalarValue,
    iter::Span,
    marks::{ExpandMark, Mark},
    transaction::Transactable,
};
use std::collections::{BTreeMap, BTreeSet};

struct Character {
    offset: usize,
    width: usize,
    marks: BTreeMap<String, ScalarValue>,
}

fn characters(document: &Document, source: &str) -> Result<BTreeMap<String, Character>> {
    let mut result = BTreeMap::new();
    if document.source(source).is_err() {
        return Ok(result);
    }
    let mut offset = 0;
    for span in document.crdt.spans(document.source(source)?)? {
        match span {
            Span::Block(_) => offset += 1,
            Span::Text { text, marks } => {
                let marks: BTreeMap<_, _> = marks
                    .map(|m| m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
                    .unwrap_or_default();
                for ch in text.chars() {
                    let cursor = document.point_at(source, offset)?.cursor;
                    result.insert(
                        cursor,
                        Character {
                            offset,
                            width: ch.len_utf16(),
                            marks: marks.clone(),
                        },
                    );
                    offset += ch.len_utf16();
                }
            }
        }
    }
    Ok(result)
}

impl Document {
    /// Reverse the transition from `before` to `after` on current state. The
    /// reverse transition can itself be reversed for redo, including after reload.
    pub fn revert(&mut self, before: &[ChangeHash], after: &[ChangeHash]) -> Result<()> {
        if before.is_empty() || after.is_empty() {
            return Err(DocumentError::Invalid(
                "Undo requires two saved versions".into(),
            ));
        }
        let prior = self.fork_at(before)?;
        let next = self.fork_at(after)?;
        next.fork_at(before)?;
        self.atomic(|current| {
            for collection in [BLOCKS, COMMENTS, PROPOSALS, CONFLICTS] {
                let keys: BTreeSet<_> = prior
                    .crdt
                    .keys(prior.root_map(collection)?)
                    .chain(next.crdt.keys(next.root_map(collection)?))
                    .collect();
                for id in keys {
                    let before = prior.record::<serde_json::Value>(collection, &id).ok();
                    let after = next.record::<serde_json::Value>(collection, &id).ok();
                    let (before, after) = if collection == BLOCKS {
                        (
                            current.expand_block_record(&prior, before)?,
                            current.expand_block_record(&next, after)?,
                        )
                    } else if collection == PROPOSALS {
                        (
                            current.expand_proposal_record(&prior, before)?,
                            current.expand_proposal_record(&next, after)?,
                        )
                    } else {
                        (before, after)
                    };
                    if before == after {
                        continue;
                    }
                    let actual = current.record::<serde_json::Value>(collection, &id).ok();
                    if actual != after {
                        return Err(DocumentError::Conflict(format!(
                            "Cannot undo: {collection}/{id} changed since this edit"
                        )));
                    }
                    if let Some(before) = before {
                        current.put_record(collection, &id, &before)?;
                    } else if let Some(mut record) = after {
                        match collection {
                            BLOCKS | COMMENTS => record["deleted"] = true.into(),
                            PROPOSALS => record["state"] = "rejected".into(),
                            CONFLICTS => record["resolved"] = true.into(),
                            _ => unreachable!(),
                        }
                        current.put_record(collection, &id, &record)?;
                    }
                }
            }
            let deletion = format!("quarry:deleted:undo:{}", current.fresh_id());
            let sources: Vec<_> = next.crdt.keys(next.root_map(SOURCES)?).collect();
            let mut ordinal = 0;
            for source in sources {
                let before = characters(&prior, &source)?;
                let after = characters(&next, &source)?;
                let present = characters(current, &source)?;
                let object = current.source(&source)?;
                current.invalidate_source(&source);
                for (cursor, added) in &after {
                    let actual = present.get(cursor).ok_or_else(|| {
                        DocumentError::Conflict("Undo text is unavailable".into())
                    })?;
                    if let Some(previous) = before.get(cursor) {
                        let names: BTreeSet<_> =
                            previous.marks.keys().chain(added.marks.keys()).collect();
                        for name in names {
                            let old_value = previous.marks.get(name).unwrap_or(&ScalarValue::Null);
                            let new_value = added.marks.get(name).unwrap_or(&ScalarValue::Null);
                            if old_value == new_value {
                                continue;
                            }
                            if actual.marks.get(name).unwrap_or(&ScalarValue::Null) != new_value {
                                return Err(DocumentError::Conflict(
                                    "Cannot undo formatting changed by a later edit".into(),
                                ));
                            }
                            current.crdt.mark(
                                &object,
                                Mark::new(
                                    name.clone(),
                                    old_value.clone(),
                                    actual.offset,
                                    actual.offset + actual.width,
                                ),
                                ExpandMark::None,
                            )?;
                        }
                    } else {
                        current.crdt.mark(
                            &object,
                            Mark::new(
                                format!("{deletion}:{ordinal}"),
                                cursor.as_str(),
                                actual.offset,
                                actual.offset + actual.width,
                            ),
                            ExpandMark::None,
                        )?;
                        ordinal += 1;
                    }
                }
            }
            Ok(())
        })
    }

    /// A split adds a marker inside a formerly whole segment. Restoring an
    /// older owner must include those partitions, never copy their characters.
    fn expand_block_record(
        &self,
        version: &Self,
        record: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let Some(record) = record else {
            return Ok(None);
        };
        let mut block: Block = serde_json::from_value(record)?;
        block.segments = self.expand_segment_refs(version, &block.segments)?;
        Ok(Some(serde_json::to_value(block)?))
    }

    fn expand_proposal_record(
        &self,
        version: &Self,
        record: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let Some(record) = record else {
            return Ok(None);
        };
        let mut proposal: crate::Proposal = serde_json::from_value(record)?;
        proposal.segments = self.expand_segment_refs(version, &proposal.segments)?;
        if let crate::ProposalAction::InsertBlocks { blocks, .. }
        | crate::ProposalAction::PasteBlocks { blocks, .. } = &mut proposal.action
        {
            for block in blocks {
                block.segments = self.expand_segment_refs(version, &block.segments)?;
            }
        }
        Ok(Some(serde_json::to_value(proposal)?))
    }

    fn expand_segment_refs(
        &self,
        version: &Self,
        references: &[crate::SegmentRef],
    ) -> Result<Vec<crate::SegmentRef>> {
        let mut refs = Vec::new();
        for reference in references {
            let old = version.source_segments(&reference.source)?;
            let start = old
                .iter()
                .position(|s| s.reference == *reference)
                .ok_or_else(|| DocumentError::Invalid("Undo segment is missing".into()))?;
            let next = old.get(start + 1).map(|s| &s.reference.segment);
            let mut include = false;
            for segment in self.source_segments(&reference.source)?.iter() {
                if next == Some(&segment.reference.segment) {
                    break;
                }
                if segment.reference == *reference {
                    include = true;
                }
                if include {
                    refs.push(segment.reference.clone());
                }
            }
        }
        Ok(refs)
    }
}
