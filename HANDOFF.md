# Implementation handoff: floway-cli review revisions

Audience: implementation agent. Source: code review of the initial floway-cli
codebase (2026-09-09). Every item below was confirmed by reading the code or
by running it; file/line references are against the current tree.

floway-cli is a Rust CLI (`cargo build`, binary `floway`) that configures six
coding agents (Claude Code, Codex, oh-my-pi, opencode, Zed, VSCode) against a
Floway API gateway. Commands: `install`, `update`, `uninstall`. Layout:

- `src/main.rs` — CLI, `update`/`uninstall` commands, `write_private_file`
- `src/install.rs` — install flow (credential/agent resolution, prompts)
- `src/state.rs` — `~/.config/floway-cli/state.json` persistence
- `src/gateway.rs` — `/v1/models` client + payload types
- `src/agents/{mod,claude,codex,harness}.rs` — per-agent config writers
- `src/{json,toml,yaml}_doc.rs` — document load/save helpers
- `src/{menu,ui}.rs` — interactive menu, prompts, ANSI helpers
- `install.sh` — curl-to-sh installer
- `.github/workflows/release.yaml` — release pipeline
- `tests/fixtures/fake_gateway.py` — fake gateway for smoke tests

Invariants to preserve: all writes transactional (same-directory stage +
rename); anything carrying the API key is mode 0600; unrelated user config,
comments (Codex TOML), and formatting survive both apply and unconfigure.

## Verification harness (use after every change)

```bash
cargo test --locked
cargo build

# Smoke test against the fixture gateway:
python3 tests/fixtures/fake_gateway.py 18099 &
export FLOWAY_CLI_CONFIG_DIR=/tmp/fw HOME=/tmp/fw CLAUDE_CONFIG_DIR=/tmp/fw/claude \
       CODEX_HOME=/tmp/fw/codex OMP_CONFIG_DIR=/tmp/fw/omp \
       OPENCODE_CONFIG_DIR=/tmp/fw/opencode ZED_CONFIG_DIR=/tmp/fw/zed \
       VSCODE_CONFIG_DIR=/tmp/fw/vscode
printf 'http://127.0.0.1:18099\nfw-test-key-1234\n' | FLOWAY_AGENTS=all ./target/debug/floway install
./target/debug/floway update
./target/debug/floway uninstall   # piped stdin → auto-confirms
```

The fixture serves three chat models: `gpt-5.6` (reasoning, image input,
pricing), `claude-opus-4-6` (no reasoning block, pricing absent),
`deepseek-v4-flash` (reasoning, no modalities block), plus one embedding model
that must be filtered out everywhere.

---

## Bugs (fix first)

### 1. omp writer hardcodes `reasoning: true` for every model

`src/agents/harness.rs`, `omp_model_config` (~line 172):
`config.insert("reasoning", json!(true))` fires unconditionally.
`opencode_model_config` in the same file correctly gates on
`model.chat.reasoning.is_some()`. With the fixture, `claude-opus-4-6` has no
`chat.reasoning` block but still gets `reasoning: true` in `models.yml`.

**Change:** emit `reasoning: true` only when `model.chat.reasoning.is_some()`.
**Acceptance:** after install, `grep -c 'reasoning: true' $OMP_CONFIG_DIR/models.yml`
returns 2 (fixture has two reasoning models), and PyYAML parses the file.

### 2. Codex unconfigure deletes user config and silently discards mutations

`src/agents/codex.rs`, `unconfigure` (~lines 100-125). Two defects:

a. `root.remove("model_providers")` and `root.remove("features")` remove the
   *entire* tables. A user with another provider under `[model_providers]` or
   other `[features]` flags loses them — violates the preserve-unrelated-keys
   invariant.
b. The mutated document is only saved when `had_marker`
   (`model_provider == "floway"`). If the user switched `model_provider` to
   something else while leaving `[model_providers.floway]` in place, the
   removal is computed then dropped: floway keys and the token survive, but
   the CLI reports success and state records the agent as removed.

**Change:** remove exactly the keys `apply` writes: `model_provider`,
`suppress_unstable_features_warning`, `model_providers.floway`,
`features.apps`, `features.standalone_web_search`. Prune `model_providers` /
`features` only when emptied by the removal. Save whenever any removal
happened (not gated on the marker). Keep the existing behavior of deleting
`config.toml` when the result is empty, and keep skipping unparseable TOML.
Still always remove `floway-token`.
**Acceptance:** install Codex, hand-edit `config.toml` to add
`[model_providers.other]` with a key and a `[features]` flag, plus a comment;
uninstall; the other provider, feature flag, and comment survive; the
`floway` table, the five managed keys, and `floway-token` are gone. Then
reinstall, set `model_provider = "openai"` by hand, uninstall: floway subtree
and token still removed. Add both as `#[cfg(test)]` tests in `codex.rs`
(follow the env-lock pattern in `claude.rs`).

### 3. Codex `auth` embeds an absolute token path

`src/agents/codex.rs`, `apply` (~line 75):
`floway["auth"] = format!("cat \"{}\"", token_path().display())` bakes a
machine-specific absolute path (including the username) into `config.toml` —
breaks synced dotfiles, relocatable `CODEX_HOME`, and leaks the username into
a commonly committed file.

**Change:** check Codex's config docs (linked at the top of `codex.rs`) for
`env_key`-style provider auth; if supported, write the key via an env-var
reference and document where the user sets it, keeping the 0600 token file
only if still needed. If Codex genuinely requires a command, emit a
`$HOME`-relative form (e.g. `cat "$HOME/.codex/floway-token"`) instead of the
expanded path. Pick one; do not ship both.
**Acceptance:** written `config.toml` contains no absolute path; `codex` auth
still resolves the key (verify by inspection against the documented mechanism).

### 4. install.sh version resolution passes curl flags as positional args

`install.sh` (~line 74):
`fetch_stdout "$LATEST_URL" -I -o /dev/null -w '%{url_effective}'` — the
`-I -o -w` flags land after the URL as extra positional args to the
`fetch_stdout` wrapper (`curl -fsSL "$1"`), so `$VERSION` captures response
body garbage and the `v[0-9]*` check silently falls through to the API
fallback. The primary path is dead code.

**Change:** resolve the tag with a direct call:
`VERSION="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$LATEST_URL" | sed 's#.*/tag/##')"`
(and a wget equivalent using `--max-redirect`/`S`-style final-URL reporting, or
drop the redirect probe and keep only the API path — simpler). Keep the
`v[0-9]*` validation and the existing fallback.
**Acceptance:** `sh -x install.sh` against the real repo shows a sane tag on
the primary path; temporarily breaking the API endpoint still resolves via
whichever path remains.

### 5. Atomic-write helpers: four copies, one with a 0600 race

Copies: `write_private_file` (`src/main.rs` ~line 93), `state.rs::write_private`
(~line 113), `json_doc::save` (~line 51), `toml_doc::save`.

`state.rs::write_private` uses `File::create` + `set_permissions` — the state
file (contains the API key) exists with umask-default permissions (typically
0644) between create and chmod. `toml_doc::save` uses `std::fs::write` with no
mode handling at all.

**Change:** consolidate on one helper (the `OpenOptions::mode(0600)` pattern
already in `main.rs`/`json_doc.rs`) living in one module — `json_doc.rs` or a
new `fs_util.rs` — parameterized by mode. Route `state.rs`, `toml_doc.rs`,
`main.rs::write_private_file`, and `json_doc::save` through it. Keep
same-directory staging + rename. Unify stage-file naming while there:
`main.rs`/`state.rs` produce `<name>.floway-stage.<pid>`-style names,
`json_doc.rs` produces `config.floway-stage.<pid>.json` (extension trailing);
pick the extension-preserving form everywhere so a stale stage file is
recognizable to the owning app.
**Acceptance:** single implementation; `grep` shows one atomic-write helper;
`stat -c '%a'` on state.json, models.yml, .env, opencode.json, and the codex
token after install all return 600; `cargo test` passes.

### 6. Misleading dead-code comment on `Effort::default`

`src/gateway.rs` — comment claims the field is "round-tripped from
/v1/models"; nothing re-serializes `Effort`, it is parse-only.

**Change:** drop the field (and the `#[allow(dead_code)]`) or fix the comment.
Dropping is preferred — weightless code.

---

## Smaller revisions

7. **`update_cmd` dead drift comment** — `src/main.rs` (~line 130): comment
   promises "Also offer agents whose config we found but never recorded"; no
   such detection exists. Delete the comment, or implement detection by
   probing each agent's `config_paths()` for the Floway marker. Deleting is
   acceptable; do not leave the stale comment.

8. **`let _ = written;` noise** — `src/main.rs` `update_cmd`: the
   `config_paths()` result is computed and discarded. Either include the paths
   in the per-agent summary line or remove the call.

9. **`resolve_agents` double env read + empty-string fallthrough** —
   `src/install.rs`: `std::env::var("FLOWAY_AGENTS")` is checked with
   `is_ok()` then read again; collapse to one read. `FLOWAY_AGENTS=""` in a
   TTY currently falls through to the menu; make an explicitly-set-but-empty
   env var an error in non-interactive contexts (the menu's non-tty branch
   already bails — align the tty branch).

10. **Menu redraw fragility in raw mode** — `src/menu.rs` `menu_loop` uses
    `println!` while raw mode is active. Works on unix (stdout stays cooked)
    but is fragile. Switch the redraw to `crossterm::execute!` with explicit
    `\r\n` line endings.

11. **`secret_prompt` echo fallback** — `src/ui.rs`: when `read_masked`
    returns empty the code can't distinguish "termios unavailable, echo still
    on" from "user pressed Enter". For the no-saved-key path, an
    empty-but-echoing fallback reads the next line with echo on. Make
    `read_masked` return an enum/Option that separates the two cases; only
    fall back to `read_line` when termios was unavailable, and note the echo
    in the prompt in that case.

12. **Unused `nix` dependency** — `Cargo.toml` declares
    `nix = { version = "0.30", features = ["user"] }`; nothing references it
    (confirmed by grep). Remove the dependency and the stale "yaml is only
    needed…" comment above it (that comment belongs to no dependency — the
    yaml writer is hand-rolled in `yaml_doc.rs`). Run `cargo build --locked`
    and commit the updated `Cargo.lock`.

13. **Release workflow** — `.github/workflows/release.yaml`:
    a. The `Stage artifact` step has two identical `if Darwin / else` tar
       branches (~line 90) — collapse to one.
    b. `cp README.md LICENSE "$STAGE"/ 2>/dev/null || cp README.md "$STAGE"/`
       silently masks the missing LICENSE. The repo declares `license = "MIT"`
       but ships no LICENSE file. Add a LICENSE file at the repo root (MIT,
       matching Cargo.toml) and make the copy fail loudly if it's missing.
    c. Cosmetic: the workflow header comment says binaries land as
       `floway-<target>` plus `.sha256` sidecars, but the sidecar is computed
       into the staging dir and only `dist/*.tar.gz` is uploaded — the
       per-artifact `.sha256` install.sh looks for
       (`$URL.sha256`) is never published. Upload the sidecars too
       (`files: dist/*`), or install.sh's checksum step is dead code.

14. **install.sh version pinning** — no way to install a specific version.
    Add `FLOWAY_CLI_VERSION` (and/or `--version`) overriding the
    latest-release resolution; validate it matches `v[0-9]*`.

---

## Definition of done

- Items 1-6 fixed; 7-14 fixed or explicitly deferred with a stated reason.
- `cargo test --locked` green, including the two new Codex tests (item 2).
- Fixture smoke test (commands above): install writes all six agents'
  configs; `models.yml` shows `reasoning: true` exactly twice; update
  succeeds; uninstall removes exactly the Floway-managed keys and leaves any
  hand-planted foreign keys/comments intact (plant them before uninstall).
- `stat` confirms 0600 on every key-carrying file.
- `cargo clippy --locked` clean (no new warnings).
- No new dependencies; no changes to the agent config shapes beyond what
  items 1 and 3 require.
