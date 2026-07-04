# Spend Cache Design

**Date:** 2026-07-04
**Status:** Approved

## Problem

`aix spend` fires a fresh HTTP request to `/user/info` on every invocation. The LiteLLM
response already contains spend data for *all* API keys associated with that user account,
so calling `aix spend` for three profiles that belong to the same LiteLLM user makes three
identical network round-trips. Additionally, spend data changes slowly — a TTL-based disk
cache eliminates redundant calls within a normal working session.

## Goals

- Skip the network when cached spend data is fresh (default TTL: 1 hour).
- Share a single cache entry across all API keys that belong to the same LiteLLM user
  account, so calling `aix spend` for any one of those keys warms the cache for the others.
- Let the user override or clear the cache explicitly.
- No new crate dependencies.

## Non-goals

- Caching responses for commands other than `spend`.
- Rate-limit back-off (the cache is not a rate-limit workaround).
- Sharing cache entries across different gateway endpoints.

## Cache layout

XDG cache dir (`~/.cache/aix/` on Linux, platform-appropriate elsewhere via the
`directories` crate, which is already a dependency):

```
~/.cache/aix/
  {base_url_hash}/          ← first 16 hex chars of SHA-256(base_url)
    index.json              ← { "X4Y5": "u123", "A1B2": "u123", "C3D4": "u456" }
    u123.json               ← { "fetched_at": 1720000000, "data": { ...response... } }
    u456.json               ← { "fetched_at": 1720000001, "data": { ...response... } }
```

- **`index.json`** maps the *last 4 characters* of an API key to a user-id string.
  The suffix convention matches what LiteLLM uses internally when masking keys
  (`sk-...XXXX`), so it is consistent with the matching logic already in `spend.rs`.
- **`{user_id}.json`** stores a timestamped envelope around the raw API response.
- **User-id derivation:** taken from the top-level `"user_id"` field of the `/user/info`
  response. If that field is absent (unexpected gateway variant), a synthetic id
  `"key:{sha256(api_key)[..16]}"` is used silently — effectively falling back to
  per-key caching.
- On every successful fetch, *all* key suffixes found in `response["keys"]` are written
  into the index pointing at the same user-id, warming future lookups.

## New module: `src/cache.rs`

Public surface:

```rust
pub struct Cache {
    base_dir: PathBuf,   // XDG cache dir / "aix"
    ttl_secs: u64,       // 0 = no expiry
    disabled: bool,
}

impl Cache {
    pub fn from_config(cfg: &Config) -> Result<Self, AixError>;

    /// Return cached response if present and fresh.
    pub fn get(&self, base_url: &str, api_key: &str) -> Option<serde_json::Value>;

    /// Persist a fresh response and update the index.
    pub fn put(&self, base_url: &str, api_key: &str, data: &serde_json::Value)
        -> Result<(), AixError>;

    /// Remove all files in the aix cache directory.
    pub fn clear(&self) -> Result<usize, AixError>;   // returns file count removed
}
```

Internal helpers (private):

- `endpoint_dir(base_url) -> PathBuf` — hashes base_url, returns the endpoint subdir.
- `load_index(dir) -> HashMap<String, String>` — reads `index.json`, returns empty map on
  missing/corrupt file.
- `save_index(dir, index) -> Result<()>` — writes `index.json` atomically (write to a temp
  file, then rename).
- `derive_user_id(api_key, data) -> String` — extracts `data["user_id"]` or falls back to
  synthetic.
- `key_suffix(api_key) -> String` — last 4 chars, same convention as `spend.rs`.

All I/O uses `std::fs`, `serde_json`, and `std::time::SystemTime`. No new dependencies.

## Config changes

New optional section in `aix.toml` (and equivalents):

```toml
[cache]
ttl_secs = 3600   # default when section is absent. 0 = no expiry.
disabled = false  # default when section is absent.
```

`Config` gains:

```rust
#[serde(default)]
pub cache: CacheConfig,
```

```rust
#[derive(Debug, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_ttl")]
    pub ttl_secs: u64,       // default: 3600
    #[serde(default)]
    pub disabled: bool,      // default: false
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { ttl_secs: default_ttl(), disabled: false }
    }
}

fn default_ttl() -> u64 { 3600 }
```

## CLI changes

### `aix spend`

New flag:

```
--no-cache    Bypass the cache for this invocation; always fetch fresh data
```

Behaviour: if `--no-cache` is passed, `Cache::get` is skipped (cache is still *written*
on success, so the next call without `--no-cache` benefits). If `cache.disabled = true`,
both read and write are skipped.

### `aix cache clear`

New top-level subcommand:

```
aix cache clear    Delete all cached response files
```

Output: `Cleared N cached file(s).` or `Cache already empty.`

`aix cache` with no subcommand prints help (clap default behaviour).

## Data flow for `aix spend`

```
resolve profile → api_key, base_url
        │
        ├─ --no-cache? ──────────────────────────────────────────────────┐
        │                                                                 │
        ├─ cache.disabled? ───────────────────────────────────────────── ┤
        │                                                                 │
        ▼                                                                 │
  Cache::get(base_url, api_key)                                          │
    load_index → suffix → user_id                                        │
    if user_id found:                                                     │
      load {user_id}.json                                                 │
      if fetched_at + ttl_secs > now → return cached data                │
        │                                                                 │
        ▼ (stale or not in index)                          ◄─────────────┘
  client.user_info() → response
        │
        ├─ cache.disabled? → skip write
        │
        ▼
  Cache::put(base_url, api_key, response)
    derive_user_id → user_id
    write {user_id}.json atomically
    update index: all key suffixes in response["keys"] → user_id
        │
        ▼
  print_human / print_json
```

## Error handling

- Cache read errors (corrupt JSON, permissions) are silently ignored — the command falls
  through to a live fetch.
- Cache write errors are also silent — the command still succeeds; the response is printed
  normally.
- `aix cache clear` surfaces I/O errors to the user (it is explicitly a destructive action,
  so silent failure would be confusing).

## Testing

- Unit tests in `src/cache.rs`: index read/write, TTL expiry, user-id derivation (with and
  without `user_id` field), `clear` returns correct file count.
- Integration test in `tests/spend_test.rs`: verify that a second `aix spend` invocation
  against a mock server that is set to reject the second request still succeeds (served from
  cache).
