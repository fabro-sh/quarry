# Linux Install Path

Phase one ships as a single `quarry` binary. GitHub releases attach tarballs for Linux x86_64, Linux ARM64, and macOS (Apple silicon and Intel): `.github/workflows/release-nightly.yml` cuts a nightly tag when `main` has new commits, and `.github/workflows/release.yml` publishes every validated stable or nightly tag. Stable releases deploy production and then refresh `Formula/quarry.rb`. A Debian package can be added later; this documented Linux path is the supported install route for source builds.

Prerequisites:

- Rust stable toolchain with Cargo.
- `git`.
- `fuse3` and `fusermount3` for Linux mounts.

Ubuntu/Debian prerequisites:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git fuse3
```

Build and install:

```sh
cargo build --release -p quarry
install -Dm755 target/release/quarry ~/.local/bin/quarry
```

Install from a release tarball:

```sh
tar -xzf quarry-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 quarry-x86_64-unknown-linux-gnu/quarry ~/.local/bin/quarry
```

Quick smoke test:

```sh
quarry server --root .quarry init
printf 'hello\n' >/tmp/quarry-hello.md
quarry put notes notes/hello.md /tmp/quarry-hello.md
quarry get notes notes/hello.md
```

FUSE mount smoke test on Linux:

```sh
mkdir -p /tmp/quarry-notes
quarry mount notes /tmp/quarry-notes --read-only
```

Unmount from another shell:

```sh
fusermount3 -u /tmp/quarry-notes
```
