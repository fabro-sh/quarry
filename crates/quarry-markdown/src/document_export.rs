//! Markdown is a projection of native content. It never supplies text identity.
use crate::{Attrs, BlockRow, LinkRange, MarkRun, Unsupported};
use quarry_document::Document;
fn unsupported(error: impl std::fmt::Display) -> Unsupported {
    Unsupported::new(error.to_string())
}

pub fn document_to_block_rows(document: &Document) -> Result<Vec<BlockRow>, Unsupported> {
    let views = document
        .blocks()
        .map_err(unsupported)?
        .iter()
        .map(|b| document.block_view(&b.id).map_err(unsupported))
        .collect::<Result<Vec<_>, Unsupported>>()?;
    block_views_to_rows(views)
}

pub fn block_views_to_rows(
    views: Vec<quarry_document::BlockView>,
) -> Result<Vec<BlockRow>, Unsupported> {
    let mut rows = Vec::new();
    for view in views {
        let block = view.block;
        let mut marks: Vec<MarkRun> = Vec::new();
        let mut links: Vec<LinkRange> = Vec::new();
        let mut offset = 0u32;
        for run in view.runs {
            let length = u32::try_from(run.text.encode_utf16().count()).map_err(unsupported)?;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| unsupported("Block is too long"))?;
            let mut formatting: Attrs = run.marks.into_iter().collect();
            if let Some(url) = formatting.shift_remove("link") {
                let url = url
                    .as_str()
                    .ok_or_else(|| unsupported("Link is not a string"))?
                    .to_string();
                if let Some(last) = links.last_mut().filter(|l| l.end == offset && l.url == url) {
                    last.end = end;
                } else {
                    links.push(LinkRange {
                        start: offset,
                        end,
                        url,
                    });
                }
            }
            if !formatting.is_empty() && offset < end {
                if let Some(last) = marks
                    .last_mut()
                    .filter(|m| m.end == offset && m.marks == formatting)
                {
                    last.end = end;
                } else {
                    marks.push(MarkRun {
                        start: offset,
                        end,
                        marks: formatting,
                    });
                }
            }
            offset = end;
        }
        rows.push(BlockRow {
            block_id: block.id,
            parent_block_id: block.parent,
            position: u32::try_from(block.position).map_err(unsupported)?,
            block_type: block.kind,
            attrs: block.attrs.into_iter().collect(),
            text: view.text,
            marks,
            links,
        });
    }
    Ok(rows)
}

pub fn document_to_markdown(document: &Document) -> Result<String, Unsupported> {
    crate::block_rows_to_markdown(&document_to_block_rows(document)?)
}
