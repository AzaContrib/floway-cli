//! Agentic framework integrations. Every agent mirrors one of the six Floway
//! Agent Setup harnesses (`claude | codex | omp | vscode | zed | opencode`)
//! but re-implements the writes natively in Rust so the same code path can
//! both configure and *un*configure.

mod claude;
mod codex;
mod harness;

use anyhow::Result;

use crate::gateway::{self, ModelList};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Omp,
    Opencode,
    Zed,
    Vscode,
}

pub const ALL_AGENTS: [AgentKind; 6] = [
    AgentKind::ClaudeCode,
    AgentKind::Codex,
    AgentKind::Omp,
    AgentKind::Opencode,
    AgentKind::Zed,
    AgentKind::Vscode,
];

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Omp => "oh-my-pi",
            AgentKind::Opencode => "opencode",
            AgentKind::Zed => "Zed",
            AgentKind::Vscode => "VSCode",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Omp => "oh-my-pi",
            AgentKind::Opencode => "opencode",
            AgentKind::Zed => "zed",
            AgentKind::Vscode => "vscode",
        }
    }

    /// Human-readable list of the files this agent's Floway config lives in.
    pub fn config_paths(self) -> Vec<String> {
        match self {
            AgentKind::ClaudeCode => vec![claude::settings_path().display().to_string()],
            AgentKind::Codex => vec![
                codex::config_path().display().to_string(),
                codex::token_path().display().to_string(),
            ],
            AgentKind::Omp => vec![
                harness::omp_paths().0.display().to_string(),
                harness::omp_paths().1.display().to_string(),
            ],
            AgentKind::Opencode => vec![harness::opencode_path().display().to_string()],
            AgentKind::Zed => vec![harness::zed_path().display().to_string()],
            AgentKind::Vscode => vec![harness::vscode_path().display().to_string()],
        }
    }

    /// Fetch + convert + write. Returns a one-line summary of what was written.
    pub fn apply(self, client: &gateway::Client, models: &ModelList) -> Result<String> {
        match self {
            AgentKind::ClaudeCode => claude::apply(client, models),
            AgentKind::Codex => codex::apply(client, models),
            AgentKind::Omp => harness::apply_omp(client, models),
            AgentKind::Opencode => harness::apply_opencode(client, models),
            AgentKind::Zed => harness::apply_zed(client, models),
            AgentKind::Vscode => harness::apply_vscode(client, models),
        }
    }

    /// Remove the Floway configuration. `Ok(None)` means nothing was present.
    pub fn unconfigure(self) -> Result<Option<String>> {
        match self {
            AgentKind::ClaudeCode => claude::unconfigure(),
            AgentKind::Codex => codex::unconfigure(),
            AgentKind::Omp => harness::unconfigure_omp(),
            AgentKind::Opencode => harness::unconfigure_opencode(),
            AgentKind::Zed => harness::unconfigure_zed(),
            AgentKind::Vscode => harness::unconfigure_vscode(),
        }
    }
}

/// The program-update half of `floway update`: per-agent commands that refresh
/// the agent binaries themselves (floway-cli never runs package managers
/// unprompted; it only reports what the user can run).
pub fn agent_self_update_commands(agents: &[AgentKind]) -> Option<Vec<String>> {
    if agents.is_empty() {
        return None;
    }
    let lines = agents
        .iter()
        .map(|agent| match agent {
            AgentKind::ClaudeCode => {
                "Claude Code: `claude update` (or reinstall via npm/brew)".to_string()
            }
            AgentKind::Codex => "Codex: `npm install --global @openai/codex@latest`".to_string(),
            AgentKind::Omp => "oh-my-pi: reinstall/upgrade via its usual channel".to_string(),
            AgentKind::Opencode => "opencode: `opencode upgrade`".to_string(),
            AgentKind::Zed => "Zed: in-app updater or your package manager".to_string(),
            AgentKind::Vscode => "VSCode: in-app updater or your package manager".to_string(),
        })
        .collect();
    Some(lines)
}
