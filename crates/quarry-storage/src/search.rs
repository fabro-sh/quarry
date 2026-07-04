use crate::{QuarryStore, is_textual_content_type, metadata_aliases, title_for_entry};

use quarry_core::{Result, SearchResponse, SearchResult, SearchSuggestion};

impl QuarryStore {
    pub async fn search_documents(
        &self,
        library: &str,
        query: &str,
        limit: Option<u64>,
    ) -> Result<SearchResponse> {
        let query = query.trim();
        let query_lc = query.to_lowercase();
        let tag_query_lc = query.trim_start_matches('#').to_lowercase();
        let limit = limit.unwrap_or(50).min(100) as usize;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library_record.id, 10_000)
            .await?;
        let mut results = Vec::new();

        for entry in documents {
            let title = title_for_entry(&entry);
            let mut matched_fields = Vec::new();
            let mut score = 0.0;
            let mut snippet = None;

            if query.is_empty() || entry.path.to_lowercase().contains(&query_lc) {
                push_unique(&mut matched_fields, "path");
                score += 3.0;
            }
            if query.is_empty() || title.to_lowercase().contains(&query_lc) {
                push_unique(&mut matched_fields, "title");
                score += 2.0;
            }
            if !query.is_empty()
                && metadata_aliases(&entry.metadata)
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query_lc))
            {
                push_unique(&mut matched_fields, "alias");
                score += 2.5;
            }
            if !query.is_empty() && is_textual_content_type(&entry.content_type) {
                let document = self.get_document(library, &entry.path).await?;
                let body = String::from_utf8_lossy(&document.content);
                if let Some(index) = body.to_lowercase().find(&query_lc) {
                    push_unique(&mut matched_fields, "body");
                    score += 1.0;
                    snippet = Some(make_snippet(&body, index, query.len()));
                }
            }
            if !tag_query_lc.is_empty() {
                let tag_match = self
                    .links_for_source_conn(&conn, &library_record.id, &entry.id)
                    .await?
                    .into_iter()
                    .filter(|link| link.target_kind == "tag")
                    .any(|link| link.target_text.to_lowercase().contains(&tag_query_lc));
                if tag_match {
                    push_unique(&mut matched_fields, "tag");
                    score += 2.5;
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    document_id: entry.id,
                    path: entry.path,
                    title,
                    content_type: entry.content_type,
                    score,
                    snippet,
                    matched_fields,
                    head_version_id: entry.head_version_id,
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        results.truncate(limit);

        Ok(SearchResponse {
            results,
            cursor: None,
        })
    }

    pub async fn suggest_documents(
        &self,
        library: &str,
        query: &str,
        limit: Option<u64>,
    ) -> Result<Vec<SearchSuggestion>> {
        let query_lc = query.trim().to_lowercase();
        let limit = limit.unwrap_or(20).min(100) as usize;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library.id, 10_000)
            .await?;
        let mut suggestions = Vec::new();

        for entry in documents {
            let title = title_for_entry(&entry);
            let title_match = query_lc.is_empty() || title.to_lowercase().contains(&query_lc);
            let path_match = query_lc.is_empty() || entry.path.to_lowercase().contains(&query_lc);
            if title_match || path_match {
                suggestions.push(SearchSuggestion {
                    path: entry.path.clone(),
                    title,
                    match_type: if title_match { "title" } else { "path" }.to_string(),
                    head_version_id: entry.head_version_id.clone(),
                    matched_text: Some(if title_match {
                        title_for_entry(&entry)
                    } else {
                        entry.path.clone()
                    }),
                    target_anchor: None,
                });
            }

            for alias in metadata_aliases(&entry.metadata) {
                if query_lc.is_empty() || alias.to_lowercase().contains(&query_lc) {
                    suggestions.push(SearchSuggestion {
                        path: entry.path.clone(),
                        title: title_for_entry(&entry),
                        match_type: "alias".to_string(),
                        head_version_id: entry.head_version_id.clone(),
                        matched_text: Some(alias),
                        target_anchor: None,
                    });
                }
            }

            if is_textual_content_type(&entry.content_type) {
                for link in self
                    .links_for_source_conn(&conn, &library.id, &entry.id)
                    .await?
                    .into_iter()
                    .filter(|link| link.target_kind == "heading")
                {
                    if query_lc.is_empty() || link.target_text.to_lowercase().contains(&query_lc) {
                        suggestions.push(SearchSuggestion {
                            path: entry.path.clone(),
                            title: title_for_entry(&entry),
                            match_type: "heading".to_string(),
                            head_version_id: entry.head_version_id.clone(),
                            matched_text: Some(link.target_text.clone()),
                            target_anchor: Some(link.target_text),
                        });
                    }
                }
            }
        }

        suggestions.sort_by(|a, b| {
            suggestion_match_rank(&a.match_type)
                .cmp(&suggestion_match_rank(&b.match_type))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.matched_text.cmp(&b.matched_text))
        });
        suggestions.truncate(limit);
        Ok(suggestions)
    }
}

fn push_unique(fields: &mut Vec<String>, field: &str) {
    if !fields.iter().any(|existing| existing == field) {
        fields.push(field.to_string());
    }
}

fn suggestion_match_rank(match_type: &str) -> u8 {
    match match_type {
        "title" => 0,
        "path" => 1,
        "alias" => 2,
        "heading" => 3,
        _ => 4,
    }
}

fn make_snippet(text: &str, index: usize, query_len: usize) -> String {
    let mut start = index.saturating_sub(60);
    let mut end = (index + query_len + 60).min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < text.len() { "..." } else { "" };
    format!("{prefix}{}{suffix}", text[start..end].replace('\n', " "))
}
