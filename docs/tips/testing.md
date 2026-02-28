# Testing Tips

## Default test loop
```bash
cargo build
cargo test
```

## Faster targeted loops
```bash
cargo test <module_or_test_name>
cargo test <module_or_test_name> -- --nocapture
```

## When changing docs only
- Code tests are usually unnecessary for docs-only edits.
- Still run lightweight sanity checks when needed (for example `cargo test` before large docs-driven refactors).

## Reliability expectations
- Tests must run offline (no network dependency).
- Keep new tests deterministic and local-environment tolerant.
- Place unit tests near implementation with `#[cfg(test)]` modules.
