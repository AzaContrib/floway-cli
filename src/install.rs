//! The `floway install` flow: resolve credentials (flags → env → saved state
//! → prompts), select agents (flags → env → menu), verify against the
//! gateway, and configure each selected agent.

use anyhow::{bail, Context, Result};

use crate::agents::AgentKind;
use crate::gateway;
use crate::menu;
use crate::state;
use crate::ui;

#[derive(Debug, Default, clap::Args)]
pub struct Options {
    /// Floway gateway origin; skips the endpoint prompt.
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,
    /// Floway API key; skips the key prompt.
    #[arg(long, value_name = "KEY")]
    pub api_key: Option<String>,
    /// Select agents without the menu: a comma list of ids
    /// (claude-code,codex,oh-my-pi,opencode,zed,vscode) or `all`.
    #[arg(long, value_name = "LIST")]
    pub agents: Option<String>,
    /// Fail instead of prompting when information is missing.
    #[arg(long)]
    pub non_interactive: bool,
}

pub fn run(options: Options) -> Result<()> {
    let mut store = state::Store::load().unwrap_or_default();
    let non_interactive =
        options.non_interactive || (options.endpoint.is_some() && options.api_key.is_some() && options.agents.is_some());

    // Credential precedence: flags > SETUP_* harness env > saved state > prompt.
    let (endpoint, api_key) = resolve_credentials(&options, non_interactive, &store)?;

    // Agent selection precedence: --agents flag > FLOWAY_AGENTS env > menu.
    let selected = resolve_agents(&options, non_interactive)?;

    // Fail before touching any agent when the credentials or gateway are bad.
    print!("Verifying the endpoint and key … ");
    ui::flush();
    let client = gateway::Client::new(endpoint.clone(), api_key.clone())?;
    let models = client
        .fetch_models()
        .context("could not reach the Floway gateway with this key")?;
    println!(
        "{}",
        ui::green(&format!("ok, {} chat models", models.data.len()))
    );

    if selected.is_empty() {
        println!("Nothing selected; done.");
        return Ok(());
    }

    let mut any_failed = false;
    for agent in &selected {
        println!("{}", ui::bold(&format!("Setting up {}", agent.label())));
        if let Err(error) = agent.apply(&client, &models) {
            any_failed = true;
            eprintln!(
                "{}: configuring {} failed: {error:#}",
                ui::red("error"),
                agent.label()
            );
            continue;
        }
        println!("{}", ui::green(&format!("  configured {}", agent.label())));
        store.add_agent(*agent);
    }

    store.set_credentials(state::Credentials {
        endpoint,
        api_key,
    });
    store
        .save()
        .context("could not persist floway state; agent configuration may not survive")?;

    if any_failed {
        bail!("one or more agents failed to configure; see the output above");
    }
    Ok(())
}

fn resolve_credentials(
    options: &Options,
    non_interactive: bool,
    store: &state::Store,
) -> Result<(String, String)> {
    let flag_endpoint = normalize_endpoint(options.endpoint.as_deref())?;
    let env_endpoint = normalize_endpoint(std::env::var("SETUP_ENDPOINT").ok().as_deref())?;

    let flag_key = options.api_key.clone().filter(|k| !k.is_empty());
    let env_key = std::env::var("SETUP_API_KEY").ok().filter(|k| !k.is_empty());

    let endpoint = match (flag_endpoint, env_endpoint, store.credentials().map(|c| c.endpoint.clone())) {
        (Some(endpoint), _, _) | (_, Some(endpoint), _) | (_, _, Some(endpoint)) => endpoint,
        (None, None, None) => {
            if non_interactive {
                bail!("no endpoint given; pass --endpoint or set SETUP_ENDPOINT");
            }
            let raw = prompt_endpoint(None)?;
            raw
        }
    };

    let api_key = match (flag_key, env_key, store.credentials().map(|c| c.api_key.clone())) {
        (Some(key), _, _) | (_, Some(key), _) | (_, _, Some(key)) => key,
        (None, None, None) => {
            if non_interactive {
                bail!("no API key given; pass --api-key or set SETUP_API_KEY");
            }
            prompt_api_key(&endpoint, None)?
        }
    };

    Ok((endpoint, api_key))
}

fn resolve_agents(options: &Options, _non_interactive: bool) -> Result<Vec<AgentKind>> {
    // Explicit flag wins; else the FLOWAY_AGENTS env the install script and
    // the harness conventions use; else the interactive menu (which itself
    // handles the non-tty FLOWAY_AGENTS path).
    if let Some(list) = &options.agents {
        return parse_agent_list(list);
    }
    if std::env::var("FLOWAY_AGENTS").is_ok() {
        // menu::select_agents reads the env in its non-tty branch; in a TTY the
        // env is still honored so scripts with a TTY behave identically.
        if let Ok(list) = std::env::var("FLOWAY_AGENTS") {
            if !list.trim().is_empty() {
                return parse_agent_list(&list);
            }
        }
    }
    menu::select_agents("Which agentic frameworks should floway set up?", &[])
}

fn parse_agent_list(list: &str) -> Result<Vec<AgentKind>> {
    let list = list.trim();
    if list.is_empty() {
        return Ok(Vec::new());
    }
    if list.eq_ignore_ascii_case("all") {
        return Ok(crate::agents::ALL_AGENTS.to_vec());
    }
    let mut picked = Vec::new();
    for token in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let id = token.replace(' ', "-").to_lowercase();
        let agent = crate::agents::ALL_AGENTS
            .iter()
            .find(|a| a.id() == id)
            .or_else(|| {
                crate::agents::ALL_AGENTS
                    .iter()
                    .find(|a| a.label().to_lowercase().replace(' ', "-") == id)
            });
        let agent = agent.ok_or_else(|| {
            anyhow::anyhow!(
                "unknown agent id {token:?}; valid ids: {}",
                crate::agents::ALL_AGENTS
                    .iter()
                    .map(|a| a.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        if !picked.contains(agent) {
            picked.push(*agent);
        }
    }
    Ok(picked)
}

fn normalize_endpoint(raw: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        bail!("the endpoint must be an http(s) origin, got {trimmed}");
    }
    Ok(Some(trimmed.to_string()))
}

fn prompt_endpoint(saved: Option<&str>) -> Result<String> {
    let default = saved.unwrap_or("http://localhost:18088");
    let raw = ui::prompt("Floway gateway endpoint", default)?;
    normalize_endpoint(Some(&raw)).map(|e| e.unwrap_or_else(|| default.to_string()))
}

fn prompt_api_key(_endpoint: &str, saved: Option<&String>) -> Result<String> {
    let raw = match saved {
        Some(key) => ui::secret_prompt_with_default("Floway API key", key)?,
        None => ui::secret_prompt("Floway API key")?,
    };
    let key = raw.trim().to_string();
    if key.is_empty() {
        bail!(
            "an API key is required (create one in the Floway dashboard under Services → API Keys)"
        );
    }
    Ok(key)
}
