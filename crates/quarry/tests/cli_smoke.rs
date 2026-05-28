use std::process::Command;

#[test]
fn cli_conflict_resolve_rejects_conflicts_from_another_library() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    run_quarry(["init", root.to_str().unwrap()]);

    let conflict_id = {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let store = quarry_storage::QuarryStore::open(quarry_storage::StoreConfig {
                db_path: root.join("quarry.db"),
                cas_path: root.join("cas"),
                lock_path: None,
            })
            .await
            .unwrap();
            let library = store.create_library("actions").await.unwrap();
            store.create_library("other").await.unwrap();
            let written = store
                .put_document(
                    &library.slug,
                    "notes/a.md",
                    b"hello\n".to_vec(),
                    serde_json::json!({"content_type":"text/markdown"}),
                    "text/markdown",
                    quarry_core::DocumentSource::Rest,
                    quarry_core::WritePrecondition::None,
                )
                .await
                .unwrap();
            store
                .record_conflict(&library.slug, "notes/a.md", Some(written.version.id), None)
                .await
                .unwrap()
                .id
        })
    };

    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "conflicts",
            "resolve",
            "other",
            &conflict_id,
        ])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "conflicts",
            "resolve",
            "actions",
            &conflict_id,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resolved["status"], "resolved");
}

#[test]
fn cli_backup_restore_reproduces_document_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let backup = temp.path().join("backup");
    let restored = temp.path().join("restored");
    let source = temp.path().join("hello.md");
    std::fs::write(&source, "hello from cli\n").unwrap();

    run_quarry(["init", root.to_str().unwrap()]);
    run_quarry([
        "--root",
        root.to_str().unwrap(),
        "put",
        "notes",
        "notes/hello.md",
        source.to_str().unwrap(),
    ]);
    run_quarry([
        "--root",
        root.to_str().unwrap(),
        "backup",
        backup.to_str().unwrap(),
    ]);
    run_quarry([
        "--root",
        restored.to_str().unwrap(),
        "restore",
        backup.to_str().unwrap(),
    ]);

    let output = quarry_command()
        .args([
            "--root",
            restored.to_str().unwrap(),
            "get",
            "notes",
            "notes/hello.md",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello from cli\n");
}

#[test]
fn cli_backup_restore_preserves_metadata_versions_and_cas_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let backup = temp.path().join("backup");
    let restored = temp.path().join("restored");
    let first = temp.path().join("first.bin");
    let second = temp.path().join("second.bin");
    std::fs::write(
        &first,
        vec![b'a'; quarry_core::INLINE_CONTENT_THRESHOLD + 1],
    )
    .unwrap();
    std::fs::write(
        &second,
        vec![b'b'; quarry_core::INLINE_CONTENT_THRESHOLD + 2],
    )
    .unwrap();

    run_quarry(["init", root.to_str().unwrap()]);
    run_quarry([
        "--root",
        root.to_str().unwrap(),
        "put",
        "assets",
        "blobs/large.bin",
        first.to_str().unwrap(),
    ]);
    run_quarry([
        "--root",
        root.to_str().unwrap(),
        "put",
        "assets",
        "blobs/large.bin",
        second.to_str().unwrap(),
    ]);
    run_quarry([
        "--root",
        root.to_str().unwrap(),
        "backup",
        backup.to_str().unwrap(),
    ]);
    run_quarry([
        "--root",
        restored.to_str().unwrap(),
        "restore",
        backup.to_str().unwrap(),
    ]);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = quarry_storage::QuarryStore::open(quarry_storage::StoreConfig {
            db_path: restored.join("quarry.db"),
            cas_path: restored.join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let document = store
            .get_document("assets", "blobs/large.bin")
            .await
            .unwrap();
        assert_eq!(
            document.content,
            vec![b'b'; quarry_core::INLINE_CONTENT_THRESHOLD + 2]
        );
        assert_eq!(document.version.content_type, "application/octet-stream");
        assert!(document.version.content_hash.is_some());

        let versions = store
            .version_history("assets", "blobs/large.bin")
            .await
            .unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions
            .iter()
            .all(|version| version.content_hash.is_some()));
    });
}

#[test]
fn cli_can_create_and_list_git_peers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    run_quarry(["init", root.to_str().unwrap()]);
    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "git",
            "peer",
            "add",
            "notes",
            repo.to_str().unwrap(),
            "--branch",
            "main",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let peer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(peer["kind"], "git");
    assert_eq!(peer["config"]["repo"], repo.to_str().unwrap());
    assert_eq!(peer["config"]["branch"], "main");

    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "git",
            "peer",
            "list",
            "notes",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let peers: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(peers.as_array().unwrap().len(), 1);
    assert_eq!(peers[0]["id"], peer["id"]);
}

fn run_quarry<const N: usize>(args: [&str; N]) {
    let output = quarry_command().args(args).output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn quarry_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quarry"))
}
