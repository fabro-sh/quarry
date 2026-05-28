use git2::{
    build::{CheckoutBuilder, RepoBuilder},
    FetchOptions, IndexAddOption, ObjectType, PushOptions, Repository, Signature,
};
use quarry_core::{
    normalize_path, ConflictRecord, DocumentListEntry, DocumentSource, QuarryError, Result,
    SyncStateEntry, GIT_BINARY_WARN_THRESHOLD,
};
use quarry_storage::QuarryStore;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use utoipa::ToSchema;
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
    let peer = peer_config(store, library, peer_id).await?;
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
        library,
        peer_id,
        branch,
        exported_paths = export.exported_paths.len(),
        remote = peer.remote.as_deref().unwrap_or(""),
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
    let peer = peer_config(store, library, peer_id).await?;
    if let Some(remote) = &peer.remote {
        fetch_remote_worktree(&peer.repo, remote, &peer.branch)?;
    }
    verify_marker(&peer.repo, &store.get_library(library).await?.id)?;
    let import = import_worktree(store, library, &peer.repo).await?;
    record_exported_sync_state(store, library, peer_id, &peer.repo).await?;
    tracing::info!(
        library,
        peer_id,
        branch = peer.branch,
        imported_paths = import.imported_paths.len(),
        remote = peer.remote.as_deref().unwrap_or(""),
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
    let peer = peer_config(store, library, peer_id).await?;
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

    enforce_delete_safety(
        &paths,
        &doc_map,
        &git_map,
        &sync_states,
        peer.max_delete_percent,
    )?;

    let mut imported_paths = Vec::new();
    let mut conflict_paths = Vec::new();
    let mut conflicts = Vec::new();
    let mut deleted_sync_paths = BTreeSet::new();

    for path in paths {
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
                        None,
                        Some(outcome.version.id.clone()),
                    )
                    .await?;
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
                imported_paths.push(path);
            }
            (Some(_), None, true, _) => {
                // Quarry changed or created the path; export publishes it below.
            }
            (Some(doc), Some(git), false, true) => {
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
        library = library_record.slug,
        peer_id,
        branch = peer.branch,
        imported_paths = imported_paths.len(),
        exported_paths = export.exported_paths.len(),
        conflicts = conflicts.len(),
        remote = peer.remote.as_deref().unwrap_or(""),
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

    let result = import_worktree_transaction(store, repo_dir, &tx.id).await;
    if result.is_err() {
        let _ = store.rollback_transaction(&tx.id).await;
    }
    result
}

async fn import_worktree_transaction(
    store: &QuarryStore,
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
            split_frontmatter(&bytes)?
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
        store
            .stage_put(tx_id, &path, content, metadata, &content_type)
            .await?;
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
            fs::write(
                &output,
                markdown_with_frontmatter(&document.metadata, &document.content)?,
            )?;
        } else {
            fs::write(&output, &document.content)?;
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
    let mut frontmatter = BTreeMap::new();
    if let JsonValue::Object(object) = metadata {
        for (key, value) in object {
            if key != "content_type" {
                frontmatter.insert(key.clone(), value.clone());
            }
        }
    }
    if frontmatter.is_empty() {
        return Ok(content.to_vec());
    }
    let yaml = serde_yaml::to_string(&frontmatter)?;
    let mut output = Vec::new();
    output.extend_from_slice(b"---\n");
    output.extend_from_slice(yaml.trim_end().as_bytes());
    output.extend_from_slice(b"\n---\n");
    output.extend_from_slice(content);
    Ok(output)
}

fn write_sidecar(repo_dir: &Path, path: &str, metadata: &JsonValue) -> Result<()> {
    if metadata == &serde_json::json!({}) {
        return Ok(());
    }
    let sidecar = repo_dir.join(format!("{path}.quarrymeta.yaml"));
    if let Some(parent) = sidecar.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(sidecar, serde_yaml::to_string(metadata)?)?;
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
    fs::write(path, serde_json::to_string_pretty(&marker)?)?;
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
    if repo_dir.join(".git").exists() {
        let repo = Repository::open(repo_dir).map_err(map_git)?;
        let mut remote = ensure_remote(&repo, remote_url)?;
        let mut options = FetchOptions::new();
        remote
            .fetch(&[branch], Some(&mut options), None)
            .map_err(map_git)?;
        checkout_remote_branch(&repo, branch)?;
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
    let repo = Repository::open(repo_dir).map_err(map_git)?;
    let mut remote = ensure_remote(&repo, remote_url)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut options = PushOptions::new();
    remote
        .push(&[&refspec], Some(&mut options))
        .map_err(map_git)?;
    Ok(())
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
    QuarryError::Git(err.to_string())
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
        let (content, mut metadata) = if path.ends_with(".md") {
            split_frontmatter(&raw)?
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

fn conflict_sibling_path(path: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    format!("{path}.conflict-git-{timestamp}")
}

fn enforce_delete_safety(
    paths: &BTreeSet<String>,
    doc_map: &HashMap<String, DocumentListEntry>,
    git_map: &HashMap<String, GitFile>,
    sync_states: &HashMap<String, SyncStateEntry>,
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
