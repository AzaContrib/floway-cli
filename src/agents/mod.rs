//! Agentic framework integrations. Every agent mirrors one of the
//! supported agentic harnesses (`claude | codex | omp | vscode | zed | opencode | dsh`)
//! and re-implements the writes natively in Rust so the same code path can
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
    #[serde(alias = "oh-my-pi")]
    Omp,
    Opencode,
    Zed,
    Vscode,
    #[serde(rename = "deepseek-harness", alias = "dsh")]
    DeepSeekHarness,
}

pub const ALL_AGENTS: [AgentKind; 7] = [
    AgentKind::ClaudeCode,
    AgentKind::Codex,
    AgentKind::Omp,
    AgentKind::Opencode,
    AgentKind::Zed,
    AgentKind::Vscode,
    AgentKind::DeepSeekHarness,
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
            AgentKind::DeepSeekHarness => "DeepSeek Harness",
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
            AgentKind::DeepSeekHarness => "deepseek-harness",
        }
    }

    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            AgentKind::Omp => &["omp"],
            AgentKind::DeepSeekHarness => &["dsh"],
            _ => &[],
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
            AgentKind::DeepSeekHarness => harness::apply_dsh(client, models),
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
            AgentKind::DeepSeekHarness => harness::unconfigure_dsh(),
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
                let pm = crate::pm::PackageManager::detect_for_binary(Some("claude"));
                format!(
                    "Claude Code: `claude update` (or reinstall via {}/brew)",
                    pm.name()
                )
            }
            AgentKind::Codex => {
                let pm = crate::pm::PackageManager::detect_for_binary(Some("codex"));
                format!(
                    "Codex: `{}`",
                    pm.global_install_command("@openai/codex@latest")
                )
            }
            AgentKind::Omp => "oh-my-pi: reinstall/upgrade via its usual channel".to_string(),
            AgentKind::Opencode => "opencode: `opencode upgrade`".to_string(),
            AgentKind::Zed => "Zed: in-app updater or your package manager".to_string(),
            AgentKind::Vscode => "VSCode: in-app updater or your package manager".to_string(),
            AgentKind::DeepSeekHarness => {
                let pm = crate::pm::PackageManager::detect_for_binary(Some("dsh"));
                format!(
                    "DeepSeek Harness: `{}`",
                    pm.global_install_command("@deepseek-ai/dsh@latest")
                )
            }
        })
        .collect();
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn self_update_commands_respects_package_manager_env() {
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "bun");
        let cmds = agent_self_update_commands(&[
            AgentKind::ClaudeCode,
            AgentKind::Codex,
            AgentKind::DeepSeekHarness,
        ])
        .unwrap();

        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0],
            "Claude Code: `claude update` (or reinstall via bun/brew)"
        );
        assert_eq!(
            cmds[1],
            "Codex: `bun add --global @openai/codex@latest`"
        );
        assert_eq!(
            cmds[2],
            "DeepSeek Harness: `bun add --global @deepseek-ai/dsh@latest`"
        );

        std::env::set_var("FLOWAY_PACKAGE_MANAGER", "pnpm");
        let cmds = agent_self_update_commands(&[
            AgentKind::Codex,
            AgentKind::DeepSeekHarness,
        ])
        .unwrap();

        assert_eq!(
            cmds[0],
            "Codex: `pnpm add --global @openai/codex@latest`"
        );
        assert_eq!(
            cmds[1],
            "DeepSeek Harness: `pnpm add --global @deepseek-ai/dsh@latest`"
        );

        std::env::remove_var("FLOWAY_PACKAGE_MANAGER");
    }
}
