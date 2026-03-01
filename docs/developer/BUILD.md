# Build, Test, and Release

This project treats `make` as the primary developer and release interface.

## Local workflow

Core targets:

```bash
make build          # cargo build --release
make build-debug    # cargo build
make test           # cargo test
make test-installer-smoke # offline installer smoke test
make check          # fmt --check + clippy -D warnings + test
make install        # install to ~/.local/bin
make install-from-release # curl-style install script
```

Other useful targets:

```bash
make run            # cargo run
make run-exec PROMPT="list files"
make clean
make help
```

## Test suites

Default offline suite:

```bash
make test
```

Optional/explicit suites:

```bash
# tmux-based UI integration regressions
make test-ui-regression

# live provider/model regressions
make test-model-regression

# optional parser/property coverage
cargo test --features fuzz-tests
```

More detail:
- [docs/developer/testing-ui.md](testing-ui.md)
- [docs/developer/model-regression-tests.md](model-regression-tests.md)

## Release artifacts

Create release archive + checksum:

```bash
make release-artifacts
```

Artifacts:
- `dist/buddy-v<version>-<host-triple>.tar.gz`
- `dist/buddy-v<version>-<host-triple>.tar.gz.sha256` (when `shasum` or `sha256sum` exists)

Full local release gate:

```bash
make release
```

Installer details:
- [docs/developer/install.md](install.md)

## Version helpers

```bash
make version
make bump-patch
make bump-minor
make bump-major
make bump-set VERSION=x.y.z
```

These targets wrap `scripts/bump-version.sh`.

## Embedded build metadata

`build.rs` injects compile-time metadata into the binary:

- package version (`CARGO_PKG_VERSION`)
- git commit hash (`git rev-parse --short=12 HEAD`, with fallback `unknown`)
- build timestamp (UTC RFC3339 via `date -u`, with fallback `unix:<secs>`)

Metadata is shown in:

- startup banner (interactive mode)
- `buddy --version`
- CLI help footer

## GitHub Actions release automation

Workflow: `.github/workflows/release.yml`

- `validate` job:
  - runs on pushes and pull requests (Linux + macOS)
  - executes `make check`
- `release-artifacts` job:
  - runs for tags matching `v*`
  - executes `make release-artifacts` on Linux + macOS
  - uploads generated artifacts
- `publish-release` job:
  - runs for `v*` tags
  - downloads uploaded artifacts
  - publishes assets to the GitHub Release for that tag
