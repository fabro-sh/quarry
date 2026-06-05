use crate::slate::{Attrs, Node};

pub fn normalize_insert_nodes(nodes: Vec<Node>) -> Vec<Node> {
    nodes.into_iter().map(normalize_node).collect()
}

fn normalize_node(node: Node) -> Node {
    match node {
        Node::Element {
            ty,
            attrs,
            children,
        } => Node::Element {
            ty,
            attrs,
            children: merge_adjacent_text(children.into_iter().map(normalize_node).collect()),
        },
        text => text,
    }
}

fn merge_adjacent_text(nodes: Vec<Node>) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match (out.last_mut(), node) {
            (
                Some(Node::Text {
                    text,
                    marks: existing,
                }),
                Node::Text { text: next, marks },
            ) if same_attrs(existing, &marks) => text.push_str(&next),
            (_, node) => out.push(node),
        }
    }
    out
}

fn same_attrs(left: &Attrs, right: &Attrs) -> bool {
    left == right
}
