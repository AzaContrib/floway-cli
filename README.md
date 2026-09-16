# floway-cli

Set up agentic harnesses for the [Floway](https://github.com/Menci/Floway) API
router. One command configures every supported coding agent against your
Floway gateway; another removes every trace.

Written in Rust. Ships as a single static binary (`floway`).

## Install

Build from source:

```bash
cargo build --release
install -Dm755 target/release/floway ~/.local/bin/floway
```

## Commands

### `floway install`

Interactive menu:

1. Prompts for the gateway endpoint (default `http://localhost:18088`) and a
   Floway API key (echo disabled). If you installed before, the saved
   endpoint/key is offered for reuse.
2. Verifies both against `GET /v1/models` before touching any agent.
3. Shows a checkbox menu (↑/↓ or j/k, space to toggle, `a` to toggle all,
   enter to confirm, esc to cancel) of every agent Floway supports.
4. Configures each selected agent — installing the CLI if missing (Claude
   Code, Codex), fetching the live model list, and writing provider settings.

Non-interactive use: pipe the endpoint/key and set `FLOWAY_AGENTS`:

```bash
printf 'https://gw.example\nfw-...-key\n' | FLOWAY_AGENTS=all floway install
FLOWAY_AGENTS=claude-code,codex floway install   # subset; ids: claude-code codex oh-my-pi opencode zed vscode deepseek-harness
```

### `floway update`

Re-fetches the model list from the gateway and re-applies configuration for
every previously-installed agent — picking up new/renamed/removed models.
Prints each agent's own update command (automatically detecting whether
`bun`, `pnpm`, `yarn`, or `npm` was used or is available on PATH; override via
`FLOWAY_PACKAGE_MANAGER`) for updating the agent programs themselves.

Pass `--self` to update the `floway` binary itself instead of agent configs.

### `floway self-update`

Updates the `floway` binary by downloading the official release for the
current platform from GitHub Releases, verifying its SHA-256 checksum, and
atomically replacing the running executable:

```bash
floway self-update          # update to the latest release
floway self-update --check  # check if a newer release is available
floway self-update --version v0.2.0  # install a specific version
floway self-update --force  # reinstall even if already on the latest version
```

### `floway uninstall`

Lists previously-installed agents and removes exactly the Floway-managed
configuration from each: managed env keys, the `Floway` provider subtree,
provider tokens, and the recorded key. Unrelated settings survive.

## What gets written

| Agent | File(s) |
|---|---|
| Claude Code | `~/.claude/settings.json` — `.env.ANTHROPIC_BASE_URL`, `.env.ANTHROPIC_AUTH_TOKEN`, gateway model discovery |
| Codex | `~/.codex/config.toml` — `model_providers.floway` (responses wire API, websockets, command auth) + `~/.codex/floway-token` |
| oh-my-pi | `~/.omp/agent/models.yml` (full model catalog with limits/costs) + the key in `~/.omp/agent/.env` |
| opencode | `~/.config/opencode/opencode.json` — `provider.Floway` with per-model limits, reasoning variants, costs |
| Zed | `~/.config/zed/global_settings.json` — `language_models.openai_compatible.Floway` |
| VSCode | user-profile `chatLanguageModels.json` — a `Floway` custom-endpoint model group |
| DeepSeek Harness | `~/.dsh/settings.yaml` — `llm-pi-ai.providers.floway` (responses API, reasoning efforts, vision) + key in `~/.dsh/.credentials.yaml` (0600) |

All writes are transactional (same-directory stage + rename), owner-only
(0600 for anything carrying the API key), and preserve unrelated keys,
comments (Codex TOML), and formatting.

## State

`~/.config/floway-cli/state.json` (0600) records the endpoint, API key, and
configured agents. `FLOWAY_CLI_CONFIG_DIR` relocates it.

## License

MIT
