# CI and Release Workflow

This project uses `make` as the first-class build/test/release interface, with GitHub Actions handling release-tag artifact publishing.

## Local workflow

Core targets:

```bash
make build          # release build
make test           # cargo test
make check          # fmt --check + clippy -D warnings + test
make install        # install into ~/.local/bin
```

Release packaging:

```bash
make release-artifacts
```

This produces:

- `dist/buddy-v<version>-<host-triple>.tar.gz`
- `dist/buddy-v<version>-<host-triple>.tar.gz.sha256` (when `shasum` or `sha256sum` exists)

Full release gate locally:

```bash
make release
```

## Version bump helpers

`scripts/bump-version.sh` is wrapped by these make targets:

```bash
make bump-patch
make bump-minor
make bump-major
make bump-set VERSION=x.y.z
```

`make version` prints the current `Cargo.toml` version.

## Embedded build metadata

`build.rs` injects compile-time metadata into the binary:

- package version (`CARGO_PKG_VERSION`)
- git commit hash (`git rev-parse --short=12 HEAD`, or `unknown`)
- build timestamp (UTC RFC3339 via `date -u`, or `unix:<secs>` fallback)

The CLI exposes this via:

- startup banner metadata line in REPL mode
- `buddy --version` / clap long version output

## GitHub Actions release automation

Workflow: `.github/workflows/release.yml`

- `validate` job:
  - runs on pushes + PRs (Linux + macOS)
  - executes `make check`
- `release-artifacts` job:
  - runs only for tag refs matching `v*`
  - executes `make release-artifacts` (Linux + macOS)
  - uploads generated artifacts
- `publish-release` job:
  - runs only for `v*` tags
  - downloads matrix artifacts
  - publishes them to the GitHub Release for that tag
