# Deprecation Policy

Buddy still supports a few legacy names/paths for upgrade compatibility.

## Timeline

- Current status: deprecated, still supported.
- Planned removal: after `v0.4`.
- Runtime behavior: one warning per CLI session when deprecated compatibility
  paths are detected.

## Deprecated Compatibility Paths

| Deprecated | Replacement | Notes |
| --- | --- | --- |
| `AGENT_API_KEY` | `BUDDY_API_KEY` | Env alias warning at startup. |
| `AGENT_BASE_URL` | `BUDDY_BASE_URL` | Env alias warning at startup. |
| `AGENT_MODEL` | `BUDDY_MODEL` | Env alias warning at startup. |
| `AGENT_API_TIMEOUT_SECS` | `BUDDY_API_TIMEOUT_SECS` | Env alias warning at startup. |
| `AGENT_FETCH_TIMEOUT_SECS` | `BUDDY_FETCH_TIMEOUT_SECS` | Env alias warning at startup. |
| `agent.toml` | `buddy.toml` | Local/global fallback remains for now. |
| `[api]` config table | `[models.<name>]` + `agent.model` | Legacy table is auto-mapped at load time. |
| `.agentx/` session root | `.buddyx/` | Used only when `.buddyx/` is absent and `.agentx/` exists. |
| Auth `profiles.<name>` records | Auth `providers.<name>` records | Re-run `buddy login` to write provider-scoped credentials. |

## Migration Checklist

1. Rename any `agent.toml` files to `buddy.toml`.
2. Move from `[api]` to `[models.<name>]` plus `agent.model`.
3. Rename all `AGENT_*` env vars to `BUDDY_*`.
4. Move persisted sessions from `.agentx/` to `.buddyx/` if needed.
5. Run `buddy login` for each login-auth provider to refresh auth storage in
   provider-scoped format.
