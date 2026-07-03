use git2::{
    build::{CheckoutBuilder, RepoBuilder},
    FetchOptions, IndexAddOption, ObjectType, PushOptions, Repository, Signature,
};
use quarry_core::{
    normalize_path, render_markdown_frontmatter, ConflictRecord, DocumentListEntry, DocumentSource,
    QuarryError, Result, SyncStateEntry, GIT_BINARY_WARN_THRESHOLD,
};
use quarry_storage::{
    split_markdown_frontmatter, BlockMarkdownWrite, BlockMarkdownWriteOutcome, BlockWriteBase,
    DocumentKind, DocumentScopeRef, QuarryStore,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use utoipa::ToSchema;
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct GitExportOptions {
    pub branch: String,
    pub force_large: bool,
    pub frontmatter_markdown: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitImportResult {
    pub imported_paths: Vec<String>,
    pub transaction_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitExportResult {
    pub exported_paths: Vec<String>,
    pub commit_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitSyncResult {
    pub imported_paths: Vec<String>,
    pub exported_paths: Vec<String>,
    pub conflict_paths: Vec<String>,
    pub conflicts: Vec<ConflictRecord>,
    pub commit_id: Option<String>,
}

#[derive(Clone, Debug)]
struct PeerConfig {
    repo: PathBuf,
    branch: String,
    remote: Option<String>,
    max_delete_percent: u8,
}

#[derive(Clone, Debug)]
struct GitFile {
    content: Vec<u8>,
    metadata: JsonValue,
    content_type: String,
    oid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Marker {
    library_id: String,
    library_slug: String,
}

pub async fn push_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    let store_clone = store.clone();
    let library = library.to_string();
    let peer_id = peer_id.to_string();
    store
        .run_global_operation(
            async move { push_peer_inner(&store_clone, &library, &peer_id).await },
        )
        .await
}

async fn push_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.push.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git push started"
    );
    let branch = peer.branch.clone();
    let export = export_worktree(
        store,
        library,
        &peer.repo,
        GitExportOptions {
            branch: branch.clone(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await?;
    if let Some(remote) = &peer.remote {
        push_remote(&peer.repo, remote, &branch)?;
    }
    record_exported_sync_state(store, library, peer_id, &peer.repo).await?;
    tracing::info!(
        event = "git.push.completed",
        library,
        peer_id,
        branch,
        exported_paths = export.exported_paths.len(),
        remote_url = peer
            .remote
            .as_deref()
            .map(redact_remote_url)
            .unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git push completed"
    );
    Ok(GitSyncResult {
        imported_paths: Vec::new(),
        exported_paths: export.exported_paths,
        conflict_paths: Vec::new(),
        conflicts: Vec::new(),
        commit_id: export.commit_id,
    })
}

pub async fn pull_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    let store_clone = store.clone();
    let library = library.to_string();
    let peer_id = peer_id.to_string();
    store
        .run_global_operation(
            async move { pull_peer_inner(&store_clone, &library, &peer_id).await },
        )
        .await
}

async fn pull_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.pull.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git pull started"
    );
    if let Some(remote) = &peer.remote {
        fetch_remote_worktree(&peer.repo, remote, &peer.branch)?;
    }
    verify_marker(&peer.repo, &store.get_library(library).await?.id)?;
    let import = import_worktree(store, library, &peer.repo).await?;
    record_exported_sync_state(store, library, peer_id, &peer.repo).await?;
    tracing::info!(
        event = "git.pull.completed",
        library,
        peer_id,
        branch = peer.branch,
        imported_paths = import.imported_paths.len(),
        remote_url = peer
            .remote
            .as_deref()
            .map(redact_remote_url)
            .unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git pull completed"
    );
    Ok(GitSyncResult {
        imported_paths: import.imported_paths,
        exported_paths: Vec::new(),
        conflict_paths: Vec::new(),
        conflicts: Vec::new(),
        commit_id: None,
    })
}

pub async fn sync_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    let store_clone = store.clone();
    let library = library.to_string();
    let peer_id = peer_id.to_string();
    store
        .run_global_operation(
            async move { sync_peer_inner(&store_clone, &library, &peer_id).await },
        )
        .await
}

async fn sync_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.sync.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git sync started"
    );
    if let Some(remote) = &peer.remote {
        fetch_remote_worktree(&peer.repo, remote, &peer.branch)?;
    }
    let library_record = store.get_library(library).await?;
    verify_marker(&peer.repo, &library_record.id)?;

    let docs = store
        .list_documents(&library_record.slug, None, Some(10_000))
        .await?;
    let doc_map: HashMap<String, DocumentListEntry> = docs
        .into_iter()
        .map(|doc| (doc.path.clone(), doc))
        .collect();
    let git_map = worktree_snapshot(&peer.repo)?;
    let mut paths: BTreeSet<String> = doc_map.keys().cloned().collect();
    paths.extend(git_map.keys().cloned());
    let sync_states: HashMap<String, SyncStateEntry> = store
        .list_sync_state(peer_id)
        .await?
        .into_iter()
        .map(|state| (state.path.clone(), state))
        .collect();
    paths.extend(sync_states.keys().cloned());

    // Pure git-side renames: a clean delete and a clean create whose bytes
    // match exactly — unique on both sides, the reconciler's no-guess move
    // rule — pair into an identity-preserving document move instead of
    // delete + create. Block ids, review anchors, version history, and the
    // peer's shadow base all ride the document id.
    let renames = pair_renames(
        store,
        &library_record.slug,
        &paths,
        &doc_map,
        &git_map,
        &sync_states,
    )
    .await?;
    let renamed_from: BTreeSet<String> = renames.iter().map(|rename| rename.from.clone()).collect();

    enforce_delete_safety(
        &paths,
        &doc_map,
        &git_map,
        &sync_states,
        &renamed_from,
        peer.max_delete_percent,
    )?;

    let mut imported_paths = Vec::new();
    let mut conflict_paths = Vec::new();
    let mut conflicts = Vec::new();
    let mut deleted_sync_paths = BTreeSet::new();
    let mut renamed_paths = renamed_from.clone();

    for rename in &renames {
        store
            .move_document(
                &library_record.slug,
                &rename.from,
                &rename.to,
                DocumentSource::Git,
            )
            .await?;
        tracing::info!(
            event = "git.sync.rename_paired",
            library = library_record.slug,
            peer_id,
            from = %rename.from,
            to = %rename.to,
            "git-side rename paired into an identity-preserving move"
        );
        // The old path's sync state clears below; the moved document exports
        // at its new path, which records the new state.
        deleted_sync_paths.insert(rename.from.clone());
        renamed_paths.insert(rename.to.clone());
        imported_paths.push(rename.to.clone());
    }

    for path in paths {
        if renamed_paths.contains(&path) {
            continue;
        }
        let doc = doc_map.get(&path);
        let git = git_map.get(&path);
        let state = sync_states.get(&path);
        let last_doc = state
            .as_ref()
            .and_then(|state| state.last_synced_doc_version_id.as_deref());
        let last_git = state
            .as_ref()
            .and_then(|state| state.last_synced_git_oid.as_deref());
        let doc_changed = doc
            .map(|doc| Some(doc.head_version_id.as_str()) != last_doc)
            .unwrap_or(last_doc.is_some());
        let git_changed = git
            .map(|git| Some(git.oid.as_str()) != last_git)
            .unwrap_or(last_git.is_some());

        match (doc, git, doc_changed, git_changed) {
            (Some(doc), Some(git), true, true) => {
                let current = store.get_document(&library_record.slug, &path).await?;
                if current.content == git.content {
                    continue;
                }
                if is_block_file(&path, &git.content_type) {
                    // Phase 4: both sides changed a Markdown document —
                    // diff3 against the peer's shadow base. Without one, the
                    // last-synced version's content is the common ancestor;
                    // without even that (both sides created the path
                    // independently) an EMPTY base keeps it conservative:
                    // differences conflict instead of silently overwriting.
                    // Non-conflicting hunks from both sides land; true
                    // conflicts become review items, never sibling files or
                    // sync failures.
                    let ancestor = match last_doc {
                        Some(version_id) => {
                            let version = store
                                .document_version(&library_record.slug, &path, version_id)
                                .await?;
                            BlockWriteBase::Markdown {
                                markdown: version.content,
                                version_id: Some(version_id.to_string()),
                            }
                        }
                        None => BlockWriteBase::Markdown {
                            markdown: String::new(),
                            version_id: None,
                        },
                    };
                    write_markdown_file(
                        store,
                        &library_record.slug,
                        Some(peer_id),
                        &path,
                        git,
                        ancestor,
                    )
                    .await?;
                    imported_paths.push(path);
                    continue;
                }

                let conflict_path = conflict_sibling_path(&path);
                let outcome = store
                    .put_document(
                        &library_record.slug,
                        &conflict_path,
                        git.content.clone(),
                        git.metadata.clone(),
                        &git.content_type,
                        DocumentSource::Git,
                        quarry_core::WritePrecondition::None,
                    )
                    .await?;
                let conflict = store
                    .record_conflict(
                        &library_record.slug,
                        &path,
                        Some(doc.head_version_id.clone()),
                        Some(outcome.version.id.clone()),
                    )
                    .await?;
                log_git_conflict_recorded(
                    &library_record,
                    peer_id,
                    &path,
                    Some(&conflict_path),
                    &conflict,
                );
                conflict_paths.push(conflict_path);
                conflicts.push(conflict);
            }
            (Some(doc), None, true, true) => {
                let conflict = store
                    .record_conflict(
                        &library_record.slug,
                        &path,
                        Some(doc.head_version_id.clone()),
                        None,
                    )
                    .await?;
                log_git_conflict_recorded(&library_record, peer_id, &path, None, &conflict);
                conflicts.push(conflict);
            }
            (Some(_doc), None, false, true) => {
                store
                    .delete_document(&library_record.slug, &path, DocumentSource::Git)
                    .await?;
                deleted_sync_paths.insert(path.clone());
                imported_paths.push(path);
            }
            (None, Some(git), true, true) if last_doc.is_some() => {
                let conflict_path = conflict_sibling_path(&path);
                // Delete-vs-create: Quarry deleted the path, Git changed it.
                // The Git side is preserved as a sibling document; Markdown
                // siblings import through the block writer (a first import —
                // fresh ids, no base) so they are ordinary BlockDocuments,
                // not raw bytes with a cleared projection.
                let sibling_version_id = if is_block_file(&path, &git.content_type) {
                    write_markdown_file(
                        store,
                        &library_record.slug,
                        Some(peer_id),
                        &conflict_path,
                        git,
                        BlockWriteBase::CurrentCanonical,
                    )
                    .await?
                    .outcome
                    .version
                    .id
                } else {
                    store
                        .put_document(
                            &library_record.slug,
                            &conflict_path,
                            git.content.clone(),
                            git.metadata.clone(),
                            &git.content_type,
                            DocumentSource::Git,
                            quarry_core::WritePrecondition::None,
                        )
                        .await?
                        .version
                        .id
                };
                let conflict = store
                    .record_conflict(&library_record.slug, &path, None, Some(sibling_version_id))
                    .await?;
                log_git_conflict_recorded(
                    &library_record,
                    peer_id,
                    &path,
                    Some(&conflict_path),
                    &conflict,
                );
                deleted_sync_paths.insert(path.clone());
                conflict_paths.push(conflict_path);
                conflicts.push(conflict);
            }
            (None, Some(_git), true, false) if last_doc.is_some() => {
                deleted_sync_paths.insert(path.clone());
            }
            (None, None, true, true) | (None, None, true, false) | (None, None, false, true) => {
                deleted_sync_paths.insert(path.clone());
            }
            (None, Some(git), _, true) | (None, Some(git), _, false) => {
                if is_block_file(&path, &git.content_type) {
                    write_markdown_file(
                        store,
                        &library_record.slug,
                        Some(peer_id),
                        &path,
                        git,
                        BlockWriteBase::CurrentCanonical,
                    )
                    .await?;
                } else {
                    store
                        .put_document(
                            &library_record.slug,
                            &path,
                            git.content.clone(),
                            git.metadata.clone(),
                            &git.content_type,
                            DocumentSource::Git,
                            quarry_core::WritePrecondition::None,
                        )
                        .await?;
                }
                imported_paths.push(path);
            }
            (Some(_), None, true, _) => {
                // Quarry changed or created the path; export publishes it below.
            }
            (Some(doc), Some(git), false, true) => {
                if is_block_file(&path, &git.content_type) {
                    // The document is unchanged since the last sync, so the
                    // current canonical state IS the common ancestor.
                    write_markdown_file(
                        store,
                        &library_record.slug,
                        Some(peer_id),
                        &path,
                        git,
                        BlockWriteBase::CurrentCanonical,
                    )
                    .await?;
                } else {
                    store
                        .put_document(
                            &library_record.slug,
                            &path,
                            git.content.clone(),
                            git.metadata.clone(),
                            &git.content_type,
                            DocumentSource::Git,
                            quarry_core::WritePrecondition::IfMatch(doc.head_version_id.clone()),
                        )
                        .await?;
                }
                imported_paths.push(path);
            }
            _ => {}
        }
    }

    let export = export_worktree(
        store,
        &library_record.slug,
        &peer.repo,
        GitExportOptions {
            branch: peer.branch.clone(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await?;
    if let Some(remote) = &peer.remote {
        push_remote(&peer.repo, remote, &peer.branch)?;
    }
    record_exported_sync_state(store, &library_record.slug, peer_id, &peer.repo).await?;
    for path in &deleted_sync_paths {
        store.upsert_sync_state(peer_id, path, None, None).await?;
    }
    tracing::info!(
        event = "git.sync.completed",
        library = library_record.slug,
        library_id = %library_record.id,
        peer_id,
        branch = peer.branch,
        imported_paths = imported_paths.len(),
        exported_paths = export.exported_paths.len(),
        conflicts = conflicts.len(),
        remote_url = peer.remote.as_deref().map(redact_remote_url).unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git sync completed"
    );

    Ok(GitSyncResult {
        imported_paths,
        exported_paths: export.exported_paths,
        conflict_paths,
        conflicts,
        commit_id: export.commit_id,
    })
}

/// Imports a Git worktree into a library.
///
/// **Atomicity (changed in Phase 4):** Markdown files commit PER DOCUMENT
/// through the reconciling writer (two-way merge against the current
/// canonical state; byte-identical files are no-ops) and therefore escape
/// the staged transaction — an import that fails midway leaves the markdown
/// documents already imported in place. Raw files keep the staged
/// multi-document transaction and roll back together on failure.
pub async fn import_worktree(
    store: &QuarryStore,
    library: &str,
    repo_dir: &Path,
) -> Result<GitImportResult> {
    if !repo_dir.exists() {
        return Err(QuarryError::NotFound(repo_dir.display().to_string()));
    }
    let library_record = store.get_library(library).await?;
    let tx = store
        .begin_transaction(
            &library_record.slug,
            DocumentSource::Git,
            Some("git".to_string()),
            Some(format!("import from {}", repo_dir.display())),
            serde_json::json!({"repo": repo_dir.display().to_string()}),
        )
        .await?;

    let result = import_worktree_transaction(store, &library_record.slug, repo_dir, &tx.id).await;
    if result.is_err() {
        let _ = store.rollback_transaction(&tx.id).await;
    }
    result
}

async fn import_worktree_transaction(
    store: &QuarryStore,
    library: &str,
    repo_dir: &Path,
    tx_id: &str,
) -> Result<GitImportResult> {
    let mut imported_paths = Vec::new();
    for entry in WalkDir::new(repo_dir).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        name != ".git" && name != ".quarry"
    }) {
        let entry = entry.map_err(|err| QuarryError::Io(err.into()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(repo_dir)
            .map_err(|err| QuarryError::Storage(err.to_string()))?;
        if is_sidecar(relative) {
            continue;
        }
        let path = normalize_path(&relative.to_string_lossy())?;
        let bytes = fs::read(entry.path())?;
        let (content, mut metadata) = if path.ends_with(".md") {
            let (_, metadata) = split_frontmatter(&bytes)?;
            (bytes, metadata)
        } else {
            (bytes, serde_json::json!({}))
        };
        merge_metadata(&mut metadata, sidecar_metadata(repo_dir, &path)?);
        ensure_content_type(&mut metadata, &path);
        let content_type = metadata
            .get("content_type")
            .and_then(JsonValue::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        if is_block_file(&path, &content_type) {
            // Phase 4: Markdown imports reconcile per document (two-way —
            // plain `git import` has no peer scope, so the base is the
            // current canonical state). Byte-identical files are no-ops, so
            // re-imports do not churn versions. Raw files keep the staged
            // multi-document transaction below.
            let file = GitFile {
                content,
                metadata,
                content_type,
                oid: String::new(),
            };
            write_markdown_file(
                store,
                library,
                None,
                &path,
                &file,
                BlockWriteBase::CurrentCanonical,
            )
            .await?;
        } else {
            store
                .stage_put(tx_id, &path, content, metadata, &content_type)
                .await?;
        }
        imported_paths.push(path);
    }

    imported_paths.sort();
    store.commit_transaction(tx_id).await?;
    Ok(GitImportResult {
        imported_paths,
        transaction_id: tx_id.to_string(),
    })
}

pub async fn export_worktree(
    store: &QuarryStore,
    library: &str,
    repo_dir: &Path,
    options: GitExportOptions,
) -> Result<GitExportResult> {
    fs::create_dir_all(repo_dir)?;
    let library_record = store.get_library(library).await?;
    verify_or_write_marker(repo_dir, &library_record.id, &library_record.slug)?;
    clean_worktree(repo_dir)?;

    let documents = store
        .list_documents(&library_record.slug, None, Some(10_000))
        .await?;
    let mut exported_paths = Vec::new();
    for entry in documents {
        if is_reserved_git_metadata_path(&entry.path) {
            return Err(QuarryError::InvalidPath(format!(
                "{} is reserved for Git metadata sidecars",
                entry.path
            )));
        }
        let document = store
            .get_document(&library_record.slug, &entry.path)
            .await?;
        if document.content.len() > GIT_BINARY_WARN_THRESHOLD && !options.force_large {
            return Err(QuarryError::Conflict(format!(
                "{} is larger than the 5 MiB phase-one Git export threshold",
                document.path
            )));
        }
        let output = repo_dir.join(&document.path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        if options.frontmatter_markdown && document.path.ends_with(".md") {
            write_atomic(
                &output,
                &markdown_with_frontmatter(&document.metadata, &document.content)?,
            )?;
        } else {
            write_atomic(&output, &document.content)?;
            write_sidecar(repo_dir, &document.path, &document.metadata)?;
        }
        exported_paths.push(document.path);
    }
    write_marker(repo_dir, &library_record.id, &library_record.slug)?;

    let commit_id = commit_all(repo_dir, &options.branch, "Quarry export")?;
    exported_paths.sort();
    Ok(GitExportResult {
        exported_paths,
        commit_id,
    })
}

fn split_frontmatter(bytes: &[u8]) -> Result<(Vec<u8>, JsonValue)> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let (text, bom_len) = text
        .strip_prefix('\u{feff}')
        .map(|text| (text, 3))
        .unwrap_or((text, 0));
    let Some(open_len) = frontmatter_open_len(text) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let body = &text[open_len..];
    let Some((end, close_len)) = frontmatter_close(body) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let yaml = &body[..end];
    let body_start = bom_len + open_len + end + close_len;
    let metadata: JsonValue =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;
    Ok((bytes[body_start..].to_vec(), metadata))
}

fn frontmatter_open_len(text: &str) -> Option<usize> {
    if text.starts_with("---\n") {
        Some(4)
    } else if text.starts_with("---\r\n") {
        Some(5)
    } else {
        None
    }
}

fn frontmatter_close(text: &str) -> Option<(usize, usize)> {
    ["\n---\n", "\r\n---\r\n", "\n---\r\n", "\r\n---\n"]
        .into_iter()
        .filter_map(|marker| text.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)
}

fn sidecar_metadata(repo_dir: &Path, path: &str) -> Result<Option<JsonValue>> {
    let sidecar = repo_dir.join(format!("{path}.quarrymeta.yaml"));
    if !sidecar.exists() {
        return Ok(None);
    }
    let yaml = fs::read_to_string(sidecar)?;
    let metadata: JsonValue =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(&yaml)?)?;
    Ok(Some(metadata))
}

fn merge_metadata(target: &mut JsonValue, patch: Option<JsonValue>) {
    let Some(patch) = patch else {
        return;
    };
    match (target, patch) {
        (JsonValue::Object(target), JsonValue::Object(patch)) => {
            for (key, value) in patch {
                target.insert(key, value);
            }
        }
        (target, value) => *target = value,
    }
}

fn ensure_content_type(metadata: &mut JsonValue, path: &str) {
    if let JsonValue::Object(object) = metadata {
        object.entry("content_type".to_string()).or_insert_with(|| {
            JsonValue::String(
                mime_guess::from_path(path)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string(),
            )
        });
    }
}

fn markdown_with_frontmatter(metadata: &JsonValue, content: &[u8]) -> Result<Vec<u8>> {
    // Documents written through the block gateway store their normalized
    // text WITH frontmatter already; exporting must not double it.
    if content.starts_with(b"---\n") || content.starts_with(b"---\r\n") {
        return Ok(content.to_vec());
    }
    let header = render_markdown_frontmatter(metadata)?;
    if header.is_empty() {
        return Ok(content.to_vec());
    }
    let mut output = header.into_bytes();
    output.extend_from_slice(content);
    Ok(output)
}

/// Writes via a sibling temp file plus rename so a crash mid-write leaves
/// either the old file or the new one, never a truncated hybrid.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let Some(file_name) = path.file_name() else {
        return Err(QuarryError::InvalidPath(path.display().to_string()));
    };
    let tmp = path.with_file_name(format!(
        "{}.quarry-tmp-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    ));
    fs::write(&tmp, contents)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

fn write_sidecar(repo_dir: &Path, path: &str, metadata: &JsonValue) -> Result<()> {
    if metadata == &serde_json::json!({}) {
        return Ok(());
    }
    let sidecar = repo_dir.join(format!("{path}.quarrymeta.yaml"));
    if let Some(parent) = sidecar.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&sidecar, serde_yaml::to_string(metadata)?.as_bytes())?;
    Ok(())
}

fn is_sidecar(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".quarrymeta.yaml")
}

fn is_reserved_git_metadata_path(path: &str) -> bool {
    path.ends_with(".quarrymeta.yaml")
}

fn marker_path(repo_dir: &Path) -> PathBuf {
    repo_dir.join(".quarry").join("marker.json")
}

fn verify_or_write_marker(repo_dir: &Path, library_id: &str, library_slug: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if path.exists() {
        let marker: Marker = serde_json::from_slice(&fs::read(&path)?)?;
        if marker.library_id != library_id {
            return Err(QuarryError::Conflict(format!(
                "Git marker belongs to library {} not {library_id}",
                marker.library_id
            )));
        }
    } else {
        write_marker(repo_dir, library_id, library_slug)?;
    }
    Ok(())
}

fn verify_marker(repo_dir: &Path, library_id: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if !path.exists() {
        return Err(QuarryError::Conflict(format!(
            "Git marker is missing at {}",
            path.display()
        )));
    }
    let marker: Marker = serde_json::from_slice(&fs::read(&path)?)?;
    if marker.library_id != library_id {
        return Err(QuarryError::Conflict(format!(
            "Git marker belongs to library {} not {library_id}",
            marker.library_id
        )));
    }
    Ok(())
}

fn write_marker(repo_dir: &Path, library_id: &str, library_slug: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let marker = Marker {
        library_id: library_id.to_string(),
        library_slug: library_slug.to_string(),
    };
    write_atomic(&path, serde_json::to_string_pretty(&marker)?.as_bytes())?;
    Ok(())
}

fn clean_worktree(repo_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(repo_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn commit_all(repo_dir: &Path, branch: &str, message: &str) -> Result<Option<String>> {
    let repo = Repository::open(repo_dir)
        .or_else(|_| Repository::init(repo_dir))
        .map_err(map_git)?;
    let head_ref = format!("refs/heads/{branch}");
    if repo.head().is_err() {
        repo.set_head(&head_ref).map_err(map_git)?;
    }
    let mut index = repo.index().map_err(map_git)?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(map_git)?;
    index.write().map_err(map_git)?;
    let tree_id = index.write_tree().map_err(map_git)?;
    let tree = repo.find_tree(tree_id).map_err(map_git)?;
    let signature = Signature::now("Quarry", "quarry@local").map_err(map_git)?;
    let parent = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .and_then(|oid| repo.find_commit(oid).ok());
    if parent
        .as_ref()
        .map(|commit| commit.tree_id() == tree_id)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let oid = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(map_git)?;
    Ok(Some(oid.to_string()))
}

fn fetch_remote_worktree(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let started = Instant::now();
    if repo_dir.join(".git").exists() {
        let repo = Repository::open(repo_dir).map_err(map_git)?;
        let mut remote = ensure_remote(&repo, remote_url)?;
        let mut options = FetchOptions::new();
        remote
            .fetch(&[branch], Some(&mut options), None)
            .map_err(map_git)?;
        checkout_remote_branch(&repo, branch)?;
        tracing::debug!(
            event = "git.remote.fetch.completed",
            repo = %repo_dir.display(),
            branch,
            remote_url = %redact_remote_url(remote_url),
            duration_ms = started.elapsed().as_millis() as u64,
            "Git remote fetch completed"
        );
        return Ok(());
    }

    if repo_dir.exists() && fs::read_dir(repo_dir)?.next().is_some() {
        return Err(QuarryError::Conflict(format!(
            "{} is not a Git repository and is not empty",
            repo_dir.display()
        )));
    }
    if repo_dir.exists() {
        fs::remove_dir(repo_dir)?;
    }
    if let Some(parent) = repo_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut builder = RepoBuilder::new();
    builder.branch(branch);
    builder.clone(remote_url, repo_dir).map_err(map_git)?;
    tracing::debug!(
        event = "git.remote.fetch.completed",
        repo = %repo_dir.display(),
        branch,
        remote_url = %redact_remote_url(remote_url),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git remote clone completed"
    );
    Ok(())
}

fn checkout_remote_branch(repo: &Repository, branch: &str) -> Result<()> {
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let object = repo.revparse_single(&remote_ref).map_err(map_git)?;
    let commit = object.peel_to_commit().map_err(map_git)?;
    let local_ref = format!("refs/heads/{branch}");
    repo.reference(&local_ref, commit.id(), true, "Quarry remote fetch")
        .map_err(map_git)?;
    repo.set_head(&local_ref).map_err(map_git)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout)).map_err(map_git)?;
    Ok(())
}

fn push_remote(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let started = Instant::now();
    let repo = Repository::open(repo_dir).map_err(map_git)?;
    let mut remote = ensure_remote(&repo, remote_url)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut options = PushOptions::new();
    remote
        .push(&[&refspec], Some(&mut options))
        .map_err(map_git)?;
    tracing::debug!(
        event = "git.remote.push.completed",
        repo = %repo_dir.display(),
        branch,
        remote_url = %redact_remote_url(remote_url),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git remote push completed"
    );
    Ok(())
}

fn log_git_conflict_recorded(
    library: &quarry_core::Library,
    peer_id: &str,
    path: &str,
    conflict_path: Option<&str>,
    conflict: &ConflictRecord,
) {
    tracing::debug!(
        event = "git.conflict.recorded",
        library = %library.slug,
        library_id = %library.id,
        peer_id,
        path,
        conflict_path = conflict_path.unwrap_or(""),
        conflict_id = %conflict.id,
        ours_version_id = conflict.ours_version_id.as_deref().unwrap_or(""),
        theirs_version_id = conflict.theirs_version_id.as_deref().unwrap_or(""),
        "Git conflict recorded"
    );
}

fn redact_remote_url(remote_url: &str) -> String {
    let Some((scheme, rest)) = remote_url.split_once("://") else {
        return remote_url.to_string();
    };
    let Some((userinfo, host_and_path)) = rest.split_once('@') else {
        return remote_url.to_string();
    };
    if userinfo.is_empty() {
        remote_url.to_string()
    } else {
        format!("{scheme}://<redacted>@{host_and_path}")
    }
}

fn ensure_remote<'repo>(repo: &'repo Repository, remote_url: &str) -> Result<git2::Remote<'repo>> {
    match repo.find_remote("origin") {
        Ok(remote) => {
            let existing_url = remote.url().ok().map(str::to_string);
            drop(remote);
            if existing_url.as_deref() != Some(remote_url) {
                repo.remote_set_url("origin", remote_url).map_err(map_git)?;
            }
        }
        Err(_) => {
            repo.remote("origin", remote_url).map_err(map_git)?;
        }
    }
    repo.find_remote("origin").map_err(map_git)
}

fn map_git(err: git2::Error) -> QuarryError {
    QuarryError::GitSource {
        source: Box::new(err),
    }
}

async fn peer_config(store: &QuarryStore, library: &str, peer_id: &str) -> Result<PeerConfig> {
    let peer = store
        .list_git_peers(library)
        .await?
        .into_iter()
        .find(|peer| peer.id == peer_id)
        .ok_or_else(|| QuarryError::NotFound(format!("git peer {peer_id}")))?;
    let repo = peer
        .config
        .get("repo")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| QuarryError::InvalidPath("git peer missing repo".to_string()))?;
    let branch = peer
        .config
        .get("branch")
        .and_then(JsonValue::as_str)
        .unwrap_or("main");
    let remote = peer
        .config
        .get("remote")
        .or_else(|| peer.config.get("remote_url"))
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    Ok(PeerConfig {
        repo: PathBuf::from(repo),
        branch: branch.to_string(),
        remote,
        max_delete_percent: peer
            .config
            .get("max_delete_percent")
            .and_then(JsonValue::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(100),
    })
}

async fn record_exported_sync_state(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
    repo_dir: &Path,
) -> Result<()> {
    let docs = store.list_documents(library, None, Some(10_000)).await?;
    let git = worktree_snapshot(repo_dir)?;
    for doc in docs {
        let git_oid = git.get(&doc.path).map(|file| file.oid.clone());
        // Phase 4 shadow-base bookkeeping: what this peer just saw (the
        // exported canonical body) is the diff3 base for its next write.
        let content_type = git
            .get(&doc.path)
            .map(|file| file.content_type.clone())
            .unwrap_or_default();
        if is_block_file(&doc.path, &content_type) {
            // Skip the full-content load when the recorded base already
            // names the current head: the base only advances with content,
            // so an unchanged document needs no rewrite (keeps no-op syncs
            // from reading every Markdown document's body).
            let recorded = store.block_shadow_base("git", peer_id, &doc.id).await?;
            let recorded_head = recorded.and_then(|base| base.base_version_id);
            if recorded_head.as_deref() != Some(doc.head_version_id.as_str()) {
                let document = store.get_document(library, &doc.path).await?;
                if let Ok(text) = String::from_utf8(document.content) {
                    let (_, body) = split_markdown_frontmatter(&text)?;
                    store
                        .put_block_shadow_base(
                            "git",
                            peer_id,
                            &document.id,
                            body,
                            Some(doc.head_version_id.clone()),
                        )
                        .await?;
                }
            }
        }
        store
            .upsert_sync_state(peer_id, &doc.path, Some(doc.head_version_id), git_oid)
            .await?;
    }
    Ok(())
}

fn worktree_snapshot(repo_dir: &Path) -> Result<HashMap<String, GitFile>> {
    let mut files = HashMap::new();
    if !repo_dir.exists() {
        return Ok(files);
    }
    for entry in WalkDir::new(repo_dir).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        name != ".git" && name != ".quarry"
    }) {
        let entry = entry.map_err(|err| QuarryError::Io(err.into()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(repo_dir)
            .map_err(|err| QuarryError::Storage(err.to_string()))?;
        if is_sidecar(relative) {
            continue;
        }
        let path = normalize_path(&relative.to_string_lossy())?;
        let raw = fs::read(entry.path())?;
        let oid = git2::Oid::hash_object(ObjectType::Blob, &raw)
            .map_err(map_git)?
            .to_string();
        // Markdown keeps its raw text (frontmatter included): the Phase 4
        // reconciled writer splits the frontmatter itself, and byte
        // comparisons against stored content (which carries frontmatter)
        // stay like-for-like. The parsed metadata still feeds sidecar
        // merging and content-type detection.
        let (content, mut metadata) = if path.ends_with(".md") {
            let (_, metadata) = split_frontmatter(&raw)?;
            (raw, metadata)
        } else {
            (raw, serde_json::json!({}))
        };
        merge_metadata(&mut metadata, sidecar_metadata(repo_dir, &path)?);
        ensure_content_type(&mut metadata, &path);
        let content_type = metadata
            .get("content_type")
            .and_then(JsonValue::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        files.insert(
            path,
            GitFile {
                content,
                metadata,
                content_type,
                oid,
            },
        );
    }
    Ok(files)
}

/// Whether a worktree file participates in the block model (Phase 4
/// reconciled writes) or stays on the raw byte path.
fn is_block_file(path: &str, content_type: &str) -> bool {
    quarry_storage::document_kind(path, content_type) == DocumentKind::BlockDocument
}

/// One reconciled Markdown write from a Git worktree file, with per-peer
/// shadow-base bookkeeping: the diff3 base is the canonical text this peer
/// last synced (recorded at export/import); a missing base degrades to the
/// two-way merge. After the write the peer's base advances to the new
/// canonical text. Merge conflicts never fail the sync — they surface as
/// conflict review items on the document. CriticMarkup (content the codec
/// rejects outright) DOES fail the file's import with the typed
/// unsupported-markdown error.
async fn write_markdown_file(
    store: &QuarryStore,
    library: &str,
    peer_id: Option<&str>,
    path: &str,
    file: &GitFile,
    fallback_base: BlockWriteBase,
) -> Result<BlockMarkdownWriteOutcome> {
    let markdown = String::from_utf8(file.content.clone())
        .map_err(|_| QuarryError::InvalidInput(format!("{path} is not valid UTF-8 markdown")))?;
    let base = match peer_id {
        Some(peer) => match store.head_document(library, path).await {
            Ok(head) => match store.block_shadow_base("git", peer, &head.id).await? {
                Some(shadow) => BlockWriteBase::Markdown {
                    markdown: shadow.base_markdown,
                    version_id: shadow.base_version_id,
                },
                None => fallback_base,
            },
            Err(QuarryError::NotFound(_)) => fallback_base,
            Err(error) => return Err(error),
        },
        None => fallback_base,
    };
    let outcome = store
        .write_block_markdown(BlockMarkdownWrite {
            scope: DocumentScopeRef::library(library),
            path: path.to_string(),
            markdown,
            metadata: file.metadata.clone(),
            base,
            source: DocumentSource::Git,
            surface: "git".to_string(),
            actor_label: peer_id.map(|peer| format!("Git sync ({peer})")),
        })
        .await?;
    if outcome.conflicts > 0 {
        tracing::info!(
            event = "git.sync.merge_conflicts_recorded",
            library,
            path,
            conflicts = outcome.conflicts,
            "git import merged with conflict review items"
        );
    }
    if let Some(peer) = peer_id {
        store
            .put_block_shadow_base(
                "git",
                peer,
                &outcome.outcome.document.id,
                &outcome.canonical_body,
                Some(outcome.outcome.version.id.clone()),
            )
            .await?;
    }
    Ok(outcome)
}

fn conflict_sibling_path(path: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    format!("{path}.conflict-git-{timestamp}")
}

struct RenamePair {
    from: String,
    to: String,
}

/// Pairs clean git-side deletes (document unchanged since the last sync,
/// file gone) with clean creates (a path this peer never synced) whose bytes
/// match exactly. Duplicate content on either side makes the pairing
/// ambiguous — no pairing happens, delete + create proceeds as before.
async fn pair_renames(
    store: &QuarryStore,
    library: &str,
    paths: &BTreeSet<String>,
    doc_map: &HashMap<String, DocumentListEntry>,
    git_map: &HashMap<String, GitFile>,
    sync_states: &HashMap<String, SyncStateEntry>,
) -> Result<Vec<RenamePair>> {
    let mut deletes_by_content: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    let mut creates_by_content: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    for path in paths {
        let state = sync_states.get(path);
        let last_doc = state.and_then(|state| state.last_synced_doc_version_id.as_deref());
        let last_git = state.and_then(|state| state.last_synced_git_oid.as_deref());
        match (doc_map.get(path), git_map.get(path)) {
            (Some(doc), None) => {
                let doc_unchanged = Some(doc.head_version_id.as_str()) == last_doc;
                if doc_unchanged && last_git.is_some() {
                    let content = store.get_document(library, path).await?.content;
                    deletes_by_content
                        .entry(content)
                        .or_default()
                        .push(path.clone());
                }
            }
            (None, Some(git)) if last_doc.is_none() => {
                creates_by_content
                    .entry(git.content.clone())
                    .or_default()
                    .push(path.clone());
            }
            _ => {}
        }
    }

    let mut pairs: Vec<RenamePair> = deletes_by_content
        .into_iter()
        .filter_map(|(content, from_paths)| {
            let to_paths = creates_by_content.get(&content)?;
            match (&from_paths[..], &to_paths[..]) {
                ([from], [to]) => Some(RenamePair {
                    from: from.clone(),
                    to: to.clone(),
                }),
                _ => None,
            }
        })
        .collect();
    pairs.sort_by(|left, right| left.from.cmp(&right.from));
    Ok(pairs)
}

fn enforce_delete_safety(
    paths: &BTreeSet<String>,
    doc_map: &HashMap<String, DocumentListEntry>,
    git_map: &HashMap<String, GitFile>,
    sync_states: &HashMap<String, SyncStateEntry>,
    renamed_from: &BTreeSet<String>,
    max_delete_percent: u8,
) -> Result<()> {
    let tracked = sync_states
        .values()
        .filter(|state| {
            state.last_synced_doc_version_id.is_some() || state.last_synced_git_oid.is_some()
        })
        .count();
    if tracked == 0 {
        return Ok(());
    }

    let delete_candidates = paths
        .iter()
        .filter(|path| {
            if renamed_from.contains(*path) {
                return false;
            }
            let Some(state) = sync_states.get(*path) else {
                return false;
            };
            let doc = doc_map.get(*path);
            let git = git_map.get(*path);
            let last_doc = state.last_synced_doc_version_id.as_deref();
            let last_git = state.last_synced_git_oid.as_deref();
            let doc_changed = doc
                .map(|doc| Some(doc.head_version_id.as_str()) != last_doc)
                .unwrap_or(last_doc.is_some());
            let git_changed = git
                .map(|git| Some(git.oid.as_str()) != last_git)
                .unwrap_or(last_git.is_some());

            matches!(
                (doc, git, doc_changed, git_changed),
                (Some(_), None, false, true) | (None, Some(_), true, false)
            )
        })
        .count();

    if delete_candidates * 100 > tracked * usize::from(max_delete_percent) {
        return Err(QuarryError::Conflict(format!(
            "delete safety abort: {delete_candidates} of {tracked} tracked paths would be deleted, exceeding {max_delete_percent}%"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_userinfo_without_touching_plain_remotes() {
        assert_eq!(
            redact_remote_url("https://token:secret@example.com/acme/repo.git"),
            "https://<redacted>@example.com/acme/repo.git"
        );
        assert_eq!(
            redact_remote_url("https://example.com/acme/repo.git"),
            "https://example.com/acme/repo.git"
        );
        assert_eq!(
            redact_remote_url("git@example.com:acme/repo.git"),
            "git@example.com:acme/repo.git"
        );
    }

    // A read-only destination distinguishes rename from open-and-truncate:
    // rename(2) only needs directory write permission, while an in-place
    // write fails — and would leave a truncated file on a crash mid-write.
    #[test]
    fn write_atomic_replaces_a_read_only_destination() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("doc.md");
        fs::write(&dest, "old")?;
        let mut permissions = fs::metadata(&dest)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&dest, permissions)?;

        write_atomic(&dest, b"new")?;

        assert_eq!(fs::read(&dest)?, b"new");
        Ok(())
    }

    #[test]
    fn write_atomic_leaves_no_temp_file_behind() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("doc.md");

        write_atomic(&dest, b"content")?;

        let names: Vec<_> = fs::read_dir(dir.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<_>>()?;
        assert_eq!(names, ["doc.md"]);
        Ok(())
    }
}
