# Install buddy

Buddy ships release tarballs for supported host targets and includes a curl-style installer.

## Quick install

```bash
curl -fsSL https://raw.githubusercontent.com/0xfe/buddy/main/scripts/install.sh | bash
```

Default behavior:

1. Detect host platform/architecture.
2. Download the matching release artifact from GitHub.
3. Verify checksum when available.
4. Install `buddy` to `~/.local/bin`.
5. Run `buddy init` when possible (or print first-run guidance when non-interactive).

## Installer options

```bash
./scripts/install.sh --help
```

Common options:

- `--version <vX.Y.Z|X.Y.Z>`: install a specific release version.
- `--repo <owner/repo>`: override release source.
- `--install-dir <path>`: override install destination (`~/.local/bin` by default).
- `--force`: replace existing binary when a different version is installed.
- `--skip-init`: skip post-install `buddy init`.
- `--from-dist <dir>`: offline install from a local `dist/` directory.

## Idempotent behavior

- If the same version is already installed, the script exits successfully without replacing the binary.
- If a different version is installed, the script requires `--force` and prints a clear error.

## Offline/local smoke install

Build artifacts locally, then install from `dist/`:

```bash
make release-artifacts
./scripts/install.sh --from-dist dist --version "v$(make -s version)"
```

Or run the built-in smoke target:

```bash
make test-installer-smoke
```

## Troubleshooting

- Ensure `~/.local/bin` is on `PATH`.
- If checksum verification is skipped, install `shasum` or `sha256sum`.
- On unsupported platforms, build from source with `make build` and `make install`.
