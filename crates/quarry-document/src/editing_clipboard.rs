//! Same-document cut transfers retain native text references until paste.
use crate::{BLOCKS, Block, Document, DocumentError, Result, SegmentRef, TargetOwner, TextPoint};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CUT_DATA: &str = "quarry:cut";
const CUT_CONSUMED: &str = "quarry:cut-consumed";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CutPiece {
    kind: String,
    attrs: BTreeMap<String, serde_json::Value>,
    segments: Vec<SegmentRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CutTransfer {
    pieces: Vec<CutPiece>,
}

fn record_id(transfer: &str) -> Result<String> {
    if transfer.is_empty() || transfer.len() > 200 {
        return Err(DocumentError::Invalid("Invalid cut transfer ID".into()));
    }
    Ok(format!("quarry:cut:{transfer}"))
}

impl Document {
    /// Remove one sibling text selection while parking its exact segment
    /// references in a hidden native record. No characters are copied.
    pub fn cut_selection(
        &mut self,
        transfer: &str,
        anchor: &TextPoint,
        focus: &TextPoint,
    ) -> Result<()> {
        self.atomic(|d| {
            let record = record_id(transfer)?;
            d.require_new(BLOCKS, &record)?;
            let missing = || DocumentError::Conflict("Cut selection is no longer visible".into());
            let anchor_location = d.locate_point(anchor)?.ok_or_else(missing)?;
            let focus_location = d.locate_point(focus)?.ok_or_else(missing)?;
            let (TargetOwner::Block(anchor_block), TargetOwner::Block(focus_block)) =
                (&anchor_location.owner, &focus_location.owner)
            else {
                return Err(DocumentError::Invalid(
                    "Cut native text from canonical blocks".into(),
                ));
            };
            let views = d.view()?.blocks;
            let address = |id: &str, offset: usize| -> Result<(usize, usize)> {
                Ok((
                    views
                        .iter()
                        .position(|view| view.block.id == id)
                        .ok_or_else(missing)?,
                    offset,
                ))
            };
            let a = address(anchor_block, anchor_location.offset)?;
            let b = address(focus_block, focus_location.offset)?;
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            if start == end {
                return Err(DocumentError::Invalid("Cut selection is empty".into()));
            }
            let selected = &views[start.0..=end.0];
            if selected.iter().any(|view| {
                view.block.parent != selected[0].block.parent
                    || !crate::carries_inline_content(&view.block.kind)
            }) {
                return Err(DocumentError::Invalid(
                    "Identity-preserving cut requires sibling text blocks".into(),
                ));
            }
            let mut pieces = Vec::new();
            if selected.len() == 1 {
                let view = &selected[0];
                let start_point = d.point_in_segments(&view.block.segments, start.1)?;
                let end_point = d.point_in_segments(&view.block.segments, end.1)?;
                let (prefix, tail) = d.partition_segments(&view.block.segments, &start_point)?;
                let (segments, suffix) = d.partition_segments(&tail, &end_point)?;
                let mut block = view.block.clone();
                block.segments = prefix.into_iter().chain(suffix).collect();
                d.put_record(BLOCKS, &block.id, &block)?;
                pieces.push(CutPiece {
                    kind: block.kind,
                    attrs: block.attrs,
                    segments,
                });
            } else {
                let first = &selected[0];
                let start_point = d.point_in_segments(&first.block.segments, start.1)?;
                let (prefix, first_segments) =
                    d.partition_segments(&first.block.segments, &start_point)?;
                pieces.push(CutPiece {
                    kind: first.block.kind.clone(),
                    attrs: first.block.attrs.clone(),
                    segments: first_segments,
                });
                for view in &selected[1..selected.len() - 1] {
                    pieces.push(CutPiece {
                        kind: view.block.kind.clone(),
                        attrs: view.block.attrs.clone(),
                        segments: view.block.segments.clone(),
                    });
                    let mut block = view.block.clone();
                    block.deleted = true;
                    d.put_record(BLOCKS, &block.id, &block)?;
                }
                let last = &selected[selected.len() - 1];
                let end_point = d.point_in_segments(&last.block.segments, end.1)?;
                let (last_segments, suffix) =
                    d.partition_segments(&last.block.segments, &end_point)?;
                pieces.push(CutPiece {
                    kind: last.block.kind.clone(),
                    attrs: last.block.attrs.clone(),
                    segments: last_segments,
                });
                let mut first_block = first.block.clone();
                first_block.segments = prefix.into_iter().chain(suffix).collect();
                d.put_record(BLOCKS, &first_block.id, &first_block)?;
                let mut last_block = last.block.clone();
                last_block.deleted = true;
                d.put_record(BLOCKS, &last_block.id, &last_block)?;
            }
            if pieces.is_empty() {
                return Err(DocumentError::Invalid("Cut selection is empty".into()));
            }
            let mut attrs = BTreeMap::new();
            attrs.insert(
                CUT_DATA.into(),
                serde_json::to_value(CutTransfer { pieces })?,
            );
            attrs.insert(CUT_CONSUMED.into(), serde_json::Value::Bool(false));
            d.put_record(
                BLOCKS,
                &record,
                &Block {
                    id: record.clone(),
                    kind: "raw_markdown".into(),
                    attrs,
                    parent: None,
                    position: 0,
                    segments: Vec::new(),
                    deleted: true,
                },
            )?;
            Ok(())
        })
    }

    /// Consume a cut transfer at a canonical text point. The first piece
    /// merges into the destination block. Later pieces become sibling blocks,
    /// matching Slate fragment insertion while retaining every text source.
    pub fn paste_cut(&mut self, transfer: &str, at: &TextPoint, new_block: &str) -> Result<()> {
        self.atomic(|d| {
            let record_id = record_id(transfer)?;
            let mut record = d.block(&record_id).map_err(|_| {
                DocumentError::Conflict("Cut transfer is unavailable or already consumed".into())
            })?;
            if record.attrs.get(CUT_CONSUMED) == Some(&serde_json::Value::Bool(true)) {
                return Err(DocumentError::Conflict(
                    "Cut transfer is unavailable or already consumed".into(),
                ));
            }
            let transfer: CutTransfer = serde_json::from_value(
                record
                    .attrs
                    .get(CUT_DATA)
                    .cloned()
                    .ok_or_else(|| DocumentError::Invalid("Invalid cut transfer".into()))?,
            )?;
            if transfer.pieces.is_empty() {
                return Err(DocumentError::Invalid("Invalid cut transfer".into()));
            }
            for piece in &transfer.pieces {
                if !crate::carries_inline_content(&piece.kind) || piece.segments.is_empty() {
                    return Err(DocumentError::Invalid("Invalid cut transfer".into()));
                }
                for segment in &piece.segments {
                    d.segment(segment)?;
                }
            }
            let location = d.locate_point(at)?.ok_or_else(|| {
                DocumentError::Conflict("Paste destination is no longer visible".into())
            })?;
            let TargetOwner::Block(destination) = location.owner else {
                return Err(DocumentError::Invalid(
                    "Paste cut text into a canonical block".into(),
                ));
            };
            let mut destination = d.active_block(&destination)?;
            if !crate::carries_inline_content(&destination.kind) {
                return Err(DocumentError::Invalid(
                    "Paste cut text into a text block".into(),
                ));
            }
            let (mut prefix, suffix) = d.partition_segments(&destination.segments, at)?;
            prefix.extend(transfer.pieces[0].segments.clone());
            destination.segments = prefix;

            if transfer.pieces.len() == 1 {
                destination.segments.extend(suffix);
                d.put_record(BLOCKS, &destination.id, &destination)?;
            } else {
                if new_block.is_empty() {
                    return Err(DocumentError::Invalid(
                        "Missing pasted block identity".into(),
                    ));
                }
                let count = transfer.pieces.len() - 1;
                let ids = (0..count)
                    .map(|index| {
                        if index == 0 {
                            new_block.to_string()
                        } else {
                            format!("{new_block}:{index}")
                        }
                    })
                    .collect::<Vec<_>>();
                for id in &ids {
                    d.require_new(BLOCKS, id)?;
                }
                for mut sibling in d.blocks()? {
                    if sibling.parent == destination.parent
                        && sibling.position > destination.position
                    {
                        sibling.position += count;
                        d.put_record(BLOCKS, &sibling.id, &sibling)?;
                    }
                }
                d.put_record(BLOCKS, &destination.id, &destination)?;
                for (index, piece) in transfer.pieces[1..].iter().enumerate() {
                    let mut segments = piece.segments.clone();
                    if index + 1 == count {
                        segments.extend(suffix.clone());
                    }
                    let block = Block {
                        id: ids[index].clone(),
                        kind: piece.kind.clone(),
                        attrs: crate::schema::normalize_attrs(&piece.kind, piece.attrs.clone())?,
                        parent: destination.parent.clone(),
                        position: destination.position + index + 1,
                        segments,
                        deleted: false,
                    };
                    d.put_record(BLOCKS, &block.id, &block)?;
                }
            }
            record
                .attrs
                .insert(CUT_CONSUMED.into(), serde_json::Value::Bool(true));
            d.put_record(BLOCKS, &record_id, &record)
        })
    }
}
