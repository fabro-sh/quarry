//! A thin WebAssembly boundary. Document rules remain in the Rust command
//! layer; the editor only translates selections and explicit operations.
use quarry_document::{CommandBuilder, CommandRequest, Document, TextPoint};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct NativeDocument {
    document: Document,
}

/// An isolated command builder. Intermediate structure is private; only a
/// validated finish can become a document or be persisted by the browser.
#[wasm_bindgen]
pub struct NativeCommandBuilder {
    builder: CommandBuilder,
}

#[wasm_bindgen]
impl NativeCommandBuilder {
    pub fn locate_point(&self, point: &str) -> Result<String, JsValue> {
        let point: TextPoint = serde_json::from_str(point).map_err(error)?;
        serde_json::to_string(&self.builder.locate_point(&point).map_err(error)?).map_err(error)
    }
    pub fn block_view(&self, id: &str) -> Result<String, JsValue> {
        serde_json::to_string(&self.builder.block_view(id).map_err(error)?).map_err(error)
    }
    pub fn push(&mut self, commands: &str) -> Result<(), JsValue> {
        self.builder
            .push(serde_json::from_str(commands).map_err(error)?)
            .map_err(error)
    }

    pub fn view(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.builder.view().map_err(error)?).map_err(error)
    }

    pub fn point(&self, block: &str, offset: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.builder.point(block, offset).map_err(error)?).map_err(error)
    }

    pub fn selection(&self, block: &str, start: usize, end: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.builder.selection(block, start, end).map_err(error)?)
            .map_err(error)
    }

    pub fn proposal_point(&self, id: &str, offset: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.builder.proposal_point(id, offset).map_err(error)?)
            .map_err(error)
    }

    pub fn proposal_selection(
        &self,
        id: &str,
        start: usize,
        end: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .builder
                .proposal_selection(id, start, end)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn proposed_block_point(
        &self,
        proposal: &str,
        block: &str,
        offset: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .builder
                .proposed_block_point(proposal, block, offset)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn proposed_block_selection(
        &self,
        proposal: &str,
        block: &str,
        start: usize,
        end: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .builder
                .proposed_block_selection(proposal, block, start, end)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn finish(self) -> Result<NativeDocument, JsValue> {
        let (document, _) = self.builder.finish().map_err(error)?;
        Ok(NativeDocument { document })
    }
}

fn error(value: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&value.to_string())
}

#[wasm_bindgen]
impl NativeDocument {
    pub fn capture_target(&self, ranges: &str) -> Result<String, JsValue> {
        let ranges: Vec<quarry_document::TextRange> =
            serde_json::from_str(ranges).map_err(error)?;
        serde_json::to_string(&self.document.capture_target(&ranges).map_err(error)?).map_err(error)
    }

    pub fn resolve_target(&self, target: &str) -> Result<String, JsValue> {
        let target: Vec<quarry_document::TargetFragment> =
            serde_json::from_str(target).map_err(error)?;
        serde_json::to_string(&self.document.resolve_target(&target).map_err(error)?).map_err(error)
    }
    pub fn command_builder(
        &self,
        request_id: &str,
        at: &str,
    ) -> Result<NativeCommandBuilder, JsValue> {
        Ok(NativeCommandBuilder {
            builder: self
                .document
                .command_builder(request_id, at)
                .map_err(error)?,
        })
    }
    #[wasm_bindgen(constructor)]
    pub fn new(id: &str) -> Result<Self, JsValue> {
        Ok(Self {
            document: Document::with_id(id).map_err(error)?,
        })
    }

    pub fn load(bytes: &[u8]) -> Result<Self, JsValue> {
        Ok(Self {
            document: Document::load(bytes).map_err(error)?,
        })
    }

    pub fn fork(&self) -> Self {
        Self {
            document: self.document.fork(),
        }
    }

    /// Import creates a separate native document. Existing documents are
    /// changed through commands; this is not a replacement-state API.
    pub fn from_markdown(id: &str, markdown: &str) -> Result<Self, JsValue> {
        let mut next = 0;
        Ok(Self {
            document: quarry_markdown::import_markdown_document(id, markdown, &mut || {
                next += 1;
                format!("{id}:import:{next}")
            })
            .map_err(error)?,
        })
    }

    pub fn markdown(&self) -> Result<String, JsValue> {
        quarry_markdown::document_to_markdown(&self.document).map_err(error)
    }

    pub fn save(&self) -> Vec<u8> {
        self.document.save()
    }

    pub fn save_after(&self, heads: &str) -> Result<Vec<u8>, JsValue> {
        let heads: Vec<String> = serde_json::from_str(heads).map_err(error)?;
        let hashes = heads
            .iter()
            .map(|head| head.parse())
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        self.document.save_after(&hashes).map_err(error)
    }

    pub fn heads(&self) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .document
                .heads()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .map_err(error)
    }

    pub fn contains_history(&self, heads: &str) -> Result<bool, JsValue> {
        let heads: Vec<String> = serde_json::from_str(heads).map_err(error)?;
        let hashes = heads
            .iter()
            .map(|head| head.parse())
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        Ok(self.document.contains_history(&hashes))
    }

    pub fn view(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.document.view().map_err(error)?).map_err(error)
    }

    pub fn point(&self, block: &str, offset: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.document.point(block, offset).map_err(error)?).map_err(error)
    }

    pub fn selection(&self, block: &str, start: usize, end: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.document.selection(block, start, end).map_err(error)?)
            .map_err(error)
    }

    pub fn proposal_point(&self, proposal: &str, offset: usize) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .document
                .proposal_point(proposal, offset)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn proposed_block_point(
        &self,
        proposal: &str,
        block: &str,
        offset: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .document
                .proposed_block_point(proposal, block, offset)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn proposed_block_selection(
        &self,
        proposal: &str,
        block: &str,
        start: usize,
        end: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .document
                .proposed_block_selection(proposal, block, start, end)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn proposal_selection(
        &self,
        proposal: &str,
        start: usize,
        end: usize,
    ) -> Result<String, JsValue> {
        serde_json::to_string(
            &self
                .document
                .proposal_selection(proposal, start, end)
                .map_err(error)?,
        )
        .map_err(error)
    }

    pub fn locate_point(&self, point: &str) -> Result<String, JsValue> {
        let point: TextPoint = serde_json::from_str(point).map_err(error)?;
        serde_json::to_string(&self.document.locate_point(&point).map_err(error)?).map_err(error)
    }

    pub fn apply(&mut self, request: &str) -> Result<(), JsValue> {
        let request: CommandRequest = serde_json::from_str(request).map_err(error)?;
        self.document.apply_request(&request).map_err(error)
    }

    pub fn merge(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        self.document
            .merge(&Document::load(bytes).map_err(error)?)
            .map_err(error)
    }

    pub fn merge_changes(&mut self, bytes: &[u8], base: &str, heads: &str) -> Result<(), JsValue> {
        let base: Vec<String> = serde_json::from_str(base).map_err(error)?;
        let heads: Vec<String> = serde_json::from_str(heads).map_err(error)?;
        let parse = |hashes: Vec<String>| {
            hashes
                .into_iter()
                .map(|hash| hash.parse().map_err(error))
                .collect::<Result<Vec<_>, _>>()
        };
        self.document
            .merge_changes(bytes, &parse(base)?, &parse(heads)?)
            .map_err(error)
    }
}
