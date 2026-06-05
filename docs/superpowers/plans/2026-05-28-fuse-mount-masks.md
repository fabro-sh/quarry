# FUSE Mount Masks Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox syntax for progress tracking.

**Goal:** Add mount-local FUSE masks so selected path prefixes can be hidden or made read-only for one Quarry mount.

**Architecture:** Keep masks inside `quarry-fuse` as projection rules carried by `FuseProjection`. The CLI parses repeated `--mask` values and passes a `MountMasks` value into the mount; storage, REST, Git, and normal CLI document commands remain unchanged. Hidden masks are enforced as not found, while read-only masks are enforced as write rejections before mutating store calls.

**Tech Stack:** Rust workspace, `clap`, `fuse3`, `tokio`, existing `quarry-fuse`, `quarry-cli`, and `quarry-storage`.

* * *
## Current State
- `quarry mount <library> <mountpoint>` creates a writable auto-commit Linux FUSE projection.
  
- `quarry mount ... --read-only` makes the whole FUSE projection read-only.
  
- `FuseProjection::open(store, library, read_only)` has only a global read-only flag.
  
- Mutating projection methods call `ensure_writable()` before storage writes.
  
- Directory listings are built from committed documents plus persisted `dir_metadata`.
  
## Target Behavior
- `--mask <path-prefix>` hides that prefix and its descendants.
  
- `--mask hide:<path-prefix>` is the explicit hidden form.
  
- `--mask ro:<path-prefix>` makes that prefix and its descendants read-only.
  
- Hidden wins over read-only.
  
- Prefix matching is path-component aware: `private` matches `private` and `private/a.md`, not `privateer`.
  
- Empty/root mask prefixes are rejected. Use existing global `--read-only` for whole-mount immutability.
  
- Masks apply only to the mount process that received them.
  
## Proposed Files
- Modify `crates/quarry-fuse/src/lib.rs`
  
  - Add `MountMasks`.
    
  - Add `FuseProjection::open_with_masks`.
    
  - Add `mount_library_with_masks`.
    
  - Enforce hidden and read-only masks in projection methods.
    
- Modify `crates/quarry-fuse/tests/projection.rs`
  
  - Add parser, hidden projection, and read-only projection tests.
    
- Modify `crates/quarry-cli/src/lib.rs`
  
  - Add `--mask` parsing and pass masks to FUSE.
    
  - Add CLI parser coverage.
    
- Modify `docs/operations/fuse.md`
  
  - Document mask syntax and semantics.
    
## Task 1: Add Mount Mask Model
**Files:**

- [ ] 
  
  Modify: `crates/quarry-fuse/src/lib.rs`
  
- [ ] 
  
  Test: `crates/quarry-fuse/tests/projection.rs`
  
- [ ] 
  
  **Step 1: Write failing parser and matcher tests**
  

Add `MountMasks` to the test imports:

```rust
use quarry_fuse::{FuseNodeKind, FuseProjection, MountMasks};
```

Add these tests near the top of `crates/quarry-fuse/tests/projection.rs`:

```rust
#[test]
fn mount_masks_parse_specs_and_match_component_prefixes() {
    let masks = MountMasks::parse_specs([
        "private",
        "hide:/secrets/",
        "ro:published",
        "ro:docs/locked",
    ])
    .unwrap();

    assert!(masks.is_hidden("private").unwrap());
    assert!(masks.is_hidden("private/key.md").unwrap());
    assert!(masks.is_hidden("secrets/token.txt").unwrap());
    assert!(!masks.is_hidden("privateer/key.md").unwrap());
    assert!(!masks.is_hidden("published/file.md").unwrap());

    assert!(masks.is_read_only("published").unwrap());
    assert!(masks.is_read_only("published/file.md").unwrap());
    assert!(masks.is_read_only("docs/locked/note.md").unwrap());
    assert!(!masks.is_read_only("published2/file.md").unwrap());
    assert!(!masks.is_read_only("private/key.md").unwrap());
}

#[test]
fn mount_masks_reject_empty_prefixes_and_unknown_modes() {
    assert!(MountMasks::parse_specs([""]).is_err());
    assert!(MountMasks::parse_specs(["hide:"]).is_err());
    assert!(MountMasks::parse_specs(["ro:/"]).is_err());
    assert!(MountMasks::parse_specs(["deny:private"]).is_err());
}
```

- [ ] 
  
  **Step 2: Run the focused failing tests**
  

Run:

```sh
cargo test -p quarry-fuse mount_masks_
```

Expected: FAIL because `MountMasks` does not exist yet.

- [ ] 
  
  **Step 3: Add the mask model**
  

In `crates/quarry-fuse/src/lib.rs`, add this type after `OpenHandle`:

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountMasks {
    hidden: Vec<String>,
    read_only: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskKind {
    Hidden,
    ReadOnly,
}

impl MountMasks {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn parse_specs<I, S>(specs: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut masks = Self::default();
        for spec in specs {
            let spec = spec.as_ref();
            let (kind, path) = parse_mask_spec(spec)?;
            let path = normalize_mount_path(path)?;
            if path.is_empty() {
                return Err(QuarryError::InvalidPath(
                    "mask prefix cannot be empty".to_string(),
                ));
            }
            match kind {
                MaskKind::Hidden => masks.hidden.push(path),
                MaskKind::ReadOnly => masks.read_only.push(path),
            }
        }
        masks.hidden.sort();
        masks.hidden.dedup();
        masks.read_only.sort();
        masks.read_only.dedup();
        Ok(masks)
    }

    pub fn is_hidden(&self, path: &str) -> Result<bool> {
        let path = normalize_mount_path(path)?;
        Ok(self.matches_hidden(&path))
    }

    pub fn is_read_only(&self, path: &str) -> Result<bool> {
        let path = normalize_mount_path(path)?;
        if self.matches_hidden(&path) {
            return Ok(false);
        }
        Ok(self.matches_read_only(&path))
    }

    fn ensure_visible(&self, path: &str) -> Result<()> {
        if self.matches_hidden(path) {
            return Err(QuarryError::NotFound(path.to_string()));
        }
        Ok(())
    }

    fn ensure_writable_path(&self, path: &str) -> Result<()> {
        self.ensure_visible(path)?;
        if self.matches_read_only(path) {
            return Err(QuarryError::Unsupported(
                "FUSE mask is read-only".to_string(),
            ));
        }
        Ok(())
    }

    fn contains_protected_descendant(&self, path: &str) -> bool {
        self.hidden
            .iter()
            .chain(self.read_only.iter())
            .any(|prefix| path_contains_prefix(path, prefix))
    }

    fn matches_hidden(&self, path: &str) -> bool {
        self.hidden.iter().any(|prefix| path_matches_prefix(path, prefix))
    }

    fn matches_read_only(&self, path: &str) -> bool {
        self.read_only
            .iter()
            .any(|prefix| path_matches_prefix(path, prefix))
    }
}

fn parse_mask_spec(spec: &str) -> Result<(MaskKind, &str)> {
    match spec.split_once(':') {
        Some(("hide", path)) => Ok((MaskKind::Hidden, path)),
        Some(("ro", path)) => Ok((MaskKind::ReadOnly, path)),
        Some((mode, _)) => Err(QuarryError::InvalidPath(format!(
            "unsupported mask mode {mode}"
        ))),
        None => Ok((MaskKind::Hidden, spec)),
    }
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

fn path_contains_prefix(path: &str, prefix: &str) -> bool {
    path == prefix || prefix.strip_prefix(path).is_some_and(|rest| rest.starts_with('/'))
}
```

- [ ] 
  
  **Step 4: Run the parser tests**
  

Run:

```sh
cargo test -p quarry-fuse mount_masks_
```

Expected: PASS.

- [ ] 
  
  **Step 5: Commit**
  

```sh
git add crates/quarry-fuse/src/lib.rs crates/quarry-fuse/tests/projection.rs
git commit -m "feat: add FUSE mount mask parser"
```
## Task 2: Enforce Hidden Masks In Reads And Listings
**Files:**

- [ ] 
  
  Modify: `crates/quarry-fuse/src/lib.rs`
  
- [ ] 
  
  Test: `crates/quarry-fuse/tests/projection.rs`
  
- [ ] 
  
  **Step 1: Write failing projection tests**
  

Add this test to `crates/quarry-fuse/tests/projection.rs`:

```rust
#[tokio::test]
async fn projection_hides_masked_paths_from_listings_and_reads() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    for (path, content) in [
        ("private/secret.md", "secret\n"),
        ("privateer/visible.md", "visible\n"),
        ("plans/public.md", "public\n"),
        ("plans/private/hidden.md", "hidden\n"),
    ] {
        store
            .put_document(
                &library.slug,
                path,
                content.as_bytes().to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
    }

    let masks = MountMasks::parse_specs(["private", "hide:plans/private"]).unwrap();
    let projection = FuseProjection::open_with_masks(store, &library.slug, false, masks)
        .await
        .unwrap();

    let root_names = projection
        .list_dir("")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(root_names, vec!["plans".to_string(), "privateer".to_string()]);

    let plan_names = projection
        .list_dir("plans")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(plan_names, vec!["public.md".to_string()]);

    assert!(projection.attr("private").await.is_err());
    assert!(projection.read_file("private/secret.md", 0, 100).await.is_err());
    assert_eq!(
        projection
            .read_file("privateer/visible.md", 0, 100)
            .await
            .unwrap(),
        b"visible\n"
    );
}
```

- [ ] 
  
  **Step 2: Run the focused failing test**
  

Run:

```sh
cargo test -p quarry-fuse projection_hides_masked_paths_from_listings_and_reads
```

Expected: FAIL because `FuseProjection::open_with_masks` does not exist.

- [ ] 
  
  **Step 3: Thread masks into** `FuseProjection`
  

Add a `masks` field:

```rust
pub struct FuseProjection {
    store: QuarryStore,
    library: String,
    read_only: bool,
    masks: MountMasks,
    explicit_dirs: Arc<RwLock<BTreeSet<String>>>,
    handles: Arc<Mutex<HashMap<u64, OpenHandle>>>,
    next_handle: Arc<AtomicU64>,
    invalidation_generation: Arc<AtomicU64>,
    invalidation_notify: Arc<Notify>,
}
```

Replace `FuseProjection::open` with a wrapper plus a mask-aware constructor:

```rust
pub async fn open(store: QuarryStore, library: &str, read_only: bool) -> Result<Self> {
    Self::open_with_masks(store, library, read_only, MountMasks::empty()).await
}

pub async fn open_with_masks(
    store: QuarryStore,
    library: &str,
    read_only: bool,
    masks: MountMasks,
) -> Result<Self> {
    let library_record = store.get_library(library).await?;
    let library = library_record.slug;
    let library_id = library_record.id;
    let mut initial_dirs = BTreeSet::new();
    for document in store.list_documents(&library, None, Some(10_000)).await? {
        initial_dirs.extend(parent_dirs(&document.path));
    }
    for directory in store.list_directories(&library, None).await? {
        if !directory.path.is_empty() {
            initial_dirs.insert(directory.path);
        }
    }
    let invalidation_generation = Arc::new(AtomicU64::new(0));
    let invalidation_notify = Arc::new(Notify::new());
    watch_store_events(
        store.subscribe_events(),
        library_id.clone(),
        invalidation_generation.clone(),
        invalidation_notify.clone(),
    );
    Ok(Self {
        store,
        library,
        read_only,
        masks,
        explicit_dirs: Arc::new(RwLock::new(initial_dirs)),
        handles: Arc::new(Mutex::new(HashMap::new())),
        next_handle: Arc::new(AtomicU64::new(1)),
        invalidation_generation,
        invalidation_notify,
    })
}
```

- [ ] 
  
  **Step 4: Apply hidden checks to reads and listings**
  

Add `self.masks.ensure_visible(&path)?;` to `attr`, `list_dir`, and `read_file` immediately after path normalization.

In `list_dir`, skip hidden documents and directory entries:

```rust
for document in self
    .store
    .list_documents(&self.library, prefix.as_deref(), Some(10_000))
    .await?
{
    if self.masks.is_hidden(&document.path)? {
        continue;
    }
    let remainder = if let Some(prefix) = &prefix {
        document.path.strip_prefix(prefix).unwrap_or(&document.path)
    } else {
        &document.path
    };
    if remainder.is_empty() {
        continue;
    }
    if let Some((dir, _)) = remainder.split_once('/') {
        entries.entry(dir.to_string()).or_insert(FuseDirEntry {
            name: dir.to_string(),
            kind: FuseNodeKind::Directory,
            size: 0,
        });
    } else {
        entries.insert(
            remainder.to_string(),
            FuseDirEntry {
                name: remainder.to_string(),
                kind: FuseNodeKind::File,
                size: document.byte_size,
            },
        );
    }
}
```

Add equivalent `if self.masks.is_hidden(dir)? { continue; }` checks in the loops over `explicit_dirs` and `store.list_directories`.

- [ ] 
  
  **Step 5: Run the hidden projection test**
  

Run:

```sh
cargo test -p quarry-fuse projection_hides_masked_paths_from_listings_and_reads
```

Expected: PASS.

- [ ] 
  
  **Step 6: Commit**
  

```sh
git add crates/quarry-fuse/src/lib.rs crates/quarry-fuse/tests/projection.rs
git commit -m "feat: hide masked FUSE paths"
```
## Task 3: Enforce Read-Only Masks For Mutations
**Files:**

- [ ] 
  
  Modify: `crates/quarry-fuse/src/lib.rs`
  
- [ ] 
  
  Test: `crates/quarry-fuse/tests/projection.rs`
  
- [ ] 
  
  **Step 1: Write failing read-only projection tests**
  

Add this test to `crates/quarry-fuse/tests/projection.rs`:

```rust
#[tokio::test]
async fn projection_read_only_masks_reject_mutations() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    for (path, content) in [
        ("published/existing.md", "published\n"),
        ("drafts/open.md", "open\n"),
    ] {
        store
            .put_document(
                &library.slug,
                path,
                content.as_bytes().to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
    }

    let masks = MountMasks::parse_specs(["ro:published"]).unwrap();
    let projection = FuseProjection::open_with_masks(store.clone(), &library.slug, false, masks)
        .await
        .unwrap();

    assert_eq!(
        projection
            .read_file("published/existing.md", 0, 100)
            .await
            .unwrap(),
        b"published\n"
    );

    assert!(projection.open_file_for_write("published/existing.md").await.is_err());
    assert!(projection.set_len("published/existing.md", 0).await.is_err());
    assert!(projection.unlink("published/existing.md").await.is_err());
    assert!(projection.mkdir("published/new").await.is_err());
    assert!(projection
        .set_directory_metadata("published", Some(0o700), None)
        .await
        .is_err());
    assert!(projection
        .rename("published/existing.md", "drafts/moved.md")
        .await
        .is_err());
    assert!(projection
        .rename("drafts/open.md", "published/open.md")
        .await
        .is_err());

    let handle = projection.create_file("drafts/new.md").await.unwrap();
    projection.write_handle(handle, 0, b"new\n").await.unwrap();
    projection.release_handle(handle).await.unwrap();
    assert_eq!(
        store
            .get_document(&library.slug, "drafts/new.md")
            .await
            .unwrap()
            .content,
        b"new\n"
    );
}
```

Add this test for directory rename safety:

```rust
#[tokio::test]
async fn projection_rejects_directory_rename_containing_protected_descendants() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "workspace/private/secret.md",
            b"secret\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "workspace/public.md",
            b"public\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let masks = MountMasks::parse_specs(["workspace/private"]).unwrap();
    let projection = FuseProjection::open_with_masks(store, &library.slug, false, masks)
        .await
        .unwrap();

    assert!(projection.rename("workspace", "renamed").await.is_err());
}
```

- [ ] 
  
  **Step 2: Run the focused failing tests**
  

Run:

```sh
cargo test -p quarry-fuse projection_read_only_masks_reject_mutations
cargo test -p quarry-fuse projection_rejects_directory_rename_containing_protected_descendants
```

Expected: FAIL because mutating methods only honor the global read-only flag.

- [ ] 
  
  **Step 3: Add path-specific write guards**
  

In `impl FuseProjection`, keep `ensure_writable` for global read-only and add:

```rust
fn ensure_writable_path(&self, path: &str) -> Result<()> {
    self.ensure_writable()?;
    self.masks.ensure_writable_path(path)
}

fn ensure_rename_allowed(&self, from_path: &str, to_path: &str) -> Result<()> {
    self.ensure_writable_path(from_path)?;
    self.ensure_writable_path(to_path)?;
    if self.masks.contains_protected_descendant(from_path) {
        return Err(QuarryError::Unsupported(
            "FUSE mask is read-only for protected descendant".to_string(),
        ));
    }
    Ok(())
}
```

- [ ] 
  
  **Step 4: Replace mutating method guards**
  

For methods taking a path, normalize first and then call the path-specific guard:

```rust
let path = normalize_mount_path(path)?;
self.ensure_writable_path(&path)?;
```

Apply this to:

- `create_file`
  
- `open_file_for_write`
  
- `open_file_for_write_truncating`
  
- `mkdir_with_optional_mode`
  
- `set_directory_metadata`
  
- `unlink`
  
- `rmdir`
  
- `set_len`
  

For handle methods, guard the handle path before modifying or committing:

```rust
self.ensure_writable()?;
let mut handles = self.handles.lock().await;
let handle = handles
    .get_mut(&handle_id)
    .ok_or_else(|| QuarryError::NotFound(format!("file handle {handle_id}")))?;
self.masks.ensure_writable_path(&handle.path)?;
```

Apply this pattern to `write_handle` and `set_handle_len`. In `commit_handle`, add:

```rust
self.ensure_writable_path(&handle.path)?;
```

For `rename`, replace the initial global write check with:

```rust
let from_path = normalize_mount_path(from_path)?;
let to_path = normalize_mount_path(to_path)?;
self.ensure_rename_allowed(&from_path, &to_path)?;
```

- [ ] 
  
  **Step 5: Run the read-only projection tests**
  

Run:

```sh
cargo test -p quarry-fuse projection_read_only_masks_reject_mutations
cargo test -p quarry-fuse projection_rejects_directory_rename_containing_protected_descendants
```

Expected: PASS.

- [ ] 
  
  **Step 6: Run existing FUSE projection tests**
  

Run:

```sh
cargo test -p quarry-fuse
```

Expected: PASS.

- [ ] 
  
  **Step 7: Commit**
  

```sh
git add crates/quarry-fuse/src/lib.rs crates/quarry-fuse/tests/projection.rs
git commit -m "feat: enforce read-only FUSE masks"
```
## Task 4: Add CLI Mask Parsing And Mount Wiring
**Files:**

- [ ] 
  
  Modify: `crates/quarry-cli/src/lib.rs`
  
- [ ] 
  
  Modify: `crates/quarry-fuse/src/lib.rs`
  
- [ ] 
  
  **Step 1: Write failing CLI parser test**
  

Add this test to the `#[cfg(test)]` module in `crates/quarry-cli/src/lib.rs`:

```rust
#[test]
fn mount_accepts_repeated_masks_and_keeps_global_read_only() {
    let cli = Cli::try_parse_from([
        "quarry",
        "mount",
        "notes",
        "/tmp/quarry-mount",
        "--mask",
        "private",
        "--mask",
        "ro:published",
        "--read-only",
    ])
    .unwrap();

    let Command::Mount(command) = cli.command else {
        panic!("expected mount command");
    };
    assert_eq!(command.mask, vec!["private".to_string(), "ro:published".to_string()]);
    assert!(command.read_only);
}
```

- [ ] 
  
  **Step 2: Run the focused failing CLI test**
  

Run:

```sh
cargo test -p quarry-cli mount_accepts_repeated_masks_and_keeps_global_read_only
```

Expected: FAIL because `MountCommand` has no `mask` field.

- [ ] 
  
  **Step 3: Add** `mount_library_with_masks` **in** `quarry-fuse`
  

In `crates/quarry-fuse/src/lib.rs`, add a wrapper while preserving the existing public function:

```rust
pub async fn mount_library(
    store: QuarryStore,
    library: &str,
    mountpoint: &Path,
    read_only: bool,
) -> Result<()> {
    mount_library_with_masks(store, library, mountpoint, read_only, MountMasks::empty()).await
}

pub async fn mount_library_with_masks(
    store: QuarryStore,
    library: &str,
    mountpoint: &Path,
    read_only: bool,
    masks: MountMasks,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_mount::mount_library(store, library, mountpoint, read_only, masks).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, library, mountpoint, read_only, masks);
        Err(QuarryError::Unsupported(
            "Quarry phase-one FUSE mounts are Linux-only".to_string(),
        ))
    }
}
```

Update the Linux helper signature and projection construction:

```rust
pub async fn mount_library(
    store: QuarryStore,
    library: &str,
    mountpoint: &Path,
    read_only: bool,
    masks: MountMasks,
) -> Result<()> {
    tokio::fs::create_dir_all(mountpoint).await?;
    let projection = FuseProjection::open_with_masks(store, library, read_only, masks).await?;
    // keep the existing mount option and session logic unchanged
}
```

Add `MountMasks` to the Linux module `use super::{...}` list.

- [ ] 
  
  **Step 4: Parse masks in the CLI and pass them to FUSE**
  

Change the FUSE import:

```rust
use quarry_fuse::{mount_library_with_masks, MountMasks};
```

Add the field to `MountCommand`:

```rust
#[arg(long = "mask")]
mask: Vec<String>,
```

In `Command::Mount`, parse once before starting FUSE:

```rust
let masks = MountMasks::parse_specs(command.mask.iter())?;
```

Pass `masks` into both mount branches. For the `--serve-addr` branch, clone before moving into the selected future:

```rust
let masks = MountMasks::parse_specs(command.mask.iter())?;
if let Some(addr) = command.serve_addr {
    let mount_store = store.clone();
    tokio::select! {
        result = mount_library_with_masks(
            mount_store,
            &command.library,
            &command.mountpoint,
            command.read_only,
            masks,
        ) => result?,
        result = serve(store, addr) => result?,
    }
} else {
    mount_library_with_masks(
        store,
        &command.library,
        &command.mountpoint,
        command.read_only,
        masks,
    )
    .await?;
}
```

- [ ] 
  
  **Step 5: Run CLI parser tests**
  

Run:

```sh
cargo test -p quarry-cli mount_accepts_repeated_masks_and_keeps_global_read_only
```

Expected: PASS.

- [ ] 
  
  **Step 6: Run affected package tests**
  

Run:

```sh
cargo test -p quarry-cli
cargo test -p quarry-fuse
```

Expected: PASS.

- [ ] 
  
  **Step 7: Commit**
  

```sh
git add crates/quarry-cli/src/lib.rs crates/quarry-fuse/src/lib.rs
git commit -m "feat: wire FUSE masks into quarry mount"
```
## Task 5: Document And Verify
**Files:**

- [ ] 
  
  Modify: `docs/operations/fuse.md`
  
- [ ] 
  
  **Step 1: Update FUSE docs**
  

In `docs/operations/fuse.md`, after the current `--read-only` paragraph, add:

````markdown
Mount-local masks can hide or protect path prefixes without changing the Library:

```sh
cargo run -p quarry -- mount notes /mnt/notes \
  --mask private \
  --mask hide:secrets \
  --mask ro:published
```

Mask rules:

- `--mask <path>` hides a prefix by default.
- `--mask hide:<path>` hides a prefix explicitly.
- `--mask ro:<path>` makes a prefix read-only in an otherwise writable mount.
- Hidden paths behave as if they do not exist.
- Hidden masks win over read-only masks.
- Masks are mount-local and do not affect REST, Git, other CLI commands, or other mounts.
````

- [ ] 
  
  **Step 2: Run package tests**
  

Run:

```sh
cargo test -p quarry-fuse
cargo test -p quarry-cli
```

Expected: PASS.

- [ ] 
  
  **Step 3: Run workspace check**
  

Run:

```sh
cargo check --workspace
```

Expected: PASS.

- [ ] 
  
  **Step 4: Commit docs**
  

```sh
git add docs/operations/fuse.md
git commit -m "docs: document FUSE mount masks"
```
## Self-Review
- Spec coverage: The plan covers mount-local scope, prefix-only masks, default hidden masks, explicit `hide:` masks, `ro:` masks, hidden-over-read-only precedence, empty prefix rejection, hidden `ENOENT` behavior, read-only mutation rejection, existing global `--read-only`, docs, and tests.
  
- Placeholder scan: The plan contains concrete commands, expected outcomes, file paths, and code snippets. It does not use placeholder markers or unspecified implementation steps.
  
- Type consistency: The plan consistently uses `MountMasks`, `MountMasks::parse_specs`, `FuseProjection::open_with_masks`, and `mount_library_with_masks`.
