//! User-level block conversion. Raw SetBlock remains a strict property write.
//! Both native and WASM callers use these rules, including proposal acceptance.
use crate::{
    BLOCKS, Block, Document, DocumentError, PROPOSALS, Proposal, ProposalAction, ProposalState,
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListFormat {
    pub style: String,
    pub indent: Option<u64>,
    pub checked: Option<bool>,
    pub start: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BlockConversion {
    pub kind: String,
    /// None means a non-list block, including plain p -> p conversions.
    pub list: Option<ListFormat>,
}

impl BlockConversion {
    pub fn plain(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            list: None,
        }
    }

    pub(crate) fn attributes(&self, previous: &Block) -> Result<BTreeMap<String, Value>> {
        if crate::block_capabilities(&self.kind).is_none() {
            return Err(DocumentError::Invalid(format!(
                "Unknown block type: {}",
                self.kind
            )));
        }
        if previous.kind != self.kind
            && (previous.kind == "raw_markdown" || self.kind == "raw_markdown")
        {
            return Err(DocumentError::Invalid(
                "Replace a raw Markdown block explicitly to change its content model".into(),
            ));
        }
        let mut attrs = previous.attrs.clone();
        let was_list = attrs.contains_key("listStyleType");
        for name in [
            "listStyleType",
            "checked",
            "listStart",
            "listRestart",
            "listRestartPolite",
        ] {
            attrs.remove(name);
        }
        if was_list {
            attrs.remove("indent");
        }
        if previous.kind == "code_block" && self.kind != "code_block" {
            attrs.remove("lang");
        }
        if let Some(list) = &self.list {
            if self.kind != "p" {
                return Err(DocumentError::Invalid(
                    "Only paragraphs can be list items".into(),
                ));
            }
            attrs.insert("listStyleType".into(), list.style.clone().into());
            let indent = list
                .indent
                .or_else(|| previous.attrs.get("indent").and_then(Value::as_u64))
                .unwrap_or(1);
            attrs.insert("indent".into(), indent.into());
            if let Some(checked) = list.checked {
                attrs.insert("checked".into(), checked.into());
            }
            if let Some(start) = list.start {
                attrs.insert("listStart".into(), start.into());
            }
        }
        crate::schema::normalize_attrs(&self.kind, attrs)
    }
}

impl Document {
    pub(crate) fn conversion_block(&self, id: &str, proposal: Option<&str>) -> Result<Block> {
        match proposal {
            None => self.active_block(id),
            Some(proposal) => self
                .proposed_blocks_view(proposal)?
                .into_iter()
                .find(|v| v.block.id == id)
                .map(|v| v.block)
                .ok_or_else(|| DocumentError::NotFound {
                    kind: "proposed block",
                    id: id.into(),
                }),
        }
    }

    pub(crate) fn conversion_snapshot(&self, id: &str) -> Result<Vec<Block>> {
        let block = self.active_block(id)?;
        let root = if block.kind == "code_line" {
            block.parent.as_deref().unwrap_or(id)
        } else {
            id
        };
        Ok(self
            .blocks()?
            .into_iter()
            .filter(|b| b.id == root || b.parent.as_deref() == Some(root))
            .collect())
    }

    pub fn convert_block(&mut self, id: &str, target: &BlockConversion) -> Result<()> {
        self.atomic(|d| {
            let original = d.blocks()?;
            let mut blocks = original.clone();
            d.convert_tree(&mut blocks, id, target)?;
            for block in &blocks {
                if original.iter().find(|old| old.id == block.id) != Some(block) {
                    d.put_record(BLOCKS, &block.id, block)?;
                }
            }
            Ok(())
        })
    }

    pub fn convert_proposed_block(
        &mut self,
        proposal_id: &str,
        id: &str,
        target: &BlockConversion,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(proposal_id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            let ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action else {
                return Err(DocumentError::Invalid(
                    "Proposal does not contain blocks".into(),
                ));
            };
            d.convert_tree(blocks, id, target)?;
            blocks.retain(|b| !b.deleted);
            // Projection and acceptance consume parents before their children.
            let mut ordered = Vec::new();
            fn visit(blocks: &[Block], parent: Option<&str>, ordered: &mut Vec<Block>) {
                let mut children: Vec<_> = blocks
                    .iter()
                    .filter(|b| b.parent.as_deref() == parent)
                    .collect();
                children.sort_by_key(|b| b.position);
                for block in children {
                    ordered.push(block.clone());
                    visit(blocks, Some(&block.id), ordered);
                }
            }
            visit(blocks, None, &mut ordered);
            if ordered.len() != blocks.len() {
                return Err(DocumentError::Invalid(
                    "Conversion produced an invalid proposed tree".into(),
                ));
            }
            *blocks = ordered;
            proposal.segments = blocks.iter().flat_map(|b| b.segments.clone()).collect();
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, proposal_id, &proposal)
        })
    }

    pub fn propose_block_conversion(
        &mut self,
        id: &str,
        author: &str,
        block: &str,
        target: &BlockConversion,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let mut preview = d.fork();
            preview.convert_block(block, target)?;
            if preview.blocks()? == d.blocks()? {
                return Ok(());
            }
            let proposal = Proposal {
                id: id.into(),
                author: author.into(),
                body: String::new(),
                segments: Vec::new(),
                state: ProposalState::Open,
                metadata: d.new_review_metadata(),
                action: ProposalAction::ConvertBlock {
                    block: block.into(),
                    target: target.clone(),
                    expected: d.conversion_snapshot(block)?,
                },
            };
            d.put_record(PROPOSALS, id, &proposal)
        })
    }

    // Only block records and empty container sources change. Existing text
    // segments are never copied, including when lifting a line out of code.
    fn convert_tree(
        &mut self,
        blocks: &mut Vec<Block>,
        id: &str,
        target: &BlockConversion,
    ) -> Result<()> {
        let index = blocks
            .iter()
            .position(|b| b.id == id && !b.deleted)
            .ok_or_else(|| DocumentError::NotFound {
                kind: "blocks",
                id: id.into(),
            })?;
        let previous = blocks[index].clone();
        let attrs = target.attributes(&previous)?;
        if target.kind == "code_block" {
            if previous.kind == "code_block" || previous.kind == "code_line" {
                return Ok(());
            }
            let wrapper = self.code_wrapper(previous.parent.clone(), previous.position)?;
            blocks[index].kind = "code_line".into();
            blocks[index].attrs = attrs;
            blocks[index].parent = Some(wrapper.id.clone());
            blocks[index].position = 0;
            blocks.push(wrapper);
        } else if previous.kind == "code_block" {
            let mut children: Vec<_> = blocks
                .iter()
                .filter(|b| b.parent.as_deref() == Some(id))
                .cloned()
                .collect();
            children.sort_by_key(|b| b.position);
            if children.is_empty() {
                blocks[index].kind = target.kind.clone();
                blocks[index].attrs = attrs;
                return Ok(());
            }
            for block in blocks.iter_mut() {
                if block.parent == previous.parent && block.position > previous.position {
                    block.position += children.len() - 1;
                }
                if let Some(position) = children.iter().position(|child| child.id == block.id) {
                    block.attrs = target.attributes(block)?;
                    block.kind = target.kind.clone();
                    block.parent = previous.parent.clone();
                    block.position = previous.position + position;
                }
            }
            blocks[index].deleted = true;
        } else if previous.kind == "code_line" && target.kind != "code_line" {
            let parent = previous
                .parent
                .as_deref()
                .ok_or_else(|| DocumentError::Invalid("Code line has no container".into()))?;
            let container_index = blocks
                .iter()
                .position(|b| b.id == parent)
                .ok_or_else(|| DocumentError::Invalid("Code container was removed".into()))?;
            let container = blocks[container_index].clone();
            let prefix = blocks
                .iter()
                .any(|b| b.parent.as_deref() == Some(parent) && b.position < previous.position);
            let suffix = blocks
                .iter()
                .any(|b| b.parent.as_deref() == Some(parent) && b.position > previous.position);
            let extra = usize::from(prefix) + usize::from(suffix);
            for block in blocks.iter_mut() {
                if block.parent == container.parent && block.position > container.position {
                    block.position += extra;
                }
            }
            if prefix && suffix {
                let mut after =
                    self.code_wrapper(container.parent.clone(), container.position + 2)?;
                after.attrs = container.attrs.clone();
                for block in blocks.iter_mut().filter(|b| {
                    b.parent.as_deref() == Some(parent) && b.position > previous.position
                }) {
                    block.parent = Some(after.id.clone());
                    block.position -= previous.position + 1;
                }
                blocks.push(after);
            } else if suffix {
                blocks[container_index].position += 1;
                for block in blocks.iter_mut().filter(|b| {
                    b.parent.as_deref() == Some(parent) && b.position > previous.position
                }) {
                    block.position -= previous.position + 1;
                }
            } else if !prefix {
                blocks[container_index].deleted = true;
            }
            blocks[index].parent = container.parent;
            blocks[index].position = container.position + usize::from(prefix);
            blocks[index].kind = target.kind.clone();
            blocks[index].attrs = attrs;
        } else {
            blocks[index].kind = target.kind.clone();
            blocks[index].attrs = attrs;
        }
        Ok(())
    }

    fn code_wrapper(&mut self, parent: Option<String>, position: usize) -> Result<Block> {
        let id = self.fresh_id();
        self.require_new(BLOCKS, &id)?;
        Ok(Block {
            id,
            kind: "code_block".into(),
            parent,
            position,
            attrs: BTreeMap::new(),
            segments: vec![self.create_source("")?],
            deleted: false,
        })
    }
}
