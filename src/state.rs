//! Persisted floway-cli state: which agents were configured, and with which
//! gateway credentials. Lives at `${FLOWAY_CLI_CONFIG_DIR:-$XDG_CONFIG_HOME/
//! floway-cli}/state.json` (mode 0600) so `update` and `uninstall` can find
//! every touched agent without re-scanning.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agents::AgentKind;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub endpoint: String,
    pub api_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,
    /// Agent ids previously configured, in first-install order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    agents: Vec<AgentKind>,
}

pub struct Store {
    state: State,
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            state: State::default(),
            path: state_path(),
        }
    }
}

impl Store {
    pub fn load() -> Result<Store> {
        let path = state_path();
        if !path.exists() {
            return Ok(Store {
                state: State::default(),
                path,
            });
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let state: State = serde_json::from_str(&raw)
            .with_context(|| format!("{} is not valid floway state", path.display()))?;
        Ok(Store { state, path })
    }

    pub fn save(&self) -> Result<()> {
        let body = serde_json::to_string_pretty(&self.state)?;
        // Mode 0600: the state carries the API key.
        crate::fs_util::write_atomic(&self.path, body.as_bytes(), 0o600)
    }

    pub fn set_credentials_to_none(&mut self) {
        self.state.credentials = None;
    }

    pub fn credentials(&self) -> Option<&Credentials> {
        self.state.credentials.as_ref()
    }

    pub fn set_credentials(&mut self, credentials: Credentials) {
        self.state.credentials = Some(credentials);
    }

    pub fn installed_agents(&self) -> Vec<AgentKind> {
        self.state.agents.clone()
    }

    pub fn add_agent(&mut self, agent: AgentKind) {
        if !self.state.agents.contains(&agent) {
            self.state.agents.push(agent);
        }
    }

    pub fn remove_agent(&mut self, agent: &AgentKind) {
        self.state.agents.retain(|a| a != agent);
    }
}

fn state_path() -> PathBuf {
    if let Ok(dir) = std::env::var("FLOWAY_CLI_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("state.json");
        }
    }
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => PathBuf::from(home).join(".config"),
            _ => PathBuf::from("."),
        },
    };
    base.join("floway-cli").join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kebab_case_agent_ids() {
        let raw = r#"{"credentials":{"endpoint":"http://e","api_key":"k"},"agents":["claude-code","codex","omp","opencode","zed","vscode"]}"#;
        let state: State = serde_json::from_str(raw).unwrap();
        assert_eq!(state.agents.len(), 6);
        assert_eq!(state.agents[0], crate::agents::AgentKind::ClaudeCode);
    }
}
