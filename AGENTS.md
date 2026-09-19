# Repository Guidelines

Guidance for AI assistants working in `floway-cli`.

## Project Overview

`floway-cli` is a Rust CLI (binary `floway`, package `floway-cli`) that configures coding agents to route model traffic through a self-hosted [Floway](https://github.com/Menci/Floway) API gateway. One command (`floway install`) writes provider settings for every selected agent; `floway uninstall` removes exactly the Floway-managed configuration and nothing else; `floway update` re-fetches the model catalog and re-applies it. It is a native Rust port of Floway's original Python agent-setup installers/converters.

Supported agents (kebab-case ids): `claude-code`, `codex`, `oh-my-pi` (id `omp`), `pi` (alias `pi-coding-agent`), `opencode`, `zed`, `vscode`, `deepseek-harness` (id `dsh`).

Behavioral contracts the README promises — changes MUST preserve all of them:

1. **Verify before write**: credentials are checked against `GET /v1/models` before touching any agent config.
2. **Transactional writes**: every config write is same-directory stage + rename.
3. **Owner-only permissions**: mode `0600` for anything carrying the API key (`state.json`, codex `floway-token`, etc.).
4. **Surgical merge/unmerge**: writes preserve unrelated keys, comments (Codex TOML via `toml_edit`), and formatting; uninstall removes exactly the managed subtree and prunes emptied parents.
5. **No surprise package management**: `floway update` only *prints* agent self-update commands; it never runs package managers.
6. **Corruption safety**: present-but-invalid JSON config is rejected, never clobbered (`json_doc::load_or_new` errors).
7. **Non-interactive contract**: piped stdin + `FLOWAY_AGENTS=all|<csv>` works; API-key prompt disables echo.
8. **UI**: respects `NO_COLOR` and non-TTY output.

## Architecture & Data Flow

Binary-only crate (no lib target). All modules are crate-private, declared flat in `src/main.rs`. Fully synchronous — `reqwest` uses `blocking` + `rustls`; no async, no threads.

```
main.rs run()
 ├─ install::run(Options)
 │   ├─ state::Store::load            (credentials + prior agents)
 │   ├─ menu::select_agents           (or --agents / FLOWAY_AGENTS)
 │   ├─ gateway::Client::new(...).fetch_models()  → ModelList  (fails fast)
 │   ├─ for agent: AgentKind::apply(&client, &models)
 │   │    ├─ agents/claude.rs   → json_doc::{load_or_new, ensure_object*, save}
 │   │    ├─ agents/codex.rs    → toml_edit parse, toml_doc::save, write_private_file
 │   │    └─ agents/harness.rs  → json_doc::*, yaml_doc::*, write_private_file
 │   └─ state::Store::save (0600)
 ├─ update_cmd:    Store::load → fetch_models → per recorded agent apply() → Store::save
 └─ uninstall_cmd: Store::load → menu::confirm → per agent unconfigure() → Store::save
```

**The central pattern — document mutation, never ownership**: writers never own whole files. They load-or-create (`json_doc::load_or_new`, `json_doc::load_or_new_jsonc` for Pi, `toml_edit::DocumentMut`, `yaml_doc::from_yaml`), mutate only their `Floway` subtree (const `UPSTREAM: &str = "Floway"` in `harness.rs`) or managed keys, and save atomically. `unconfigure` removes exactly that subtree. `gateway::Client` and `ModelList` are passed by reference from the command layer into every agent writer; `state::Store` is the only persistent state.

## Key Directories

- `src/` — all Rust code (flat modules; see Important Files).
- `src/agents/` — per-agent configurators: `mod.rs` (registry/dispatch), `claude.rs`, `codex.rs`, `harness.rs` (omp/pi/opencode/zed/vscode/dsh writers).
- `tests/` — **fixtures only** (`fixtures/fake_gateway.py`); there are no Rust integration tests.
- `.github/workflows/` — `release.yaml`, the only CI.

There is no `docs/`, `scripts/`, Makefile, or justfile.

## Development Commands

```bash
cargo build                    # debug build (bin: floway)
cargo build --release          # release (strip + LTO)
cargo run -- install           # run the CLI
cargo test                     # inline unit tests
cargo check --locked           # fast check; Cargo.lock is committed
```

CI's exact build (the only enforced gate): `cargo build --release --locked --target <triple>`.

Release: push a `v*` tag → `.github/workflows/release.yaml` builds musl-static Linux (x86_64/aarch64), macOS (x86_64/aarch64), and Windows (msvc) tarballs with sha256 sidecars and publishes a GitHub Release. Cross-building `aarch64-unknown-linux-musl` locally needs the linker env vars copied from that workflow.

Manual smoke test against a fake gateway:

```bash
python3 tests/fixtures/fake_gateway.py 18099 &   # bearer token: fw-test-key-1234
printf 'http://127.0.0.1:18099\nfw-test-key-1234\n' | FLOWAY_AGENTS=all cargo run -- install
```

Set `FLOWAY_CLI_CONFIG_DIR` to a temp dir to keep your real state out of smoke tests.

## Code Conventions & Common Patterns

- **Errors**: `anyhow` everywhere — `anyhow::Result`, `bail!`, `.context()` for path-annotated IO. No custom error types. Top level prints `error: {error:#}` (full cause chain). Per-agent loops isolate failures (`any_failed` flag, continue, final `bail!`). `unwrap`/`expect` only where infallible by construction; `Store::load().unwrap_or_default()` in `install.rs` is intentional (corrupt state must not block install).
- **Secrets/files**: write secrets with `crate::write_private_file(path, body)` (stage + rename, 0600). Note three near-duplicate stage-rename implementations exist (`main.rs::write_private_file`, `state.rs::write_private`, `json_doc::save`); reuse `write_private_file` or the doc helpers for new code rather than adding a fourth.
- **Config paths**: every path resolver honors an env override before its HOME-relative default — `FLOWAY_CLI_CONFIG_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR` (oh-my-pi and Pi) and `PI_CONFIG_DIR` (oh-my-pi only), `OPENCODE_CONFIG_DIR`, `ZED_CONFIG_DIR`, `VSCODE_CONFIG_DIR`, `DSH_CONFIG_DIR` (and `DSH_HOME`). Overrides expand a leading `~` where the agent does (`expand_tilde`). Follow this pattern for any new config location; tests rely on it.
- **Selection/input precedence**: credentials — `--endpoint/--api-key` flags > `SETUP_ENDPOINT`/`SETUP_API_KEY` env > saved state > prompt. Agents — `--agents` > `FLOWAY_AGENTS` > interactive menu. Package manager — `FLOWAY_PACKAGE_MANAGER`/`FLOWAY_PM` env > binary inspection in PATH > ambient env (`npm_config_user_agent`, `PNPM_HOME`, `BUN_INSTALL`) > PATH traversal (`pnpm` > `bun` > `yarn` > `npm`) > fallback `npm`.
- **Naming**: snake_case fns, CamelCase types; agent ids are kebab-case (`AgentKind` is `#[serde(rename_all = "kebab-case")]`) and are persisted in `state.json` and accepted by `--agents`/`FLOWAY_AGENTS`.
- **Non-tty behavior**: `menu::confirm` returns its default silently when stdin is not a terminal; `menu::select_agents` falls back to parsing `FLOWAY_AGENTS`. Keep non-interactive paths working.
- **Sync only**: use `reqwest::blocking`; do not introduce async runtimes.
- **Style**: standard rustfmt defaults (no `rustfmt.toml`/`clippy.toml`); section banners are `// ---`; `//!`/`///` doc comments document usage invariants — keep them accurate.

Known rough edges (don't propagate; fix opportunistically only if asked): `FLOWAY_AGENTS` parsing is duplicated (`install.rs::parse_agent_list` vs `menu.rs` non-tty branch); `install::Options` fields are re-declared inline in `Command::Install` instead of flattened; `toml` and `nix` are declared in Cargo.toml but unused in src.

## Important Files

| Path | Role |
|---|---|
| `src/main.rs` | clap CLI, `install`/`update`/`uninstall` dispatch, crate-root `write_private_file` |
| `src/install.rs` | `floway install` orchestration, credential/agent resolution, `parse_agent_list` |
| `src/state.rs` | `Store`/`State`/`Credentials`; `${FLOWAY_CLI_CONFIG_DIR:-$XDG_CONFIG_HOME}/floway-cli/state.json` (0600) |
| `src/gateway.rs` | blocking `GET {endpoint}/v1/models` client + `ModelList`/`Model`/`Limits`/`Pricing` schema; rates are decimal strings scaled 1e6 via `Rates::scaleb6` |
| `src/agents/mod.rs` | `AgentKind` enum (8 variants), `ALL_AGENTS`, apply/unconfigure dispatch, `agent_self_update_commands` |
| `src/agents/claude.rs` | `~/.claude/settings.json` env merge (`MANAGED_ENV_KEYS`); model-list-agnostic |
| `src/agents/codex.rs` | `~/.codex/config.toml` via `toml_edit::DocumentMut` + `floway-token` file; unparseable TOML is left untouched |
| `src/agents/harness.rs` | omp/pi/opencode/zed/vscode/dsh writers; each owns only its provider subtree |
| `src/pm.rs` | Node.js/Bun package manager detection (`pnpm`, `bun`, `yarn`, `npm`) for agent self-update commands |
| `src/json_doc.rs` | canonical JSON read-modify-write: `load_or_new` (rejects corrupt JSON), `load_or_new_jsonc` + `strip_jsonc` (BOM/comments/trailing commas, for Pi's `models.json`), `ensure_object*`, `save` |
| `src/self_update.rs` | binary self-update: GitHub release resolution, checksum verification, atomic swap |
| `src/yaml_doc.rs` | minimal hand-rolled YAML emitter + `serde_yaml` parse; used for oh-my-pi `models.yml` and DeepSeek Harness configs |
| `src/toml_doc.rs` | stage+rename save for `toml_edit` docs (no 0600 — the token file carries the secret) |
| `src/menu.rs` / `src/ui.rs` | crossterm checkbox menu, y/n confirm; colors/prompts, termios echo-off (`unsafe` libc — the only `unsafe` in the crate) |
| `Cargo.toml` / `Cargo.lock` | binary-only manifest; committed lockfile |
| `install.sh` | POSIX installer (curl\|sh); depends on the CLI flags `--endpoint`, `--api-key`, `--non-interactive` — keep them stable |
| `.github/workflows/release.yaml` | tag-triggered release matrix; the only CI |
| `tests/fixtures/fake_gateway.py` | manual smoke-test gateway (port 18099, token `fw-test-key-1234`) |

## Runtime/Tooling Preferences

- **Rust stable, edition 2021**; no MSRV pin, no `rust-toolchain` file (CI uses `dtolnay/rust-toolchain@stable`).
- **Cargo** is the only build system; `Cargo.lock` is committed — use `--locked` in CI-like contexts.
- Key deps and their roles: `clap 4` (derive), `anyhow`, `reqwest 0.12` (blocking + rustls, no OpenSSL), `crossterm 0.29`, `serde`/`serde_json` (`preserve_order` — config key order matters in output), `toml_edit 0.23` (comment-preserving TOML), `serde_yaml 0.9` (deprecated upstream; kept deliberately and only for the oh-my-pi writer), `libc` (termios). `toml` and `nix` are declared but currently unused.
- **No dev-dependencies** — deliberate; don't add test crates casually.
- Windows builds in CI but Unix-only paths exist (0600 perms, termios); guard new platform-specific code with `cfg`.

## Testing & QA

- **Suite**: `cargo test` runs the inline `#[cfg(test)]` unit tests (28 at the time of writing) across `src/`; the two oldest are `state::tests::parses_kebab_case_agent_ids` and `agents::claude::tests::apply_then_unconfigure_round_trip`. Filter with standard libtest syntax (`cargo test agents::claude`).
- **No CI gate for tests, fmt, or clippy** — the only workflow runs on `v*` tags and only builds. Verify locally before pushing.
- **Conventions for new tests**: inline `#[cfg(test)] mod tests` at the bottom of the source file with `use super::*;`; hand-roll temp dirs via `std::env::temp_dir()` + pid-suffixed names and clean up; guard env-var mutation (`HOME`, `*_CONFIG_DIR`) with a `static Mutex` like `claude.rs`'s `ENV_LOCK`; no dev-dependencies.
- **Coverage is minimal** — gateway HTTP, most agent writers, and the menu/ui are untested. Prefer round-trip tests (apply → inject foreign keys → unconfigure → assert foreign keys survive) mirroring the claude test; that asserts the core merge/unmerge contract.
- **Smoke testing** end-to-end: `tests/fixtures/fake_gateway.py` (see Development Commands). It mirrors the real `GET /v1/models` payload shapes (chat with modalities/reasoning/pricing, plain chat, embedding) and enforces bearer auth — mirror its shapes rather than inventing new fixtures.
