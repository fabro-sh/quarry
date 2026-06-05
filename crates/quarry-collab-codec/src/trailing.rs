use crate::slate::Node;

pub fn strip_trailing_empty_paragraphs(nodes: &[Node]) -> Vec<Node> {
    let mut end = nodes.len();
    while end > 0 && is_empty_paragraph(&nodes[end - 1]) {
        end -= 1;
    }
    nodes[..end].to_vec()
}

pub fn is_empty_paragraph(node: &Node) -> bool {
    match node {
        Node::Element {
            ty,
            attrs,
            children,
        } if ty == "p" && attrs.is_empty() => children.iter().all(|child| {
            matches!(
                child,
                Node::Text {
                    text,
                    marks
                } if text.is_empty() && marks.is_empty()
            )
        }),
        _ => false,
    }
}
