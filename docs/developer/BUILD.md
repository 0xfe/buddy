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

## Hosted release workflow (GitHub Actions)

This repo publishes release binaries from tag pushes (`v*`) via
`.github/workflows/release.yml`.

Recommended flow:

```bash
# 1) update version
make bump-patch                  # or bump-minor / bump-major / bump-set VERSION=x.y.z

# 2) run local gate before tagging
make release

# 3) auto-commit Cargo version files if needed, then create + push release tag
make release-tag                 # pushes to origin by default
# optional: make release-tag RELEASE_REMOTE=<remote>
```

`make release-tag` behavior:

- allows dirty state only in `Cargo.toml` / `Cargo.lock`,
- commits staged version updates as `release: v<version>` when needed,
- rejects detached-head releases and existing tags,
- pushes current branch, then pushes `v<version>` tag.

When the tag is pushed, GitHub Actions builds and uploads artifacts for:

- Linux `amd64` (`ubuntu-24.04`)
- Linux `arm64` (`ubuntu-24.04-arm`)
- macOS `amd64` (`macos-14`, cross-target `x86_64-apple-darwin`)
- macOS `arm64` (`macos-14`)

Artifacts are published to the GitHub Release for that tag as:

- `buddy-v<version>-<host-triple>.tar.gz`
- `buddy-v<version>-<host-triple>.tar.gz.sha256` (when checksum tool exists)

Workflow jobs:

- `validate`: runs `make check` and installer smoke tests on push/PR.
- `release-artifacts`: runs only on `v*` tags and builds per OS/arch matrix.
- `publish-release`: attaches built artifacts to the GitHub Release.

## Local release reproduction workflow

Use this when reproducing release issues seen in GitHub Actions.

```bash
# start clean
make clean

# run the same local quality gate
make check

# build local artifact exactly as release packaging does
make release-artifacts

# validate installer against local dist artifacts
make test-installer-smoke
```

Notes:

- `make release-artifacts` packages for the current machine host triple by default.
- `make release-artifacts BUILD_TARGET=<triple>` packages for an explicit target
  (for example `x86_64-apple-darwin` on Apple Silicon CI hosts).
- To reproduce a specific CI artifact issue, run the same commands on a host
  matching that target (Linux/macOS and `amd64`/`arm64`).
- Optional suites remain explicit and are not part of default CI release gates:
  `make test-ui-regression`, `make test-model-regression`.

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
  - executes `make release-artifacts` on Linux/macOS for `amd64` + `arm64`
  - uploads generated artifacts
- `publish-release` job:
  - runs for `v*` tags
  - downloads uploaded artifacts
  - publishes assets to the GitHub Release for that tag
