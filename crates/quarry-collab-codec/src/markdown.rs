use crate::normalize::normalize_insert_nodes;
use crate::slate::{attrs, Attrs, Node};
use crate::Unsupported;
use indexmap::IndexMap;
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use serde_json::{json, Value};

pub(crate) const CRITIC_MARKERS: [&str; 6] = ["{==", "{++", "{--", "{~~", "{>>", "{#"];

pub fn block_markdown_to_slate(markdown: &str) -> Result<Vec<Node>, Unsupported> {
    if CRITIC_MARKERS
        .iter()
        .any(|marker| markdown.contains(marker))
    {
        return Err(Unsupported::new("critic markup"));
    }

    block_markdown_to_slate_raw(markdown)
}

/// Convert a single markdown block to Slate nodes without rejecting CriticMarkup
/// markers. The review codec parses those markers literally and rewrites them
/// into review marks afterwards (see `crate::review`); the editor/collab path
/// keeps using `block_markdown_to_slate`, which still rejects them.
pub fn block_markdown_to_slate_raw(markdown: &str) -> Result<Vec<Node>, Unsupported> {
    let events = Parser::new_ext(markdown, browser_compatible_markdown_options())
        .map(|event| event.into_static())
        .collect::<Vec<_>>();
    slate_from_block_events(events)
}

/// Parse an already-collected slice of top-level pulldown events into Slate
/// nodes. Used by `crate::rows` to parse one top-level block at a time so a
/// failing block can fall back to `raw_markdown` without rejecting the rest
/// of the document.
pub(crate) fn slate_from_block_events(
    events: Vec<Event<'static>>,
) -> Result<Vec<Node>, Unsupported> {
    let mut parser = EventParser { events, index: 0 };
    Ok(normalize_insert_nodes(parser.parse_top_level()?))
}

pub(crate) fn browser_compatible_markdown_options() -> Options {
    // Keep this as an explicit allowlist instead of `Options::all()` so new
    // pulldown-cmark extensions cannot silently change the Rust shadow codec.
    //
    // Some enabled extensions still parse to unsupported events below; that is
    // intentional because the browser can also recognize those syntaxes and the
    // injection path must fail closed rather than accept them as plain text.
    //
    // ENABLE_MATH is deliberately absent: the browser's deserializer has no
    // math plugin, so `$` is plain text there — enabling it here made prose
    // with two dollar signs (e.g. "- $0 / +$200" price lists) degrade whole
    // blocks to raw_markdown for a syntax nothing downstream supports.
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_OLD_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_WIKILINKS
}

struct EventParser {
    events: Vec<Event<'static>>,
    index: usize,
}

#[derive(Clone)]
struct ListContext {
    indent: usize,
    ordered: bool,
    next_start: u64,
}

#[derive(Default)]
struct InlineContext {
    marks: Attrs,
}

impl EventParser {
    fn parse_top_level(&mut self) -> Result<Vec<Node>, Unsupported> {
        let mut nodes = Vec::new();
        while let Some(event) = self.next() {
            match event {
                Event::Start(tag) => self.parse_block_start(tag, &mut nodes, None)?,
                Event::Rule => nodes.push(empty_element("hr")),
                Event::Text(text) if text.trim().is_empty() => {}
                Event::SoftBreak | Event::HardBreak => {}
                Event::End(_) => return Err(Unsupported::new("unexpected end tag")),
                Event::Html(html) | Event::InlineHtml(html) if html.trim().is_empty() => {}
                Event::Text(text) => {
                    let children = text_nodes(&text, &InlineContext::default());
                    nodes.push(Node::element("p", Attrs::new(), children));
                }
                other => return Err(Unsupported::new(format!("unexpected event {other:?}"))),
            }
        }
        Ok(nodes)
    }

    fn parse_block_start(
        &mut self,
        tag: Tag<'static>,
        out: &mut Vec<Node>,
        list: Option<ListContext>,
    ) -> Result<(), Unsupported> {
        match tag {
            Tag::Paragraph => {
                let children =
                    self.parse_inline_until(TagEnd::Paragraph, InlineContext::default())?;
                push_paragraph_or_images(out, children, list);
            }
            Tag::Heading { level, .. } => {
                if list.is_some() {
                    return Err(Unsupported::new("heading inside list item"));
                }
                let ty = match level {
                    HeadingLevel::H1 => "h1",
                    HeadingLevel::H2 => "h2",
                    HeadingLevel::H3 => "h3",
                    HeadingLevel::H4 => "h4",
                    HeadingLevel::H5 => "h5",
                    HeadingLevel::H6 => "h6",
                };
                let end = TagEnd::Heading(level);
                let children = self.parse_inline_until(end, InlineContext::default())?;
                out.push(Node::element(ty, Attrs::new(), children));
            }
            Tag::BlockQuote(_) => {
                if list.is_some() {
                    return Err(Unsupported::new("blockquote inside list item"));
                }
                out.push(self.parse_blockquote()?);
            }
            Tag::CodeBlock(kind) => {
                if list.is_some() {
                    return Err(Unsupported::new("code block inside list item"));
                }
                out.push(self.parse_code_block(kind)?);
            }
            Tag::List(start) => {
                self.parse_list(start, out, list.map(|ctx| ctx.indent).unwrap_or(0))?
            }
            Tag::Table(align) => {
                if list.is_some() {
                    return Err(Unsupported::new("table inside list item"));
                }
                out.push(self.parse_table(align)?);
            }
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {
                return Err(Unsupported::new(format!("unsupported block {tag:?}")))
            }
            Tag::Item
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. } => {
                return Err(Unsupported::new(format!(
                    "unexpected inline/container tag {tag:?}"
                )))
            }
        }
        Ok(())
    }

    fn parse_blockquote(&mut self) -> Result<Node, Unsupported> {
        let mut children = Vec::new();
        let mut first_block = true;
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::BlockQuote(_)) => break,
                Event::Start(Tag::Paragraph) => {
                    if !first_block {
                        children.push(Node::text("\n\n", Attrs::new()));
                    }
                    children.extend(
                        self.parse_inline_until(TagEnd::Paragraph, InlineContext::default())?,
                    );
                    first_block = false;
                }
                Event::Start(Tag::Heading { .. })
                | Event::Start(Tag::List(_))
                | Event::Start(Tag::Table(_))
                | Event::Start(Tag::CodeBlock(_))
                | Event::Start(Tag::BlockQuote(_)) => {
                    return Err(Unsupported::new("nested block inside blockquote"))
                }
                Event::Text(text) if text.trim().is_empty() => {}
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported blockquote event {other:?}"
                    )))
                }
            }
        }
        Ok(Node::element("blockquote", Attrs::new(), children))
    }

    fn parse_code_block(&mut self, kind: CodeBlockKind<'static>) -> Result<Node, Unsupported> {
        let mut code = String::new();
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::CodeBlock) => break,
                Event::Text(text) => code.push_str(&text),
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported code event {other:?}"
                    )))
                }
            }
        }
        let code = code.strip_suffix('\n').unwrap_or(&code).to_string();
        let lang = match kind {
            CodeBlockKind::Indented => None,
            CodeBlockKind::Fenced(lang) => lang
                .split_whitespace()
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        };
        if lang.as_deref() == Some("mermaid") {
            return Ok(Node::element(
                "mermaid",
                attrs([("code", json!(code))]),
                vec![empty_text()],
            ));
        }

        let lines = if code.is_empty() {
            vec![String::new()]
        } else {
            code.split('\n').map(str::to_string).collect()
        };
        let children = lines
            .into_iter()
            .map(|line| {
                Node::element(
                    "code_line",
                    Attrs::new(),
                    vec![Node::text(line, Attrs::new())],
                )
            })
            .collect();
        let mut node_attrs = Attrs::new();
        if let Some(lang) = lang {
            node_attrs.insert("lang".to_string(), json!(lang));
        }
        Ok(Node::element("code_block", node_attrs, children))
    }

    fn parse_list(
        &mut self,
        start: Option<u64>,
        out: &mut Vec<Node>,
        parent_indent: usize,
    ) -> Result<(), Unsupported> {
        let ordered = start.is_some();
        let mut ctx = ListContext {
            indent: parent_indent + 1,
            ordered,
            next_start: start.unwrap_or(1),
        };
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::List(_)) => break,
                Event::Start(Tag::Item) => {
                    self.parse_list_item(out, &ctx)?;
                    if ordered {
                        ctx.next_start += 1;
                    }
                }
                Event::Text(text) if text.trim().is_empty() => {}
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported list event {other:?}"
                    )))
                }
            }
        }
        Ok(())
    }

    fn parse_list_item(
        &mut self,
        out: &mut Vec<Node>,
        ctx: &ListContext,
    ) -> Result<(), Unsupported> {
        let mut checked = None;
        let mut had_block = false;
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::Item) => break,
                Event::TaskListMarker(value) => checked = Some(value),
                Event::Start(Tag::Paragraph) => {
                    let mut attrs = list_attrs(ctx, checked);
                    checked = None;
                    let children =
                        self.parse_inline_until(TagEnd::Paragraph, InlineContext::default())?;
                    out.push(Node::element("p", std::mem::take(&mut attrs), children));
                    had_block = true;
                }
                Event::Start(Tag::List(start)) => {
                    self.parse_list(start, out, ctx.indent)?;
                    had_block = true;
                }
                Event::End(TagEnd::List(_)) => {
                    self.back();
                    break;
                }
                Event::Text(_)
                | Event::Code(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Html(_)
                | Event::InlineHtml(_)
                | Event::Start(Tag::Emphasis)
                | Event::Start(Tag::Strong)
                | Event::Start(Tag::Strikethrough)
                | Event::Start(Tag::Superscript)
                | Event::Start(Tag::Subscript)
                | Event::Start(Tag::Link { .. })
                | Event::Start(Tag::Image { .. }) => {
                    self.back();
                    let (children, ended_item) = self.parse_tight_list_item_inline()?;
                    out.push(Node::element("p", list_attrs(ctx, checked), children));
                    checked = None;
                    had_block = true;
                    if ended_item {
                        break;
                    }
                }
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported list item event {other:?}"
                    )))
                }
            }
        }
        if !had_block {
            out.push(Node::element(
                "p",
                list_attrs(ctx, checked),
                vec![empty_text()],
            ));
        }
        Ok(())
    }

    fn parse_table(&mut self, align: Vec<Alignment>) -> Result<Node, Unsupported> {
        let mut rows = Vec::new();
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::Table) => break,
                Event::Start(Tag::TableHead) => {
                    rows.extend(self.parse_table_section(true)?);
                }
                Event::Start(Tag::TableRow) => {
                    rows.push(self.parse_table_row(false)?);
                }
                Event::Text(text) if text.trim().is_empty() => {}
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported table event {other:?}"
                    )))
                }
            }
        }
        Ok(Node::element(
            "table",
            attrs([(
                "align",
                Value::Array(align.into_iter().map(alignment_value).collect()),
            )]),
            rows,
        ))
    }

    fn parse_table_section(&mut self, header: bool) -> Result<Vec<Node>, Unsupported> {
        let mut rows = Vec::new();
        let mut implicit_cells = Vec::new();
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::TableHead) => {
                    if !implicit_cells.is_empty() {
                        rows.push(Node::element("tr", Attrs::new(), implicit_cells));
                    }
                    break;
                }
                Event::Start(Tag::TableRow) => rows.push(self.parse_table_row(header)?),
                Event::Start(Tag::TableCell) => implicit_cells.push(self.parse_table_cell(header)?),
                Event::Text(text) if text.trim().is_empty() => {}
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported table section event {other:?}"
                    )))
                }
            }
        }
        Ok(rows)
    }

    fn parse_table_row(&mut self, header: bool) -> Result<Node, Unsupported> {
        let mut cells = Vec::new();
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::TableRow) => break,
                Event::Start(Tag::TableCell) => cells.push(self.parse_table_cell(header)?),
                Event::Text(text) if text.trim().is_empty() => {}
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported table row event {other:?}"
                    )))
                }
            }
        }
        Ok(Node::element("tr", Attrs::new(), cells))
    }

    fn parse_table_cell(&mut self, header: bool) -> Result<Node, Unsupported> {
        let mut children = Vec::new();
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::TableCell) => break,
                Event::Start(Tag::Paragraph) => children
                    .extend(self.parse_inline_until(TagEnd::Paragraph, InlineContext::default())?),
                Event::Text(text) => children.extend(text_nodes(&text, &InlineContext::default())),
                Event::Start(Tag::Emphasis)
                | Event::Start(Tag::Strong)
                | Event::Start(Tag::Strikethrough)
                | Event::Start(Tag::Superscript)
                | Event::Start(Tag::Subscript)
                | Event::Start(Tag::Link { .. })
                | Event::Start(Tag::Image { .. })
                | Event::Code(_)
                | Event::Html(_)
                | Event::InlineHtml(_) => {
                    self.back();
                    children.extend(
                        self.parse_inline_until(TagEnd::TableCell, InlineContext::default())?,
                    );
                    break;
                }
                Event::SoftBreak => children.push(Node::text(" ", Attrs::new())),
                Event::HardBreak => children.push(Node::text("\n", Attrs::new())),
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported table cell event {other:?}"
                    )))
                }
            }
        }
        if children.is_empty() {
            children.push(empty_text());
        }
        Ok(Node::element(
            if header { "th" } else { "td" },
            Attrs::new(),
            vec![Node::element("p", Attrs::new(), children)],
        ))
    }

    fn parse_inline_until(
        &mut self,
        end: TagEnd,
        context: InlineContext,
    ) -> Result<Vec<Node>, Unsupported> {
        let mut children = Vec::new();
        let mut context = context;
        while let Some(event) = self.next() {
            match event {
                Event::End(found) if found == end => break,
                Event::Text(text) => children.extend(text_nodes(&text, &context)),
                Event::Code(code) => {
                    let mut marks = context.marks.clone();
                    marks.insert("code".to_string(), json!(true));
                    children.push(Node::text(code.to_string(), marks));
                }
                // CommonMark: a soft break is collapsible whitespace, a hard
                // break is a real line break.
                Event::SoftBreak => {
                    children.push(Node::text(" ", context.marks.clone()));
                }
                Event::HardBreak => {
                    children.push(Node::text("\n", context.marks.clone()));
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    if let Some(mark) = opening_inline_mark(&html) {
                        context.marks.insert(mark.to_string(), json!(true));
                    } else if let Some(mark) = closing_inline_mark(&html) {
                        context.marks.shift_remove(mark);
                    } else if !html.trim().is_empty() {
                        return Err(Unsupported::new("inline html"));
                    }
                }
                Event::Start(tag) => match tag {
                    Tag::Emphasis => {
                        let mut marks = context.marks.clone();
                        marks.insert("italic".to_string(), json!(true));
                        children.extend(
                            self.parse_inline_until(TagEnd::Emphasis, InlineContext { marks })?,
                        );
                    }
                    Tag::Strong => {
                        let mut marks = context.marks.clone();
                        marks.insert("bold".to_string(), json!(true));
                        children.extend(
                            self.parse_inline_until(TagEnd::Strong, InlineContext { marks })?,
                        );
                    }
                    Tag::Strikethrough => {
                        let mut marks = context.marks.clone();
                        marks.insert("strikethrough".to_string(), json!(true));
                        children.extend(
                            self.parse_inline_until(
                                TagEnd::Strikethrough,
                                InlineContext { marks },
                            )?,
                        );
                    }
                    Tag::Superscript => {
                        let mut marks = context.marks.clone();
                        marks.insert("superscript".to_string(), json!(true));
                        children.extend(
                            self.parse_inline_until(TagEnd::Superscript, InlineContext { marks })?,
                        );
                    }
                    Tag::Subscript => {
                        let mut marks = context.marks.clone();
                        marks.insert("subscript".to_string(), json!(true));
                        children.extend(
                            self.parse_inline_until(TagEnd::Subscript, InlineContext { marks })?,
                        );
                    }
                    Tag::Link {
                        link_type: LinkType::WikiLink { .. },
                        dest_url,
                        ..
                    } => {
                        let link_children =
                            self.parse_inline_until(TagEnd::Link, InlineContext::default())?;
                        children.push(wikilink_from_link(dest_url.as_ref(), link_children));
                    }
                    Tag::Link { dest_url, .. } => {
                        let link_children = self.parse_inline_until(
                            TagEnd::Link,
                            InlineContext {
                                marks: context.marks.clone(),
                            },
                        )?;
                        children.push(Node::element(
                            "a",
                            attrs([("url", json!(dest_url.to_string()))]),
                            link_children,
                        ));
                    }
                    Tag::Image {
                        link_type: LinkType::WikiLink { .. },
                        dest_url,
                        ..
                    } => {
                        let link_children =
                            self.parse_inline_until(TagEnd::Image, InlineContext::default())?;
                        children.push(wikilink_from_link_with_embed(
                            dest_url.as_ref(),
                            link_children,
                            true,
                        ));
                    }
                    Tag::Image { dest_url, .. } => {
                        let caption =
                            self.parse_inline_until(TagEnd::Image, InlineContext::default())?;
                        children.push(Node::element(
                            "img",
                            attrs([
                                ("caption", serialize_caption_nodes(caption)?),
                                ("url", json!(dest_url.to_string())),
                            ]),
                            vec![empty_text()],
                        ));
                    }
                    other => {
                        return Err(Unsupported::new(format!(
                            "unsupported inline start {other:?}"
                        )))
                    }
                },
                Event::End(found) => {
                    return Err(Unsupported::new(format!(
                        "unexpected inline end {found:?}, expected {end:?}"
                    )))
                }
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported inline event {other:?}"
                    )))
                }
            }
        }
        if children.is_empty() {
            children.push(empty_text());
        }
        Ok(children)
    }

    fn next(&mut self) -> Option<Event<'static>> {
        let event = self.events.get(self.index).cloned();
        self.index += usize::from(event.is_some());
        event
    }

    fn back(&mut self) {
        self.index = self.index.saturating_sub(1);
    }

    fn parse_tight_list_item_inline(&mut self) -> Result<(Vec<Node>, bool), Unsupported> {
        let mut children = Vec::new();
        let mut context = InlineContext::default();
        let mut ended_item = false;
        while let Some(event) = self.next() {
            match event {
                Event::End(TagEnd::Item) => {
                    ended_item = true;
                    break;
                }
                Event::Start(Tag::List(_)) | Event::End(TagEnd::List(_)) => {
                    self.back();
                    break;
                }
                Event::Text(text) => children.extend(text_nodes(&text, &context)),
                Event::Code(code) => {
                    let mut marks = context.marks.clone();
                    marks.insert("code".to_string(), json!(true));
                    children.push(Node::text(code.to_string(), marks));
                }
                Event::SoftBreak => {
                    children.push(Node::text(" ", context.marks.clone()));
                }
                Event::HardBreak => {
                    children.push(Node::text("\n", context.marks.clone()));
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    if let Some(mark) = opening_inline_mark(&html) {
                        context.marks.insert(mark.to_string(), json!(true));
                    } else if let Some(mark) = closing_inline_mark(&html) {
                        context.marks.shift_remove(mark);
                    } else if !html.trim().is_empty() {
                        return Err(Unsupported::new("inline html"));
                    }
                }
                Event::Start(Tag::Emphasis) => {
                    let mut marks = context.marks.clone();
                    marks.insert("italic".to_string(), json!(true));
                    children.extend(
                        self.parse_inline_until(TagEnd::Emphasis, InlineContext { marks })?,
                    );
                }
                Event::Start(Tag::Strong) => {
                    let mut marks = context.marks.clone();
                    marks.insert("bold".to_string(), json!(true));
                    children
                        .extend(self.parse_inline_until(TagEnd::Strong, InlineContext { marks })?);
                }
                Event::Start(Tag::Strikethrough) => {
                    let mut marks = context.marks.clone();
                    marks.insert("strikethrough".to_string(), json!(true));
                    children.extend(
                        self.parse_inline_until(TagEnd::Strikethrough, InlineContext { marks })?,
                    );
                }
                Event::Start(Tag::Superscript) => {
                    let mut marks = context.marks.clone();
                    marks.insert("superscript".to_string(), json!(true));
                    children.extend(
                        self.parse_inline_until(TagEnd::Superscript, InlineContext { marks })?,
                    );
                }
                Event::Start(Tag::Subscript) => {
                    let mut marks = context.marks.clone();
                    marks.insert("subscript".to_string(), json!(true));
                    children.extend(
                        self.parse_inline_until(TagEnd::Subscript, InlineContext { marks })?,
                    );
                }
                Event::Start(Tag::Link {
                    link_type: LinkType::WikiLink { .. },
                    dest_url,
                    ..
                }) => {
                    let link_children =
                        self.parse_inline_until(TagEnd::Link, InlineContext::default())?;
                    children.push(wikilink_from_link(dest_url.as_ref(), link_children));
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let link_children = self.parse_inline_until(
                        TagEnd::Link,
                        InlineContext {
                            marks: context.marks.clone(),
                        },
                    )?;
                    children.push(Node::element(
                        "a",
                        attrs([("url", json!(dest_url.to_string()))]),
                        link_children,
                    ));
                }
                Event::Start(Tag::Image {
                    link_type: LinkType::WikiLink { .. },
                    dest_url,
                    ..
                }) => {
                    let link_children =
                        self.parse_inline_until(TagEnd::Image, InlineContext::default())?;
                    children.push(wikilink_from_link_with_embed(
                        dest_url.as_ref(),
                        link_children,
                        true,
                    ));
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    let caption =
                        self.parse_inline_until(TagEnd::Image, InlineContext::default())?;
                    children.push(Node::element(
                        "img",
                        attrs([
                            ("caption", serialize_caption_nodes(caption)?),
                            ("url", json!(dest_url.to_string())),
                        ]),
                        vec![empty_text()],
                    ));
                }
                other => {
                    return Err(Unsupported::new(format!(
                        "unsupported tight list item event {other:?}"
                    )))
                }
            }
        }
        if children.is_empty() {
            children.push(empty_text());
        }
        Ok((children, ended_item))
    }
}

fn push_paragraph_or_images(out: &mut Vec<Node>, children: Vec<Node>, list: Option<ListContext>) {
    let mut paragraph = Vec::new();
    let push_paragraph = |out: &mut Vec<Node>, paragraph: &mut Vec<Node>| {
        if !paragraph.is_empty() {
            out.push(Node::element("p", Attrs::new(), std::mem::take(paragraph)));
        }
    };

    for child in children {
        if matches!(&child, Node::Element { ty, .. } if ty == "img") && list.is_none() {
            push_paragraph(out, &mut paragraph);
            out.push(child);
        } else {
            paragraph.push(child);
        }
    }
    if paragraph.is_empty() {
        if out.is_empty() {
            out.push(Node::element(
                "p",
                list.map(|ctx| list_attrs(&ctx, None)).unwrap_or_default(),
                vec![empty_text()],
            ));
        }
    } else {
        out.push(Node::element(
            "p",
            list.map(|ctx| list_attrs(&ctx, None)).unwrap_or_default(),
            paragraph,
        ));
    }
}

fn list_attrs(ctx: &ListContext, checked: Option<bool>) -> Attrs {
    let mut attrs = IndexMap::new();
    attrs.insert("indent".to_string(), json!(ctx.indent));
    if let Some(checked) = checked {
        attrs.insert("listStyleType".to_string(), json!("todo"));
        attrs.insert("checked".to_string(), json!(checked));
    } else if ctx.ordered {
        attrs.insert("listStyleType".to_string(), json!("decimal"));
        attrs.insert("listStart".to_string(), json!(ctx.next_start));
    } else {
        attrs.insert("listStyleType".to_string(), json!("disc"));
    }
    attrs
}

fn serialize_caption_nodes(caption: Vec<Node>) -> Result<Value, Unsupported> {
    serde_json::to_value(caption)
        .map_err(|error| Unsupported::new(format!("image caption serialization failed: {error}")))
}

fn text_nodes(text: &str, context: &InlineContext) -> Vec<Node> {
    if text == "\u{200b}" {
        return Vec::new();
    }
    if context.marks.get("code") == Some(&json!(true)) {
        return vec![Node::text(text, context.marks.clone())];
    }

    let mut nodes = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let before = &rest[..start];
        if !before.is_empty() {
            nodes.push(Node::text(before, context.marks.clone()));
        }
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("]]") else {
            nodes.push(Node::text(&rest[start..], context.marks.clone()));
            return nodes;
        };
        let raw = &after_start[..end];
        if let Some(wikilink) = wikilink_node(raw, false) {
            nodes.push(wikilink);
        } else {
            nodes.push(Node::text(
                &rest[start..start + end + 4],
                context.marks.clone(),
            ));
        }
        rest = &after_start[end + 2..];
    }
    if !rest.is_empty() {
        nodes.push(Node::text(rest, context.marks.clone()));
    }
    nodes
}

fn wikilink_node(raw: &str, embed: bool) -> Option<Node> {
    let (target_and_anchor, alias) = raw.split_once('|').map_or((raw, None), |(target, alias)| {
        (target, Some(alias.trim().to_string()))
    });
    let (target, anchor) = target_and_anchor
        .split_once('#')
        .map_or((target_and_anchor, None), |(target, anchor)| {
            (target, Some(anchor.trim().to_string()))
        });
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let mut attrs = attrs([("target", json!(target))]);
    if let Some(anchor) = anchor.filter(|value| !value.is_empty()) {
        attrs.insert("anchor".to_string(), json!(anchor));
    }
    if let Some(alias) = alias.filter(|value| !value.is_empty()) {
        attrs.insert("alias".to_string(), json!(alias));
    }
    if embed {
        attrs.insert("embed".to_string(), json!(true));
    }
    Some(Node::element("wikilink", attrs, vec![empty_text()]))
}

fn wikilink_from_link(dest_url: &str, children: Vec<Node>) -> Node {
    wikilink_from_link_with_embed(dest_url, children, false)
}

fn wikilink_from_link_with_embed(dest_url: &str, children: Vec<Node>, embed: bool) -> Node {
    let display = plain_text(&children);
    let (target, anchor) = dest_url
        .split_once('#')
        .map_or((dest_url, None), |(target, anchor)| {
            (target, Some(anchor.trim().to_string()))
        });
    let mut attrs = attrs([("target", json!(target.trim()))]);
    if let Some(anchor) = anchor.filter(|value| !value.is_empty()) {
        attrs.insert("anchor".to_string(), json!(anchor));
    }
    if !display.is_empty() && display != target {
        attrs.insert("alias".to_string(), json!(display));
    }
    if embed {
        attrs.insert("embed".to_string(), json!(true));
    }
    Node::element("wikilink", attrs, vec![empty_text()])
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

fn alignment_value(align: Alignment) -> Value {
    match align {
        Alignment::None => Value::Null,
        Alignment::Left => json!("left"),
        Alignment::Center => json!("center"),
        Alignment::Right => json!("right"),
    }
}

fn opening_inline_mark(html: &str) -> Option<&'static str> {
    match html.trim().to_ascii_lowercase().as_str() {
        "<u>" => Some("underline"),
        "<sub>" => Some("subscript"),
        "<sup>" => Some("superscript"),
        _ => None,
    }
}

fn closing_inline_mark(html: &str) -> Option<&'static str> {
    match html.trim().to_ascii_lowercase().as_str() {
        "</u>" => Some("underline"),
        "</sub>" => Some("subscript"),
        "</sup>" => Some("superscript"),
        _ => None,
    }
}

fn empty_element(ty: &str) -> Node {
    Node::element(ty, Attrs::new(), vec![empty_text()])
}

fn empty_text() -> Node {
    Node::text("", Attrs::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_options_are_explicitly_allowlisted() {
        let expected = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
            | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
            | Options::ENABLE_OLD_FOOTNOTES
            | Options::ENABLE_GFM
            | Options::ENABLE_DEFINITION_LIST
            | Options::ENABLE_SUPERSCRIPT
            | Options::ENABLE_SUBSCRIPT
            | Options::ENABLE_WIKILINKS;

        let actual = browser_compatible_markdown_options();
        assert_eq!(actual, expected);
        assert!(!actual.contains(Options::ENABLE_SMART_PUNCTUATION));
        assert!(!actual.contains(Options::ENABLE_HEADING_ATTRIBUTES));
        // The browser's deserializer has no math plugin; `$` must stay plain
        // text so dollar amounts never degrade blocks to raw_markdown.
        assert!(!actual.contains(Options::ENABLE_MATH));
    }
}
