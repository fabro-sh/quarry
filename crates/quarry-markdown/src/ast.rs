use indexmap::IndexMap;
use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Attrs = IndexMap<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element {
        ty: String,
        attrs: Attrs,
        children: Vec<Node>,
    },
    Text {
        text: String,
        marks: Attrs,
    },
}

impl Node {
    pub fn element(ty: impl Into<String>, attrs: Attrs, children: Vec<Node>) -> Self {
        Self::Element {
            ty: ty.into(),
            attrs,
            children,
        }
    }

    pub fn text(text: impl Into<String>, marks: Attrs) -> Self {
        Self::Text {
            text: text.into(),
            marks,
        }
    }
}

impl Serialize for Node {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Node::Element {
                ty,
                attrs,
                children,
            } => {
                let mut map = serializer.serialize_map(Some(attrs.len() + 2))?;
                map.serialize_entry("children", children)?;
                for (key, value) in attrs {
                    map.serialize_entry(key, value)?;
                }
                map.serialize_entry("type", ty)?;
                map.end()
            }
            Node::Text { text, marks } => {
                let mut map = serializer.serialize_map(Some(marks.len() + 1))?;
                for (key, value) in marks {
                    map.serialize_entry(key, value)?;
                }
                map.serialize_entry("text", text)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut map = IndexMap::<String, Value>::deserialize(deserializer)?;
        if let Some(text) = map.shift_remove("text") {
            let text = text
                .as_str()
                .ok_or_else(|| de::Error::custom("text node text must be a string"))?
                .to_string();
            return Ok(Node::Text { text, marks: map });
        }

        let ty = map
            .shift_remove("type")
            .and_then(|value| value.as_str().map(str::to_string))
            .ok_or_else(|| de::Error::custom("element node type must be a string"))?;
        let children = map
            .shift_remove("children")
            .ok_or_else(|| de::Error::custom("element node children missing"))
            .and_then(|value| serde_json::from_value(value).map_err(de::Error::custom))?;
        Ok(Node::Element {
            ty,
            attrs: map,
            children,
        })
    }
}

pub fn attrs(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Attrs {
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
}
