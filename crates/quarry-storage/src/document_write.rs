//! Markdown publication changes the durable document through native operations.
//! This also covers staged transactions and callers without an HTTP server.
use crate::{BlockMutationState, BlockRow, QuarryStore, blocks, document_state};
use document_state::{document_error as err, document_projection};
use quarry_core::{QuarryError, Result};
use quarry_document::{Document, SeedBlock};
use quarry_markdown::{ReconcileBase, ReconcileOp, block_rows_to_markdown, reconcile};
use turso::{Connection, params};

impl QuarryStore {
    pub(crate) async fn publish_document_version_conn(
        &self,
        conn: &Connection,
        id: &str,
        version_id: &str,
    ) -> Result<()> {
        let (version, content) = self.version_content_conn(conn, id, version_id).await?;
        let mut paths = conn
            .query("SELECT path FROM documents WHERE id=?1", params![id])
            .await
            .map_err(crate::map_turso_error)?;
        let row = paths
            .next()
            .await
            .map_err(crate::map_turso_error)?
            .ok_or_else(|| QuarryError::NotFound(id.into()))?;
        let path = crate::text(&row, 0)?;
        let saved = document_state::load_state_conn(conn, id).await?;
        if crate::document_kind(&path, &version.content_type) == crate::DocumentKind::RawDocument {
            if saved.is_some() {
                return Err(QuarryError::Unsupported(
                    "A document with native history cannot become a raw file".into(),
                ));
            }
            return crate::publish_native_put_conn(conn, id, version_id).await;
        }
        let markdown = std::str::from_utf8(&content).map_err(err)?;
        let (_, body) = crate::split_markdown_frontmatter(markdown)?;
        let mut document = if let Some(saved) = saved {
            let mut document = Document::load(&saved.bytes).map_err(err)?;
            apply_markdown(&mut document, body)?;
            document
        } else {
            quarry_markdown::import_markdown_document(id, body, &mut || {
                uuid::Uuid::new_v4().to_string()
            })?
        };
        persist_projection(conn, id, version_id, &mut document).await?;
        crate::publish_native_put_conn(conn, id, version_id).await
    }

    /// Runs under the store's exclusive startup lock. Each document is atomic;
    /// restarting an interrupted upgrade resumes only the remaining documents.
    pub(crate) async fn migrate_document_states(&self) -> Result<()> {
        let conn = self.conn()?;
        let mut rows = conn.query("SELECT d.id,d.head_version_id,d.path,v.content_type FROM documents d JOIN document_versions v ON v.id=d.head_version_id LEFT JOIN document_states s ON s.document_id=d.id WHERE s.document_id IS NULL", ())
            .await.map_err(crate::map_turso_error)?;
        let mut missing = Vec::new();
        while let Some(row) = rows.next().await.map_err(crate::map_turso_error)? {
            if crate::document_kind(&crate::text(&row, 2)?, &crate::text(&row, 3)?)
                == crate::DocumentKind::BlockDocument
            {
                missing.push((crate::text(&row, 0)?, crate::text(&row, 1)?));
            }
        }
        drop(rows);
        for (id, version) in missing {
            self.write_transaction(move |store, conn| {
                Box::pin(async move {
                    let (head, content) = store.version_content_conn(conn, &id, &version).await?;
                    let rows = blocks::load_block_tree_conn(conn, &id).await?;
                    let mut document = if rows.is_empty() {
                        let (_, body) = crate::split_markdown_frontmatter(
                            std::str::from_utf8(&content).map_err(err)?,
                        )?;
                        let mut document =
                            quarry_markdown::import_markdown_document(&id, body, &mut || {
                                uuid::Uuid::new_v4().to_string()
                            })?;
                        // Rows can be absent after an earlier Markdown overwrite.
                        // Preserve those SQL discussions as explicit missing targets.
                        crate::document_import::import_review_items(
                            &mut document,
                            &blocks::list_block_review_items_conn(conn, &id).await?,
                        )?;
                        document
                    } else {
                        let (_, body) = crate::split_markdown_frontmatter(
                            std::str::from_utf8(&content).map_err(err)?,
                        )?;
                        crate::import_document_with_markdown(
                            &BlockMutationState {
                                document_id: id.clone(),
                                path: String::new(),
                                head_version_id: version.clone(),
                                content_type: head.content_type,
                                metadata: head.metadata,
                                rows,
                                review_items: blocks::list_block_review_items_conn(conn, &id)
                                    .await?,
                                version_ids: Default::default(),
                                replay: None,
                            },
                            body,
                        )?
                    };
                    persist_projection(conn, &id, &version, &mut document).await
                })
            })
            .await?;
        }
        Ok(())
    }
}

pub(crate) async fn persist_projection(
    conn: &Connection,
    id: &str,
    version: &str,
    document: &mut Document,
) -> Result<()> {
    let rows = document_projection(document)?;
    block_rows_to_markdown(&rows)?;
    blocks::replace_block_rows_conn(conn, id, &rows).await?;
    blocks::replace_block_review_items_conn(
        conn,
        id,
        &crate::document_review_projection(document)?,
    )
    .await?;
    document_state::write_state_conn(
        conn,
        id,
        version,
        &format!("publish_{version}"),
        &crate::DurableDocumentCommit {
            bytes: document.save(),
            request_hash: format!("version:{version}"),
        },
    )
    .await
}

pub(crate) fn apply_markdown(document: &mut Document, markdown: &str) -> Result<()> {
    let current = document_projection(document)?;
    let outcome = reconcile(ReconcileBase::CurrentCanonical, markdown, &current, || {
        uuid::Uuid::new_v4().to_string()
    })?;
    let mut placements = Vec::new();
    for op in outcome.ops {
        match op {
            ReconcileOp::ReplaceBlockContent {
                block_id,
                text,
                marks,
                links,
            } => {
                let previous = document.block_view(&block_id).map_err(err)?.text;
                for hunk in quarry_markdown::utf16_text_diff_hunks(&previous, &text) {
                    let at = document
                        .point(&block_id, hunk.prefix as usize)
                        .map_err(err)?;
                    let ranges = document
                        .selection(&block_id, hunk.prefix as usize, hunk.old_mid_end as usize)
                        .map_err(err)?;
                    let inserted = String::from_utf16(
                        &text
                            .encode_utf16()
                            .skip(hunk.prefix as usize)
                            .take((hunk.new_mid_end - hunk.prefix) as usize)
                            .collect::<Vec<_>>(),
                    )
                    .map_err(err)?;
                    document.delete_text(&ranges).map_err(err)?;
                    document.insert_text(&at, &inserted).map_err(err)?;
                }
                let row = BlockRow {
                    block_id,
                    parent_block_id: None,
                    position: 0,
                    block_type: String::new(),
                    attrs: Default::default(),
                    text,
                    marks,
                    links,
                };
                set_formatting(document, &row)?;
            }
            ReconcileOp::SetBlockType {
                block_id,
                block_type,
                attrs,
            } => {
                let attrs = attrs
                    .map(|a| a.into_iter().collect())
                    .unwrap_or(document.block(&block_id).map_err(err)?.attrs);
                document
                    .set_block(&block_id, &block_type, attrs)
                    .map_err(err)?;
            }
            ReconcileOp::SetBlockAttrs { block_id, attrs } => {
                let kind = document.block(&block_id).map_err(err)?.kind;
                document
                    .set_block(&block_id, &kind, attrs.into_iter().collect())
                    .map_err(err)?;
            }
            ReconcileOp::DeleteBlock { block_id } => {
                document.delete_block(&block_id).map_err(err)?
            }
            op => placements.push(op),
        }
    }
    // The reconciler emits final indices. Determine the final order before
    // issuing sequential moves, so displaced siblings keep their identities.
    let mut order: Vec<_> = document
        .blocks()
        .map_err(err)?
        .into_iter()
        .filter(|b| b.parent.is_none())
        .map(|b| b.id)
        .collect();
    for op in &placements {
        if let ReconcileOp::MoveBlock { block_id, .. } = op {
            order.retain(|id| id != block_id);
        }
    }
    for op in &placements {
        match op {
            ReconcileOp::MoveBlock { block_id, position } => {
                order.insert((*position).min(order.len()), block_id.clone())
            }
            ReconcileOp::InsertBlock { rows, position } => {
                order.insert((*position).min(order.len()), rows[0].block_id.clone());
                insert_rows(document, rows, None, usize::MAX)?;
            }
            ReconcileOp::InsertChild {
                rows,
                parent_block_id,
                position,
            } => insert_rows(document, rows, Some(parent_block_id.clone()), *position)?,
            _ => unreachable!(),
        }
    }
    let mut before = None;
    for id in order.iter().rev() {
        document.move_block(id, None, before).map_err(err)?;
        before = Some(id.as_str());
    }
    Ok(())
}

fn insert_rows(
    document: &mut Document,
    rows: &[BlockRow],
    parent: Option<String>,
    position: usize,
) -> Result<()> {
    for (index, row) in rows.iter().enumerate() {
        document
            .insert_block(SeedBlock {
                id: row.block_id.clone(),
                kind: row.block_type.clone(),
                parent: if index == 0 {
                    parent.clone()
                } else {
                    row.parent_block_id.clone()
                },
                position: if index == 0 {
                    position.min(document.blocks().map_err(err)?.len())
                } else {
                    row.position as usize
                },
                attrs: row.attrs.clone().into_iter().collect(),
                text: row.text.clone(),
            })
            .map_err(err)?;
        set_formatting(document, row)?;
    }
    Ok(())
}

fn set_formatting(document: &mut Document, row: &BlockRow) -> Result<()> {
    let view = document.block_view(&row.block_id).map_err(err)?;
    let names: std::collections::BTreeSet<_> = view
        .runs
        .into_iter()
        .flat_map(|r| r.marks.into_keys())
        .collect();
    let ranges = document
        .selection(&row.block_id, 0, row.text.encode_utf16().count())
        .map_err(err)?;
    for name in names {
        document
            .format(&ranges, &name, &serde_json::Value::Null)
            .map_err(err)?;
    }
    for mark in &row.marks {
        let ranges = document
            .selection(&row.block_id, mark.start as usize, mark.end as usize)
            .map_err(err)?;
        for (name, value) in &mark.marks {
            document.format(&ranges, name, value).map_err(err)?;
        }
    }
    for link in &row.links {
        let ranges = document
            .selection(&row.block_id, link.start as usize, link.end as usize)
            .map_err(err)?;
        document
            .format(&ranges, "link", &link.url.clone().into())
            .map_err(err)?;
    }
    Ok(())
}
