# Security notes

How aix handles API keys, ChatGPT credentials, and config files. See also [Secret sources](configuration.md#secret-sources).

## ChatGPT OAuth credentials

The auth store defaults to the platform's local application-data directory under
`aix/auth`; set `AIX_AUTH_DIR` to use another location. On Unix, the auth
directory is mode `0700` and credential files are mode `0600`. The store keeps
the issued client ID, verified account identity, scopes, and OAuth tokens as
local JSON. They are protected by filesystem permissions, **not encrypted at
rest**. Protect the account and storage device accordingly.

Login uses a loopback callback with OAuth state and PKCE verification. The ID
token signature, issuer, audience, expiry, and nonce are checked before the
identity is saved. Refreshes serialize per profile and persist rotated tokens
atomically. Status is offline and omits token values. Logout clears local token
values even if remote revocation cannot be confirmed; in that case aix prints a
warning to disconnect aix from ChatGPT settings if needed.

## Prefer indirect secret sources

Avoid `direct` keys in config files that live on disk. Prefer, in order:

1. **`file`** — secret lives in a permission-restricted file (e.g., Nix `agenix`, Docker secrets, systemd credentials). No shell expansion of the value.
2. **`env`** — key is injected by your shell session, CI system, or secrets manager at launch time. Never written to disk by `aix`.
3. **`command`** — delegates to a CLI secrets manager (`pass`, `1password`, `bitwarden-cli`). Subprocess is launched per invocation; output is never logged.
4. **`direct`** — plain string in the config. Acceptable only for throwaway keys in local-only, non-shared configs.

## Never commit real keys

The config file can contain real file paths and env var names — those are fine to commit. Only `direct` values embed the secret itself. Add your config file to `.gitignore` if it contains any `direct` keys:

```gitignore
# If your config has direct keys, keep it out of git
.config/aix/aix.toml
```

If you use the Nix deployment, the generated config uses `file` sources pointing to `/run/secrets/aix/*` — those paths are safe to commit.

## .env files are opt-in

`aix` does **not** automatically load `.env` files from the working directory. Loading happens only when you explicitly list files under `env_files` in your config. Environment variables set before `aix` runs always win over `.env` values.

## In-memory secrets are zeroized on drop

Resolved secret values use memory types that zero their contents when dropped. API keys are not written to disk by aix or printed in logs. ChatGPT tokens are intentionally persisted in the owner-only auth store described above; dry-run never prints token values and shows environment variable names, not values.
