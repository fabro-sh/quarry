use crate::{
    QuarryStore, StoreEvent, map_turso_error, metadata_aliases, row::link_from_row, title_for_entry,
};

use quarry_core::{
    Document, DocumentLink, DocumentListEntry, GraphEdge, GraphNode, GraphResponse, LinkCollection,
    ReindexReport, Result, normalize_path, now_timestamp,
};
use std::collections::{HashMap, HashSet, VecDeque};
use turso::{Connection, Rows, Value, params};

impl QuarryStore {
    pub async fn reindex_library(&self, library: &str) -> Result<ReindexReport> {
        let library = library.to_string();
        let (library_id, report) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    let indexed_documents = store.reindex_links_conn(conn, &library.id).await?;
                    Ok((
                        library_id,
                        ReindexReport {
                            ok: true,
                            indexed_documents,
                        },
                    ))
                })
            })
            .await?;
        self.emit_event(StoreEvent::library_reindexed(library_id));
        Ok(report)
    }

    pub async fn outgoing_links(&self, library: &str, path: &str) -> Result<LinkCollection> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document = self.document_entry_conn(&conn, &library.id, &path).await?;
        Ok(LinkCollection {
            path: document.path.clone(),
            links: self
                .links_for_source_conn(&conn, &library.id, &document.id)
                .await?,
        })
    }

    pub async fn backlinks(&self, library: &str, path: &str) -> Result<LinkCollection> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let target = self.document_entry_conn(&conn, &library.id, &path).await?;
        Ok(LinkCollection {
            path: target.path,
            links: self
                .links_for_target_conn(&conn, &library.id, &target.id)
                .await?,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "link graph query exposes independent filters"
    )]
    pub async fn graph(
        &self,
        library: &str,
        root: Option<&str>,
        depth: Option<u64>,
        limit: Option<u64>,
        folder: Option<&str>,
        tag: Option<&str>,
        link_kind: Option<&str>,
        resolved: Option<bool>,
    ) -> Result<GraphResponse> {
        let limit = limit.unwrap_or(500).min(10_000) as usize;
        let depth = depth.unwrap_or(1).min(32);
        let root = root.map(normalize_path).transpose()?;
        let folder = folder
            .map(normalize_graph_folder)
            .transpose()?
            .filter(|folder| !folder.is_empty());
        let tag = tag.map(normalize_graph_tag).filter(|tag| !tag.is_empty());
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library.id, 10_000)
            .await?;
        let document_by_id: HashMap<String, &DocumentListEntry> = documents
            .iter()
            .map(|entry| (entry.id.to_string(), entry))
            .collect();
        let mut node_map: HashMap<String, GraphNode> = HashMap::new();
        let mut edges = Vec::new();
        let mut candidate_nodes = 0usize;

        let mut add_node = |entry: &DocumentListEntry| {
            if node_map.contains_key(entry.id.as_str()) {
                return;
            }
            candidate_nodes += 1;
            if node_map.len() < limit {
                node_map.insert(entry.id.to_string(), graph_node_from_entry(entry));
            }
        };

        let document_matches_folder = |entry: &DocumentListEntry| {
            folder
                .as_deref()
                .is_none_or(|folder| path_is_in_folder(&entry.path, folder))
        };
        let link_matches_folder = |link: &DocumentLink| {
            folder.as_deref().is_none_or(|folder| {
                path_is_in_folder(&link.src_path, folder)
                    && link
                        .target_path
                        .as_deref()
                        .is_none_or(|path| path_is_in_folder(path, folder))
            })
        };
        let link_matches_tag = |link: &DocumentLink| {
            tag.as_deref().is_none_or(|tag| {
                link.target_kind == "tag" && link.target_text.eq_ignore_ascii_case(tag)
            })
        };
        let links: Vec<DocumentLink> = self
            .links_for_library_conn(&conn, &library.id)
            .await?
            .into_iter()
            .filter(|link| {
                link_kind.is_none_or(|kind| link.target_kind == kind)
                    && resolved.is_none_or(|expected| link.resolved == expected)
                    && link_matches_folder(link)
                    && link_matches_tag(link)
            })
            .collect();
        let mut included_ids: HashSet<String> = HashSet::new();
        let has_edge_filter = link_kind.is_some() || resolved.is_some() || tag.is_some();

        if root.is_none() {
            if has_edge_filter {
                for link in &links {
                    if let Some(source) = document_by_id.get(link.src_doc_id.as_str())
                        && document_matches_folder(source)
                        && included_ids.insert(source.id.to_string())
                    {
                        add_node(source);
                    }
                    if let Some(target_id) = link.target_doc_id.as_deref()
                        && let Some(target) = document_by_id.get(target_id)
                        && document_matches_folder(target)
                        && included_ids.insert(target.id.to_string())
                    {
                        add_node(target);
                    }
                }
            } else {
                for entry in &documents {
                    if document_matches_folder(entry) {
                        included_ids.insert(entry.id.to_string());
                        add_node(entry);
                    }
                }
            }
        } else if let Some(root_path) = root.as_deref()
            && let Some(root_entry) = documents.iter().find(|entry| entry.path == root_path)
            && document_matches_folder(root_entry)
        {
            included_ids.insert(root_entry.id.to_string());
            add_node(root_entry);
            let mut queue = VecDeque::from([(root_entry.id.to_string(), 0u64)]);
            while let Some((document_id, distance)) = queue.pop_front() {
                if distance >= depth {
                    continue;
                }
                for link in &links {
                    let neighbor_id = if link.src_doc_id.as_str() == document_id {
                        link.target_doc_id.as_deref()
                    } else if link.target_doc_id.as_deref() == Some(document_id.as_str()) {
                        Some(link.src_doc_id.as_str())
                    } else {
                        None
                    };
                    let Some(neighbor_id) = neighbor_id else {
                        continue;
                    };
                    if included_ids.insert(neighbor_id.to_string())
                        && let Some(neighbor) = document_by_id.get(neighbor_id)
                    {
                        add_node(neighbor);
                        queue.push_back((neighbor_id.to_string(), distance + 1));
                    }
                }
            }
        }

        for link in links {
            if root.is_some() || folder.is_some() || has_edge_filter {
                let source_included = included_ids.contains(link.src_doc_id.as_str());
                let target_included = link
                    .target_doc_id
                    .as_deref()
                    .is_some_and(|target_id| included_ids.contains(target_id));
                if link.target_doc_id.is_some() {
                    if !source_included || !target_included {
                        continue;
                    }
                } else if !source_included {
                    continue;
                }
            }
            edges.push(GraphEdge {
                id: format!(
                    "{}:{}:{}",
                    link.src_doc_id, link.start_offset, link.end_offset
                ),
                source: link.src_doc_id,
                source_path: link.src_path,
                target: link.target_doc_id,
                target_path: link.target_path,
                target_kind: link.target_kind,
                target_text: link.target_text,
                resolved: link.resolved,
                resolution_status: link.resolution_status,
            });
        }

        let truncated = candidate_nodes > limit;
        let nodes = node_map.into_values().collect();
        Ok(GraphResponse {
            nodes,
            edges,
            truncated,
        })
    }

    pub(crate) async fn reindex_links_conn(
        &self,
        conn: &Connection,
        library_id: &str,
    ) -> Result<usize> {
        let documents = self
            .document_entries_for_library_conn(conn, library_id, 10_000)
            .await?;

        conn.execute(
            "DELETE FROM links WHERE library_id = ?1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
        conn.execute(
            "DELETE FROM aliases WHERE library_id = ?1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;

        for document in &documents {
            for alias in metadata_aliases(&document.metadata) {
                if alias.trim().is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO aliases (library_id, doc_id, alias, alias_source)
                     VALUES (?1, ?2, ?3, 'metadata')",
                    params![
                        library_id.to_string(),
                        document.id.clone(),
                        alias.trim().to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
            }
        }

        for entry in &documents {
            if !is_textual_content_type(&entry.content_type) {
                continue;
            }
            let document = self.document_conn(conn, library_id, &entry.path).await?;
            for link in extract_links_for_document(&document, &documents) {
                insert_link_conn(conn, library_id, &link).await?;
            }
        }

        Ok(documents.len())
    }

    pub(crate) async fn links_for_source_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        source_doc_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?3)
                 WHERE l.library_id = ?1
                   AND l.src_doc_id = ?2
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?3)
                 ORDER BY l.start_offset, l.end_offset, l.target_kind",
                params![
                    library_id.to_string(),
                    source_doc_id.to_string(),
                    now_timestamp()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }

    async fn links_for_target_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        target_doc_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?3)
                 WHERE l.library_id = ?1
                   AND l.target_doc_id = ?2
                   AND l.target_kind <> 'heading'
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?3)
                 ORDER BY l.start_offset, l.end_offset, l.target_kind",
                params![
                    library_id.to_string(),
                    target_doc_id.to_string(),
                    now_timestamp()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }

    async fn links_for_library_conn(
        &self,
        conn: &Connection,
        library_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?2)
                 WHERE l.library_id = ?1
                   AND l.target_kind <> 'heading'
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?2)
                 ORDER BY sd.path, l.start_offset, l.end_offset, l.target_kind",
                params![library_id.to_string(), now_timestamp()],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }
}

fn graph_node_from_entry(entry: &DocumentListEntry) -> GraphNode {
    GraphNode {
        id: entry.id.clone(),
        path: entry.path.clone(),
        title: title_for_entry(entry),
        content_type: entry.content_type.clone(),
    }
}

pub(crate) fn is_textual_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/markdown"
                | "application/x-markdown"
                | "application/yaml"
                | "application/x-yaml"
        )
}

fn extract_links_for_document(
    document: &Document,
    documents: &[DocumentListEntry],
) -> Vec<DocumentLink> {
    if !is_textual_content_type(&document.version.content_type) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&document.content);
    let mut links = Vec::new();
    extract_headings(&text, document, &mut links);
    extract_wikilinks(&text, document, documents, &mut links);
    extract_markdown_links(&text, document, documents, &mut links);
    extract_tags(&text, document, &mut links);
    links.sort_by_key(|link| link.start_offset);
    links
}

fn extract_headings(text: &str, document: &Document, links: &mut Vec<DocumentLink>) {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_body = line.trim_end_matches(['\r', '\n']);
        let trimmed_start = line_body.trim_start();
        let leading_whitespace = line_body.len() - trimmed_start.len();
        let heading_marks = trimmed_start
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b'#')
            .count();
        if !(1..=6).contains(&heading_marks) {
            offset += line.len();
            continue;
        }
        let after_marks = &trimmed_start[heading_marks..];
        if !after_marks.starts_with(' ') && !after_marks.starts_with('\t') {
            offset += line.len();
            continue;
        }
        let content_start_in_after_marks = after_marks.len() - after_marks.trim_start().len();
        let raw_text = after_marks.trim();
        let heading_text = raw_text.trim_end_matches('#').trim();
        if heading_text.is_empty() {
            offset += line.len();
            continue;
        }
        let start_offset =
            offset + leading_whitespace + heading_marks + content_start_in_after_marks;
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: "heading".to_string(),
            target_text: heading_text.to_string(),
            target_doc_id: Some(document.id.clone()),
            target_path: Some(document.path.clone()),
            target_anchor: Some(slugify_heading(heading_text)),
            alias: None,
            start_offset,
            end_offset: start_offset + heading_text.len(),
            resolved: true,
            resolution_status: "resolved".to_string(),
        });
        offset += line.len();
    }
}

fn extract_wikilinks(
    text: &str,
    document: &Document,
    documents: &[DocumentListEntry],
    links: &mut Vec<DocumentLink>,
) {
    let mut search_start = 0;
    while let Some(open_rel) = text[search_start..].find("[[") {
        let open = search_start + open_rel;
        let Some(close_rel) = text[open + 2..].find("]]") else {
            break;
        };
        let close = open + 2 + close_rel;
        let inner = &text[open + 2..close];
        let is_embed = open > 0 && text.as_bytes()[open - 1] == b'!';
        let start_offset = if is_embed { open - 1 } else { open };
        let (target_text, alias) = split_alias(inner);
        let (lookup_target, target_anchor) = split_anchor(&target_text);
        let resolution = resolve_link_target(&lookup_target, documents);
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: if is_embed { "embed" } else { "wiki_link" }.to_string(),
            target_text: lookup_target,
            target_doc_id: resolution.target.map(|entry| entry.id.clone()),
            target_path: resolution.target.map(|entry| entry.path.clone()),
            target_anchor,
            alias,
            start_offset,
            end_offset: close + 2,
            resolved: resolution.target.is_some(),
            resolution_status: resolution.status.to_string(),
        });
        search_start = close + 2;
    }
}

fn extract_markdown_links(
    text: &str,
    document: &Document,
    documents: &[DocumentListEntry],
    links: &mut Vec<DocumentLink>,
) {
    let mut search_start = 0;
    while let Some(open_rel) = text[search_start..].find('[') {
        let open = search_start + open_rel;
        if text[open..].starts_with("[[") {
            search_start = open + 2;
            continue;
        }
        let Some(label_end_rel) = text[open + 1..].find("](") else {
            search_start = open + 1;
            continue;
        };
        let target_start = open + 1 + label_end_rel + 2;
        let Some(close_rel) = text[target_start..].find(')') else {
            break;
        };
        let close = target_start + close_rel;
        let target = text[target_start..close].trim();
        if target.is_empty() {
            search_start = close + 1;
            continue;
        }
        let (lookup_target, target_anchor) = split_anchor(target);
        let resolution = if is_external_link(&lookup_target) || lookup_target.starts_with('#') {
            LinkResolution::external()
        } else {
            resolve_link_target(&lookup_target, documents)
        };
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: "markdown_link".to_string(),
            target_text: lookup_target,
            target_doc_id: resolution.target.map(|entry| entry.id.clone()),
            target_path: resolution.target.map(|entry| entry.path.clone()),
            target_anchor,
            alias: None,
            start_offset: open,
            end_offset: close + 1,
            resolved: resolution.target.is_some(),
            resolution_status: resolution.status.to_string(),
        });
        search_start = close + 1;
    }
}

fn extract_tags(text: &str, document: &Document, links: &mut Vec<DocumentLink>) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let previous = index.checked_sub(1).map(|idx| bytes[idx] as char);
        if previous.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == ']') {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < bytes.len() {
            let ch = bytes[end] as char;
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '/' {
                end += 1;
            } else {
                break;
            }
        }
        if end > index + 1 {
            let tag = text[index + 1..end].to_string();
            links.push(DocumentLink {
                src_doc_id: document.id.clone(),
                src_version_id: document.version.id.clone(),
                src_path: document.path.clone(),
                target_kind: "tag".to_string(),
                target_text: tag,
                target_doc_id: None,
                target_path: None,
                target_anchor: None,
                alias: None,
                start_offset: index,
                end_offset: end,
                resolved: false,
                resolution_status: "unresolved".to_string(),
            });
        }
        index = end.max(index + 1);
    }
}

fn split_alias(text: &str) -> (String, Option<String>) {
    let (target, alias) = text
        .split_once('|')
        .map(|(target, alias)| (target, Some(alias)))
        .unwrap_or((text, None));
    (
        target.trim().to_string(),
        alias
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(ToOwned::to_owned),
    )
}

fn split_anchor(target: &str) -> (String, Option<String>) {
    if let Some((path, anchor)) = target.split_once('#') {
        return (
            path.trim().to_string(),
            Some(anchor.trim().trim_start_matches('#').to_string()),
        );
    }
    if let Some((path, anchor)) = target.split_once('^') {
        return (
            path.trim().to_string(),
            Some(format!("^{}", anchor.trim().trim_start_matches('^'))),
        );
    }
    (target.trim().to_string(), None)
}

fn is_external_link(target: &str) -> bool {
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
}

struct LinkResolution<'a> {
    target: Option<&'a DocumentListEntry>,
    status: &'static str,
}

impl<'a> LinkResolution<'a> {
    fn resolved(target: &'a DocumentListEntry) -> Self {
        Self {
            target: Some(target),
            status: "resolved",
        }
    }

    fn unresolved() -> Self {
        Self {
            target: None,
            status: "unresolved",
        }
    }

    fn ambiguous() -> Self {
        Self {
            target: None,
            status: "ambiguous",
        }
    }

    /// The link does not reference a library document: an external URL
    /// (`https://…`, `mailto:`) or a same-document anchor (`#section`, empty target).
    fn external() -> Self {
        Self {
            target: None,
            status: "external",
        }
    }
}

fn resolve_link_target<'a>(target: &str, documents: &'a [DocumentListEntry]) -> LinkResolution<'a> {
    let normalized = target.trim().trim_start_matches('/');
    if normalized.is_empty() {
        // No document target intended (e.g. a bare `#anchor` or empty `[[]]`).
        return LinkResolution::external();
    }
    let normalized_lc = normalized.to_lowercase();
    let normalized_md_lc = format!("{normalized_lc}.md");
    let normalized_without_ext = strip_markdown_extension(&normalized_lc);
    let mut candidates: Vec<(&DocumentListEntry, u8)> = documents
        .iter()
        .filter_map(|entry| {
            let path_lc = entry.path.to_lowercase();
            let path_without_ext = strip_markdown_extension(&path_lc);
            let file_name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            let file_stem_lc = strip_markdown_extension(&file_name.to_lowercase());
            let rank = if path_lc == normalized_lc {
                0
            } else if path_lc == normalized_md_lc {
                1
            } else if path_without_ext == normalized_without_ext {
                2
            } else if file_stem_lc == normalized_without_ext {
                3
            } else if metadata_aliases(&entry.metadata)
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(normalized))
            {
                4
            } else {
                return None;
            };
            Some((entry, rank))
        })
        .collect();
    candidates.sort_by(|(a, a_rank), (b, b_rank)| {
        a_rank.cmp(b_rank).then_with(|| {
            a.path
                .len()
                .cmp(&b.path.len())
                .then_with(|| a.path.cmp(&b.path))
        })
    });
    let Some((first, rank)) = candidates.first().copied() else {
        return LinkResolution::unresolved();
    };
    let shortest_path_len = first.path.len();
    let ambiguous = candidates.iter().skip(1).any(|(entry, candidate_rank)| {
        *candidate_rank == rank && (rank == 4 || entry.path.len() == shortest_path_len)
    });
    if ambiguous {
        LinkResolution::ambiguous()
    } else {
        LinkResolution::resolved(first)
    }
}

fn strip_markdown_extension(path: &str) -> String {
    path.strip_suffix(".md")
        .or_else(|| path.strip_suffix(".markdown"))
        .unwrap_or(path)
        .to_string()
}

fn slugify_heading(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lowercase in ch.to_lowercase() {
                slug.push(lowercase);
            }
            last_was_dash = false;
        } else if !slug.is_empty() && !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    if last_was_dash {
        slug.pop();
    }
    slug
}

fn normalize_graph_folder(folder: &str) -> Result<String> {
    let trimmed = folder.trim().trim_matches('/');
    if trimmed.is_empty() {
        Ok(String::new())
    } else {
        normalize_path(trimmed)
    }
}

fn normalize_graph_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_string()
}

fn path_is_in_folder(path: &str, folder: &str) -> bool {
    path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

async fn insert_link_conn(conn: &Connection, library_id: &str, link: &DocumentLink) -> Result<()> {
    conn.execute(
        "INSERT INTO links
         (library_id, src_doc_id, src_version_id, target_kind, target_text, target_doc_id,
          target_anchor, start_offset, end_offset, alias, resolution_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        vec![
            Value::Text(library_id.to_string()),
            Value::Text(link.src_doc_id.to_string()),
            Value::Text(link.src_version_id.to_string()),
            Value::Text(link.target_kind.clone()),
            Value::Text(link.target_text.clone()),
            opt_value(link.target_doc_id.as_ref().map(ToString::to_string)),
            opt_value(link.target_anchor.clone()),
            Value::Integer(link.start_offset as i64),
            Value::Integer(link.end_offset as i64),
            opt_value(link.alias.clone()),
            Value::Text(link.resolution_status.clone()),
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn links_from_rows(rows: &mut Rows) -> Result<Vec<DocumentLink>> {
    let mut links = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        links.push(link_from_row(&row)?);
    }
    Ok(links)
}

fn opt_value<T>(value: Option<T>) -> Value
where
    T: Into<Value>,
{
    value.map(Into::into).unwrap_or(Value::Null)
}
