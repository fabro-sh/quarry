use crate::{Block, BlockContentModel, DocumentError, Result, block_capabilities};
use serde_json::Value;
use std::collections::BTreeMap;

pub const INLINE_BOOLEAN_MARKS: [&str; 7] = [
    "bold",
    "italic",
    "strikethrough",
    "underline",
    "superscript",
    "subscript",
    "code",
];

pub(crate) fn validate_format(name: &str, value: &Value) -> Result<()> {
    let valid = if INLINE_BOOLEAN_MARKS.contains(&name) || name == "wikilink" {
        value.is_null() || value.is_boolean()
    } else if name == "link" {
        value.is_null() || value.is_string()
    } else {
        false
    };
    if !valid {
        return Err(DocumentError::Invalid(format!(
            "Unsupported formatting name or value: {name}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_parent(kind: &str, parent: Option<&str>) -> Result<()> {
    let capability = block_capabilities(kind)
        .ok_or_else(|| DocumentError::Invalid(format!("Unknown block type: {kind}")))?;
    let valid = match parent {
        None => capability.root,
        Some(parent) => {
            block_capabilities(parent).is_some_and(|p| p.children.iter().any(|child| child == kind))
        }
    };
    if !valid {
        return Err(DocumentError::Invalid(format!(
            "Block type {kind} is not allowed in {}",
            parent.unwrap_or("the document root")
        )));
    }
    Ok(())
}

pub(crate) fn normalize_attrs(
    kind: &str,
    mut attrs: BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>> {
    if attrs.contains_key("id") {
        return Err(DocumentError::Invalid(
            "Block attrs cannot contain id".into(),
        ));
    }
    if let Some(style) = attrs.get("listStyleType").cloned() {
        if kind != "p" {
            return Err(DocumentError::Invalid(
                "Only paragraphs can be list items".into(),
            ));
        }
        if !matches!(style.as_str(), Some("disc" | "decimal" | "todo")) {
            return Err(DocumentError::Invalid("Invalid listStyleType".into()));
        }
        let indent = attrs.entry("indent".into()).or_insert(Value::from(1));
        if !indent.as_u64().is_some_and(|n| n > 0 && n <= 128) {
            return Err(DocumentError::Invalid(
                "List indent must be an integer from 1 to 128".into(),
            ));
        }
        if style == "todo" {
            if !attrs
                .entry("checked".into())
                .or_insert(Value::Bool(false))
                .is_boolean()
            {
                return Err(DocumentError::Invalid(
                    "Todo checked must be a boolean".into(),
                ));
            }
        } else {
            attrs.remove("checked");
        }
        if style == "decimal" {
            if attrs.get("listStart").is_some_and(|v| v.as_u64().is_none()) {
                return Err(DocumentError::Invalid(
                    "List start must be a non-negative integer".into(),
                ));
            }
        } else {
            attrs.remove("listStart");
        }
    } else {
        attrs.remove("checked");
        attrs.remove("listStart");
    }
    Ok(attrs)
}

pub(crate) fn validate_block(block: &Block, has_text: bool) -> Result<()> {
    let capability = block_capabilities(&block.kind)
        .ok_or_else(|| DocumentError::Invalid(format!("Unknown block type: {}", block.kind)))?;
    if block.attrs != normalize_attrs(&block.kind, block.attrs.clone())? {
        return Err(DocumentError::Invalid(
            "Block attributes are not canonical".into(),
        ));
    }
    if capability.content != BlockContentModel::Text && has_text {
        return Err(DocumentError::Invalid(format!(
            "{} cannot contain inline text",
            block.kind
        )));
    }
    if capability.content == BlockContentModel::Raw
        && block
            .attrs
            .get("markdown")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(DocumentError::Invalid(
            "raw_markdown requires a non-empty markdown attribute".into(),
        ));
    }
    Ok(())
}
