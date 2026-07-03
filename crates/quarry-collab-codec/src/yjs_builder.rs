use crate::Unsupported;
use crate::review::{ReviewMeta, ReviewMetaEntry, ReviewMetaPatch};
use crate::slate::{Attrs as SlateAttrs, Node};
use serde_json::{Number, Value};
use std::collections::HashMap;
use std::sync::Arc;
use yrs::types::Attrs;
use yrs::types::text::YChange;
use yrs::{
    Any, Doc, Map, MapRef, OffsetKind, Options, Out, ReadTxn, Text, Transact, WriteTxn, Xml,
    XmlTextPrelim, XmlTextRef,
};

pub type BuiltNode = Node;

pub fn build_nodes(nodes: &[Node]) -> Result<Vec<BuiltNode>, Unsupported> {
    for node in nodes {
        validate_node(node)?;
    }
    Ok(nodes.to_vec())
}

pub fn apply_built(
    txn: &mut yrs::TransactionMut<'_>,
    root: &XmlTextRef,
    index: u32,
    nodes: &[BuiltNode],
) {
    let mut offset = index;
    for node in nodes {
        offset += insert_node(txn, root, offset, node);
    }
}

pub fn encode_update_v1_from_built(nodes: &[BuiltNode], root_name: &str) -> Vec<u8> {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    let root = {
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text(root_name);
        let root: &XmlTextRef = text.as_ref();
        apply_built(&mut txn, root, 0, nodes);
        root.clone()
    };
    let txn = doc.transact();
    let _ = root;
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

pub fn encode_update_v1_from_built_with_review(
    nodes: &[BuiltNode],
    root_name: &str,
    review_root_name: &str,
    meta: &ReviewMeta,
) -> Vec<u8> {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text(root_name);
        let root: &XmlTextRef = text.as_ref();
        apply_built(&mut txn, root, 0, nodes);
        let review = txn.get_or_insert_map(review_root_name);
        write_review_meta_to_map(&mut txn, &review, meta);
    }
    let txn = doc.transact();
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

pub fn write_review_meta_to_map(
    txn: &mut yrs::TransactionMut<'_>,
    root: &MapRef,
    meta: &ReviewMeta,
) {
    let comments = ensure_review_section(txn, root, "comments");
    comments.clear(txn);
    write_review_entries(txn, &comments, &meta.comments);

    let suggestions = ensure_review_section(txn, root, "suggestions");
    suggestions.clear(txn);
    write_review_entries(txn, &suggestions, &meta.suggestions);
}

pub fn apply_review_patch_to_map(
    txn: &mut yrs::TransactionMut<'_>,
    root: &MapRef,
    patch: &ReviewMetaPatch,
) {
    let comments = ensure_review_section(txn, root, "comments");
    write_review_entries(txn, &comments, &patch.comments);
    for id in &patch.remove_comments {
        let _ = comments.remove(txn, id);
    }

    let suggestions = ensure_review_section(txn, root, "suggestions");
    write_review_entries(txn, &suggestions, &patch.suggestions);
    for id in &patch.remove_suggestions {
        let _ = suggestions.remove(txn, id);
    }
}

pub fn xmltext_to_slate<T: ReadTxn>(txn: &T, root: &XmlTextRef) -> Result<Node, Unsupported> {
    let children = text_children_to_slate(txn, root)?;
    Ok(Node::element("fragment", SlateAttrs::new(), children))
}

fn insert_node(
    txn: &mut yrs::TransactionMut<'_>,
    parent: &XmlTextRef,
    index: u32,
    node: &Node,
) -> u32 {
    match node {
        Node::Text { text, marks } => {
            parent.insert_with_attributes(txn, index, text, attrs_to_yrs(marks));
            utf16_len(text)
        }
        Node::Element {
            ty,
            attrs,
            children,
        } => {
            let embedded = parent.insert_embed(txn, index, XmlTextPrelim::default());
            for (key, value) in element_attrs_to_yrs(ty, attrs) {
                embedded.insert_attribute(txn, key, value);
            }
            let mut child_offset = 0;
            for child in children {
                child_offset += insert_node(txn, &embedded, child_offset, child);
            }
            1
        }
    }
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

fn validate_node(node: &Node) -> Result<(), Unsupported> {
    match node {
        Node::Text { marks, .. } => {
            for value in marks.values() {
                validate_value(value)?;
            }
        }
        Node::Element {
            ty,
            attrs,
            children,
        } => {
            if ty.is_empty() {
                return Err(Unsupported::new("empty element type"));
            }
            for value in attrs.values() {
                validate_value(value)?;
            }
            for child in children {
                validate_node(child)?;
            }
        }
    }
    Ok(())
}

fn validate_value(value: &Value) -> Result<(), Unsupported> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) if number.as_i64().is_some() || number.as_f64().is_some() => Ok(()),
        Value::Array(values) => values.iter().try_for_each(validate_value),
        Value::Object(map) => map.values().try_for_each(validate_value),
        Value::Number(_) => Err(Unsupported::new("unsupported JSON number")),
    }
}

fn element_attrs_to_yrs(ty: &str, attrs: &SlateAttrs) -> Attrs {
    let mut out = attrs_to_yrs(attrs);
    out.insert(Arc::from("type"), Any::from(ty.to_string()));
    out
}

fn attrs_to_yrs(attrs: &SlateAttrs) -> Attrs {
    attrs
        .iter()
        .map(|(key, value)| (Arc::from(key.as_str()), value_to_any(value)))
        .collect()
}

fn value_to_any(value: &Value) -> Any {
    match value {
        Value::Null => Any::Null,
        Value::Bool(value) => Any::from(*value),
        Value::Number(value) => number_to_any(value),
        Value::String(value) => Any::from(value.clone()),
        Value::Array(values) => Any::from(values.iter().map(value_to_any).collect::<Vec<_>>()),
        Value::Object(map) => Any::from(
            map.iter()
                .map(|(key, value)| (key.clone(), value_to_any(value)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

fn ensure_review_section(txn: &mut yrs::TransactionMut<'_>, root: &MapRef, key: &str) -> MapRef {
    root.get_or_init(txn, key)
}

fn write_review_entries(
    txn: &mut yrs::TransactionMut<'_>,
    section: &MapRef,
    entries: &std::collections::BTreeMap<String, ReviewMetaEntry>,
) {
    for (id, entry) in entries {
        section.insert(txn, id.as_str(), review_entry_to_any(entry));
    }
}

fn review_entry_to_any(entry: &ReviewMetaEntry) -> Any {
    value_to_any(&serde_json::to_value(entry).expect("review metadata entry serializes"))
}

fn number_to_any(value: &Number) -> Any {
    if let Some(value) = value.as_i64() {
        Any::from(value)
    } else if let Some(value) = value.as_u64() {
        if let Ok(value) = i64::try_from(value) {
            Any::from(value)
        } else {
            Any::from(value as f64)
        }
    } else {
        Any::from(value.as_f64().unwrap_or_default())
    }
}

fn text_children_to_slate<T: ReadTxn>(
    txn: &T,
    text: &XmlTextRef,
) -> Result<Vec<Node>, Unsupported> {
    let mut children = Vec::new();
    for diff in text.diff(txn, YChange::identity) {
        let attrs = yrs_attrs_to_slate(diff.attributes.as_deref())?;
        match diff.insert {
            Out::Any(any) => {
                let text = any_to_text(any)?;
                children.push(Node::Text { text, marks: attrs });
            }
            Out::YXmlText(child) => {
                children.push(element_from_embedded_text(txn, attrs, &child)?);
            }
            Out::YText(child) => {
                let child_ref: &XmlTextRef = child.as_ref();
                children.push(element_from_embedded_text(txn, attrs, child_ref)?);
            }
            other => return Err(Unsupported::new(format!("unsupported Yjs embed {other:?}"))),
        }
    }
    Ok(children)
}

fn element_from_embedded_text<T: ReadTxn>(
    txn: &T,
    mut attrs: SlateAttrs,
    child: &XmlTextRef,
) -> Result<Node, Unsupported> {
    for (key, value) in xml_attrs_to_slate(txn, child)? {
        attrs.entry(key).or_insert(value);
    }
    let Some(Value::String(ty)) = attrs.shift_remove("type") else {
        return Err(Unsupported::new("embedded slate element missing type"));
    };
    let mut children = text_children_to_slate(txn, child)?;
    if children.is_empty() {
        children.push(Node::text("", SlateAttrs::new()));
    }
    Ok(Node::Element {
        ty,
        attrs,
        children,
    })
}

fn xml_attrs_to_slate<T: ReadTxn>(txn: &T, child: &XmlTextRef) -> Result<SlateAttrs, Unsupported> {
    let mut entries = child.attributes(txn).collect::<Vec<_>>();
    entries.sort_by_key(|(left, _)| *left);
    let mut out = SlateAttrs::new();
    for (key, value) in entries {
        out.insert(key.to_string(), out_to_value(value)?);
    }
    Ok(out)
}

fn yrs_attrs_to_slate(attrs: Option<&Attrs>) -> Result<SlateAttrs, Unsupported> {
    let mut out = SlateAttrs::new();
    if let Some(attrs) = attrs {
        let mut entries = attrs.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(left, _)| *left);
        for (key, value) in entries {
            out.insert(key.to_string(), any_to_value(value)?);
        }
    }
    Ok(out)
}

fn out_to_value(out: Out) -> Result<Value, Unsupported> {
    match out {
        Out::Any(any) => any_to_value(&any),
        other => Err(Unsupported::new(format!("unsupported XML attr {other:?}"))),
    }
}

fn any_to_text(any: Any) -> Result<String, Unsupported> {
    match any {
        Any::String(value) => Ok(value.to_string()),
        other => Err(Unsupported::new(format!(
            "non-string text insert {other:?}"
        ))),
    }
}

fn any_to_value(any: &Any) -> Result<Value, Unsupported> {
    match any {
        Any::Null | Any::Undefined => Ok(Value::Null),
        Any::Bool(value) => Ok(Value::Bool(*value)),
        Any::Number(value) => Ok(json_number(*value)),
        Any::BigInt(value) => Ok(Value::Number(Number::from(*value))),
        Any::String(value) => Ok(Value::String(value.to_string())),
        Any::Array(values) => values
            .iter()
            .map(any_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Any::Map(map) => {
            let object = map
                .iter()
                .map(|(key, value)| any_to_value(value).map(|value| (key.clone(), value)))
                .collect::<Result<serde_json::Map<_, _>, _>>()?;
            Ok(Value::Object(object))
        }
        other => Err(Unsupported::new(format!("unsupported Yjs attr {other:?}"))),
    }
}

fn json_number(value: f64) -> Value {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return Value::Number(Number::from(value as i64));
    }
    Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
