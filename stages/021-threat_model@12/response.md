{
  "job_id": "threat:012-packaging-71669691",
  "component": "packaging",
  "entryPoints": [
    "scripts/update-homebrew-formula.rb:14 — ARGV input: formula_path, release_tag, and four SHA256 values supplied by the release workflow (release.yml:463).",
    "scripts/update-homebrew-formula.rb:47 — File.read(formula_path): reads a file whose path comes from ARGV[0].",
    "Formula/quarry.rb:9 — Homebrew downloads release tarballs over HTTPS from github.com release assets at install time on end-user machines.",
    "Formula/quarry.rb:28 — Install-time inspection of the unpacked tarball: File.exist?(\"quarry\") and Dir[\"*/quarry\"].first on archive contents.",
    "Dockerfile:40 — Build-arg QUARRY_FEATURES flows into a shell command line in the builder stage.",
    "Dockerfile:78 — runtime-prebuilt target copies a prebuilt binary from tmp/docker-context/${TARGETARCH}/quarry, content produced outside the Dockerfile and trusted wholesale."
  ],
  "sinks": [
    "scripts/update-homebrew-formula.rb:56 — File.write(formula_path, updated_formula): writes generated Ruby (a formula later executed by brew) to an ARGV-controlled path with ARGV-interpolated content.",
    "scripts/update-homebrew-formula.rb:28 — release_tag interpolated unescaped into Ruby double-quoted url strings; tag is validated only for a leading 'v' (line 16), not for quotes or newlines, so a crafted tag can break out of the string literal in the generated formula.",
    "Formula/quarry.rb:31 — system \"cargo\", \"install\", *std_cargo_args: builds from source when head or no prebuilt binary found; executed on end-user machines by brew.",
    "Formula/quarry.rb:33 — bin.install release_binary: installs a binary selected from the downloaded tarball (Dir[\"*/quarry\"].first) with no verification beyond the tarball sha256.",
    "Dockerfile:42-46 — Shell expansion of $QUARRY_FEATURES inside a RUN if-block, passed to cargo build --features (quoted, but the value steers compilation).",
    "Dockerfile:72 — Container CMD runs chown -R on $QUARRY_ROOT and execs gosu quarry quarry server start --addr 0.0.0.0:${PORT:-7831} via sh -c with environment-derived values.",
    "Dockerfile:69 — HEALTHCHECK curls http://127.0.0.1:${PORT:-7831}/v1/health inside the container."
  ],
  "assumptions": [
    "scripts/update-homebrew-formula.rb:16 — Assumes a tag starting with 'v' is safe to embed in generated Ruby source; no escaping or character validation beyond the prefix.",
    "scripts/update-homebrew-formula.rb:20 — Assumes 64-hex SHA256 strings came from genuine release artifacts; the script never verifies the digests against the actual tarballs, so integrity rests entirely on the release.yml job that computed them.",
    "scripts/update-homebrew-formula.rb:48 — Assumes the existing formula matches the expected homepage/license/head/install regex structure; a malformed or adversarial formula aborts or yields spliced output without further sanity checks.",
    "Formula/quarry.rb:10 — Assumes the pinned sha256 digests genuinely correspond to the published release assets; tampered assets whose digests were updated in the same commit would pass brew's verification.",
    "Dockerfile:78 — Assumes tmp/docker-context/${TARGETARCH}/quarry staged by CI is the authentic release binary; no signature or checksum verification of the prebuilt artifact.",
    "Dockerfile:17 and Dockerfile:41 — Assumes bun.lock / Cargo.lock pin trustworthy dependencies (bun install --frozen-lockfile, cargo build --locked); supply-chain integrity is delegated to the lockfiles."
  ],
  "trustBoundaries": [
    ".github/workflows/release.yml:463 — CI (with contents:write token) invokes the Ruby script with the tag from github.ref_name and digests from downloaded build artifacts, then commits and pushes the generated formula to main: CI-produced data becomes committed source.",
    "scripts/update-homebrew-formula.rb:54 — Inputs (tag, checksums, existing formula text) are spliced into Ruby source that end users' brew installations will later execute.",
    "Formula/quarry.rb:33 — A remote release artifact crosses from the network onto end-user machines as an installed executable in PATH.",
    "Dockerfile:72 — Runtime environment variables ($PORT, $QUARRY_ROOT) cross from the PaaS-controlled environment into a sh -c command line and the server's bind address.",
    "Dockerfile:58-72 — The container starts as root (chown) then drops to uid 1000 via gosu; a privilege boundary is crossed at every container start."
  ],
  "hotFiles": [
    "scripts/update-homebrew-formula.rb",
    "Formula/quarry.rb",
    "Dockerfile",
    ".github/workflows/release.yml"
  ]
}