//! Slate nodes → Markdown, the inverse of `crate::markdown`.
//!
//! The output is deterministic and idempotent: parsing the output and writing
//! it again yields byte-identical Markdown. Exact preservation of the input
//! bytes is a non-goal (one-time normalization is accepted): loose lists
//! tighten, hard breaks soften, setext headings become ATX, `<sup>`/`^x^`
//! forms normalize, and punctuation that could re-parse as syntax is
//! backslash-escaped.

use crate::slate::{Attrs, Node};
use crate::Unsupported;
use serde_json::Value;

/// Inline mark nesting order, outermost first. `code` is innermost because
/// code spans are atomic (no marks render inside them).
/// Whether `key` is an inline mark the Markdown writer can render. Marks
/// outside this set make [`slate_to_markdown`] fail with [`Unsupported`];
/// the session checkpoint projection drops them up front (see
/// `session_doc::classify_marks`) so a live session can always persist.
pub fn is_known_inline_mark(key: &str) -> bool {
    MARK_ORDER.contains(&key)
}

const MARK_ORDER: [&str; 7] = [
    "bold",
    "italic",
    "strikethrough",
    "underline",
    "superscript",
    "subscript",
    "code",
];

const MARK_DELIMITERS: [(&str, &str, &str); 6] = [
    ("bold", "**", "**"),
    ("italic", "*", "*"),
    ("strikethrough", "~~", "~~"),
    ("underline", "<u>", "</u>"),
    ("superscript", "<sup>", "</sup>"),
    ("subscript", "<sub>", "</sub>"),
];

/// Characters escaped everywhere in plain text (outside code).
const GLOBAL_ESCAPES: [char; 14] = [
    '\\', '`', '*', '_', '[', ']', '<', '>', '&', '#', '|', '~', '^', '$',
];

/// Characters escaped only at the start of a line.
const LINE_START_ESCAPES: [char; 4] = ['-', '+', '=', ':'];

pub fn slate_to_markdown(nodes: &[Node]) -> Result<String, Unsupported> {
    let mut out = String::new();
    let mut runs = ListRuns::default();
    let mut previous: Option<Option<ListItemKey>> = None;
    for node in nodes {
        let key = list_item_key(node);
        if let Some(previous) = previous {
            out.push_str(block_separator(previous, key));
        }
        match key {
            Some(key) => {
                let number = runs.marker_number(node, key);
                out.push_str(&render_list_item(node, key, number)?);
            }
            None => {
                runs.reset();
                out.push_str(&render_block(node)?);
            }
        }
        previous = Some(key);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
struct ListItemKey {
    indent: u64,
    ordered: bool,
}

fn list_item_key(node: &Node) -> Option<ListItemKey> {
    let Node::Element { ty, attrs, .. } = node else {
        return None;
    };
    let style = attrs.get("listStyleType").and_then(Value::as_str)?;
    if ty != "p" {
        return None;
    }
    Some(ListItemKey {
        indent: attrs
            .get("indent")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1),
        ordered: style == "decimal",
    })
}

/// Consecutive list items join on one line break (a tight list). A blank line
/// separates everything else, including a marker-family change at the same
/// indent (which re-parses as two adjacent lists, matching the row split).
fn block_separator(previous: Option<ListItemKey>, current: Option<ListItemKey>) -> &'static str {
    match (previous, current) {
        (Some(previous), Some(current))
            if previous.indent == current.indent && previous.ordered != current.ordered =>
        {
            "\n\n"
        }
        (Some(_), Some(_)) => "\n",
        _ => "\n\n",
    }
}

/// Ordered markers must be sequential within a run regardless of the rows'
/// `listStart` values: a re-parse always numbers items sequentially from the
/// first marker, so sequential rendering is the only stable choice.
#[derive(Default)]
struct ListRuns {
    counters: Vec<(u64, u64)>,
}

impl ListRuns {
    fn marker_number(&mut self, node: &Node, key: ListItemKey) -> u64 {
        self.counters.retain(|(indent, _)| *indent <= key.indent);
        let slot = self
            .counters
            .iter()
            .position(|(indent, _)| *indent == key.indent);
        if !key.ordered {
            if let Some(slot) = slot {
                self.counters.remove(slot);
            }
            return 0;
        }
        let number = match slot {
            Some(slot) => self.counters[slot].1 + 1,
            None => list_start(node),
        };
        match slot {
            Some(slot) => self.counters[slot].1 = number,
            None => self.counters.push((key.indent, number)),
        }
        number
    }

    fn reset(&mut self) {
        self.counters.clear();
    }
}

fn list_start(node: &Node) -> u64 {
    let Node::Element { attrs, .. } = node else {
        return 1;
    };
    attrs.get("listStart").and_then(Value::as_u64).unwrap_or(1)
}

fn render_block(node: &Node) -> Result<String, Unsupported> {
    let Node::Element {
        ty,
        attrs,
        children,
    } = node
    else {
        return Err(Unsupported::new("bare text node at block level"));
    };
    match ty.as_str() {
        "p" => Ok(tidy_lines(&render_inline(children, true)?)),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = ty[1..].parse::<usize>().expect("heading level digit");
            // ATX headings are single-line: a multi-line setext heading joins
            // with spaces (anything else demotes the extra lines on re-parse).
            let inline = tidy_lines(&render_inline(children, false)?).replace('\n', " ");
            if inline.is_empty() {
                Ok("#".repeat(level))
            } else {
                Ok(format!("{} {inline}", "#".repeat(level)))
            }
        }
        "blockquote" => render_blockquote(children),
        "code_block" => render_code_block(attrs, children),
        "mermaid" => {
            let code = attrs.get("code").and_then(Value::as_str).unwrap_or("");
            Ok(render_fence("mermaid", code))
        }
        "table" => render_table(attrs, children),
        "img" => {
            let caption = caption_nodes(attrs)?;
            let url = attrs.get("url").and_then(Value::as_str).unwrap_or("");
            let inline = render_inline(&caption, false)?;
            Ok(format!("![{inline}]({})", render_url(url)))
        }
        "hr" => Ok("***".to_string()),
        "raw_markdown" => Ok(attrs
            .get("markdown")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()),
        other => Err(Unsupported::new(format!(
            "no markdown form for block <{other}>"
        ))),
    }
}

fn render_list_item(node: &Node, key: ListItemKey, number: u64) -> Result<String, Unsupported> {
    let Node::Element {
        attrs, children, ..
    } = node
    else {
        return Err(Unsupported::new("bare text node at block level"));
    };
    let style = attrs
        .get("listStyleType")
        .and_then(Value::as_str)
        .unwrap_or("disc");
    let marker = match style {
        "todo" => {
            let checked = attrs
                .get("checked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if checked {
                "- [x] ".to_string()
            } else {
                "- [ ] ".to_string()
            }
        }
        "decimal" => format!("{number}. "),
        _ => "- ".to_string(),
    };
    let prefix = "    ".repeat(key.indent as usize - 1);
    let continuation = format!("{prefix}{}", " ".repeat(marker.len()));
    let inline = tidy_lines(&render_inline(children, true)?);
    let body = inline.replace('\n', &format!("\n{continuation}"));
    Ok(format!("{prefix}{marker}{body}"))
}

fn render_blockquote(children: &[Node]) -> Result<String, Unsupported> {
    let inline = tidy_lines(&render_inline(children, true)?);
    let quoted: Vec<String> = inline
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect();
    Ok(quoted.join("\n"))
}

fn render_code_block(attrs: &Attrs, children: &[Node]) -> Result<String, Unsupported> {
    let lines: Vec<String> = children
        .iter()
        .map(|child| match child {
            Node::Element { ty, children, .. } if ty == "code_line" => Ok(plain_text(children)),
            _ => Err(Unsupported::new("code_block with non-code_line child")),
        })
        .collect::<Result<_, _>>()?;
    let lang = attrs.get("lang").and_then(Value::as_str).unwrap_or("");
    Ok(render_fence(lang, &lines.join("\n")))
}

fn render_fence(lang: &str, code: &str) -> String {
    let longest_run = code.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest_run + 1).max(3));
    if code.is_empty() {
        format!("{fence}{lang}\n{fence}")
    } else {
        format!("{fence}{lang}\n{code}\n{fence}")
    }
}

fn render_table(attrs: &Attrs, children: &[Node]) -> Result<String, Unsupported> {
    let mut lines = Vec::new();
    let mut header_columns = 0usize;
    for (index, row) in children.iter().enumerate() {
        let Node::Element { ty, children, .. } = row else {
            return Err(Unsupported::new("table with bare text child"));
        };
        if ty != "tr" {
            return Err(Unsupported::new(format!("table with non-row child <{ty}>")));
        }
        let cells: Vec<String> = children
            .iter()
            .map(render_table_cell)
            .collect::<Result<_, _>>()?;
        if index == 0 {
            header_columns = cells.len();
        }
        lines.push(format!("| {} |", cells.join(" | ")));
        if index == 0 {
            lines.push(render_alignment_row(attrs, header_columns));
        }
    }
    Ok(lines.join("\n"))
}

fn render_table_cell(cell: &Node) -> Result<String, Unsupported> {
    let Node::Element { ty, children, .. } = cell else {
        return Err(Unsupported::new("table row with bare text child"));
    };
    if ty != "th" && ty != "td" {
        return Err(Unsupported::new(format!(
            "table row with non-cell child <{ty}>"
        )));
    }
    let inline: Vec<String> = children
        .iter()
        .map(|paragraph| match paragraph {
            Node::Element { ty, children, .. } if ty == "p" => render_inline(children, false),
            _ => Err(Unsupported::new("table cell without paragraph content")),
        })
        .collect::<Result<_, _>>()?;
    Ok(inline.join(" ").trim().to_string())
}

fn render_alignment_row(attrs: &Attrs, columns: usize) -> String {
    let aligns = attrs
        .get("align")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let cells: Vec<&str> = (0..columns)
        .map(|index| match aligns.get(index).and_then(Value::as_str) {
            Some("left") => ":--",
            Some("center") => ":-:",
            Some("right") => "--:",
            _ => "---",
        })
        .collect();
    format!("| {} |", cells.join(" | "))
}

fn caption_nodes(attrs: &Attrs) -> Result<Vec<Node>, Unsupported> {
    match attrs.get("caption") {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| Unsupported::new("image caption is not a slate node list")),
    }
}

fn plain_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text { text, .. } => out.push_str(text),
            Node::Element { children, .. } => out.push_str(&plain_text(children)),
        }
    }
    out
}

/// Trims whitespace at line edges of rendered inline content. Import-derived
/// rows never need line-edge whitespace to survive (CommonMark strips it on
/// re-parse anyway), and trimming keeps the output stable.
fn tidy_lines(inline: &str) -> String {
    inline
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Inline rendering: mark grouping and text escaping.
// ---------------------------------------------------------------------------

fn render_inline(children: &[Node], line_start: bool) -> Result<String, Unsupported> {
    let mut out = String::new();
    let mut line_start = line_start;
    render_spans(children, &mut out, &mut line_start)?;
    Ok(out)
}

fn render_spans(
    spans: &[Node],
    out: &mut String,
    line_start: &mut bool,
) -> Result<(), Unsupported> {
    let mut index = 0;
    while index < spans.len() {
        let marks = span_marks(&spans[index]);
        let Some(mark) = outermost_mark(&marks)? else {
            render_atom(&spans[index], out, line_start)?;
            index += 1;
            continue;
        };
        if mark == "code" {
            render_code_span(&spans[index], out, line_start)?;
            index += 1;
            continue;
        }
        let group_end = spans[index..]
            .iter()
            .position(|span| !span_marks(span).contains_key(mark))
            .map(|offset| index + offset)
            .unwrap_or(spans.len());
        let mut inner: Vec<Node> = spans[index..group_end]
            .iter()
            .map(|span| without_mark(span, mark))
            .collect();
        // Emphasis delimiters are not left/right flanking next to whitespace
        // ("**bold **" re-parses as literal stars), so a run's edge
        // whitespace is hoisted outside the delimiters.
        let leading = hoist_leading_whitespace(&mut inner);
        let trailing = hoist_trailing_whitespace(&mut inner);
        push_escaped(&leading, out, line_start);
        if !inner.is_empty() {
            let (open, close) = mark_delimiters(mark)?;
            out.push_str(open);
            *line_start = false;
            render_spans(&inner, out, line_start)?;
            out.push_str(close);
        }
        push_escaped(&trailing, out, line_start);
        index = group_end;
    }
    Ok(())
}

fn hoist_leading_whitespace(spans: &mut Vec<Node>) -> String {
    let mut hoisted = String::new();
    while let Some(Node::Text { text, .. }) = spans.first_mut() {
        let kept = text.trim_start_matches([' ', '\t', '\n']).len();
        let split = text.len() - kept;
        if split == 0 {
            break;
        }
        hoisted.push_str(&text[..split]);
        text.replace_range(..split, "");
        if text.is_empty() {
            spans.remove(0);
        } else {
            break;
        }
    }
    hoisted
}

fn hoist_trailing_whitespace(spans: &mut Vec<Node>) -> String {
    let mut hoisted = String::new();
    while let Some(Node::Text { text, .. }) = spans.last_mut() {
        let kept = text.trim_end_matches([' ', '\t', '\n']).len();
        if kept == text.len() {
            break;
        }
        hoisted.insert_str(0, &text[kept..]);
        text.truncate(kept);
        if text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
    hoisted
}

fn render_atom(span: &Node, out: &mut String, line_start: &mut bool) -> Result<(), Unsupported> {
    match span {
        Node::Text { text, .. } => {
            push_escaped(text, out, line_start);
            Ok(())
        }
        Node::Element {
            ty,
            attrs,
            children,
        } if ty == "a" => {
            let url = attrs.get("url").and_then(Value::as_str).unwrap_or("");
            out.push('[');
            *line_start = false;
            render_spans(children, out, line_start)?;
            out.push_str(&format!("]({})", render_url(url)));
            Ok(())
        }
        Node::Element { ty, attrs, .. } if ty == "wikilink" => {
            out.push_str(&render_wikilink(attrs));
            *line_start = false;
            Ok(())
        }
        Node::Element {
            ty,
            attrs,
            children: _,
        } if ty == "img" => {
            let caption = caption_nodes(attrs)?;
            let url = attrs.get("url").and_then(Value::as_str).unwrap_or("");
            out.push_str("![");
            *line_start = false;
            let mut caption_line_start = false;
            render_spans(&caption, out, &mut caption_line_start)?;
            out.push_str(&format!("]({})", render_url(url)));
            Ok(())
        }
        Node::Element { ty, .. } => Err(Unsupported::new(format!(
            "no markdown form for inline <{ty}>"
        ))),
    }
}

fn render_wikilink(attrs: &Attrs) -> String {
    let target = attrs.get("target").and_then(Value::as_str).unwrap_or("");
    let anchor = attrs.get("anchor").and_then(Value::as_str);
    let alias = attrs.get("alias").and_then(Value::as_str);
    let embed = attrs.get("embed").and_then(Value::as_bool).unwrap_or(false);
    let mut inner = target.to_string();
    if let Some(anchor) = anchor {
        inner.push('#');
        inner.push_str(anchor);
    }
    if let Some(alias) = alias {
        inner.push('|');
        inner.push_str(alias);
    }
    let bang = if embed { "!" } else { "" };
    format!("{bang}[[{inner}]]")
}

fn render_code_span(
    span: &Node,
    out: &mut String,
    line_start: &mut bool,
) -> Result<(), Unsupported> {
    let Node::Text { text, .. } = span else {
        return Err(Unsupported::new("code mark on a non-text span"));
    };
    let longest_run = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest_run + 1).max(1));
    // CommonMark strips one space of padding only when the content is not
    // entirely spaces, so all-space content must stay unpadded or it grows
    // on every round trip.
    let all_spaces = !text.is_empty() && text.chars().all(|ch| ch == ' ');
    let needs_padding = !all_spaces
        && (text.starts_with('`')
            || text.ends_with('`')
            || text.starts_with(' ')
            || text.ends_with(' '));
    if needs_padding {
        out.push_str(&format!("{fence} {text} {fence}"));
    } else {
        out.push_str(&format!("{fence}{text}{fence}"));
    }
    *line_start = false;
    Ok(())
}

fn span_marks(span: &Node) -> Attrs {
    match span {
        Node::Text { marks, .. } => marks.clone(),
        Node::Element { ty, children, .. } if ty == "a" => {
            // A link participates in a mark group when every text child
            // carries the mark (the import inherits surrounding context marks
            // into link children).
            let mut common: Option<Attrs> = None;
            for child in children {
                let Node::Text { marks, .. } = child else {
                    return Attrs::new();
                };
                common = Some(match common {
                    None => marks.clone(),
                    Some(existing) => existing
                        .into_iter()
                        .filter(|(key, _)| marks.contains_key(key))
                        .collect(),
                });
            }
            let mut common = common.unwrap_or_default();
            // `code` never promotes onto a link group: a code span cannot
            // wrap a non-text span, but CommonMark renders code spans inside
            // link text, so the mark stays on the children and the link
            // exports as [`docs`](url). Live sessions can produce this shape
            // (CodePlugin + LinkPlugin); promotion used to wedge every
            // checkpoint with "code mark on a non-text span".
            common.shift_remove("code");
            common
        }
        Node::Element { .. } => Attrs::new(),
    }
}

fn without_mark(span: &Node, mark: &str) -> Node {
    match span {
        Node::Text { text, marks } => {
            let mut marks = marks.clone();
            marks.shift_remove(mark);
            Node::text(text.clone(), marks)
        }
        Node::Element {
            ty,
            attrs,
            children,
        } if ty == "a" => Node::element(
            ty.clone(),
            attrs.clone(),
            children
                .iter()
                .map(|child| without_mark(child, mark))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn outermost_mark(marks: &Attrs) -> Result<Option<&'static str>, Unsupported> {
    if let Some(unknown) = marks.keys().find(|key| !MARK_ORDER.contains(&key.as_str())) {
        return Err(Unsupported::new(format!("unknown inline mark {unknown}")));
    }
    Ok(MARK_ORDER
        .into_iter()
        .find(|mark| marks.contains_key(*mark)))
}

fn mark_delimiters(mark: &str) -> Result<(&'static str, &'static str), Unsupported> {
    MARK_DELIMITERS
        .into_iter()
        .find(|(name, _, _)| *name == mark)
        .map(|(_, open, close)| (open, close))
        .ok_or_else(|| Unsupported::new(format!("unknown inline mark {mark}")))
}

fn render_url(url: &str) -> String {
    if url.contains(|ch: char| ch.is_whitespace() || ch == '(' || ch == ')') {
        format!("<{url}>")
    } else {
        url.to_string()
    }
}

fn push_escaped(text: &str, out: &mut String, line_start: &mut bool) {
    // `digit_run` tracks digits that opened a line: "12." there would
    // re-parse as an ordered-list marker, so the punctuation gets escaped.
    let mut digit_run = false;
    for ch in text.chars() {
        if ch == '\n' {
            out.push('\n');
            *line_start = true;
            digit_run = false;
            continue;
        }
        if *line_start {
            *line_start = false;
            if LINE_START_ESCAPES.contains(&ch) {
                out.push('\\');
                out.push(ch);
                continue;
            }
            if ch.is_ascii_digit() {
                digit_run = true;
                out.push(ch);
                continue;
            }
        } else if digit_run {
            if ch.is_ascii_digit() {
                out.push(ch);
                continue;
            }
            digit_run = false;
            if ch == '.' || ch == ')' {
                out.push('\\');
                out.push(ch);
                continue;
            }
        }
        if GLOBAL_ESCAPES.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
}
