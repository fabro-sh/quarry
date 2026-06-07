use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn cli_default_debug_logs_stay_on_stderr_and_stdout_stays_payload_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let source = temp.path().join("hello.md");
    std::fs::write(&source, "hello from cli\n").unwrap();

    let output = quarry_command()
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", root.display())
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("logging.initialized"));

    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "put",
            "notes",
            "notes/hello.md",
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("document.put.started"));
    let written: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(written["document"]["path"], "notes/hello.md");

    let output = quarry_command()
        .args(["--root", root.to_str().unwrap(), "list", "notes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let output = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "get",
            "notes",
            "notes/hello.md",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello from cli\n");

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
            store
                .record_conflict(
                    "notes",
                    "notes/hello.md",
                    written["version"]["id"].as_str().map(ToString::to_string),
                    None,
                )
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
            "notes",
            &conflict_id,
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let resolved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resolved["status"], "resolved");
}

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
            .raw_version_history("assets", "blobs/large.bin")
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

#[cfg(unix)]
#[test]
fn serve_sigterm_exits_and_removes_lock_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let lock_path = root.join("quarry.lock");
    let addr = unused_loopback_addr();
    let mut child = quarry_command()
        .args([
            "--root",
            root.to_str().unwrap(),
            "serve",
            "--addr",
            &addr.to_string(),
        ])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for_path(&lock_path, Duration::from_secs(5));
    wait_for_tcp(addr, Duration::from_secs(5));
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());

    let deadline = Instant::now() + Duration::from_secs(5);
    let exit_status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("quarry serve did not exit after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).unwrap();
    }
    assert!(
        exit_status.success(),
        "quarry serve should exit gracefully, got {exit_status:?}, stderr: {stderr}"
    );
    assert!(
        !lock_path.exists(),
        "quarry.lock should be removed on SIGTERM"
    );
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_quarry"));
    command.env_remove("RUST_LOG");
    command.env_remove("QUARRY_LOG_FORMAT");
    command
}

fn unused_loopback_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn wait_for_path(path: &std::path::Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

fn wait_for_tcp(addr: std::net::SocketAddr, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {addr}");
}
