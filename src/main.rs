//! floway-cli — set up agentic harnesses for the Floway API router.

mod agents;
mod fs_util;
mod gateway;
mod install;
mod json_doc;
mod menu;
mod pm;
mod state;
mod toml_doc;
mod ui;
mod yaml_doc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

/// Set up agentic harnesses for the Floway API router.
#[derive(Parser)]
#[command(name = "floway", version, about, arg_required_else_help = false)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactively choose which agentic framework to install and configure.
    Install {
        /// Floway gateway origin; skips the endpoint prompt.
        #[arg(long, value_name = "URL")]
        endpoint: Option<String>,
        /// Floway API key; skips the key prompt.
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        /// Select agents without the menu: a comma list of ids
        /// (claude-code,codex,oh-my-pi,opencode,zed,vscode,deepseek-harness) or `all`.
        #[arg(long, value_name = "LIST")]
        agents: Option<String>,
        /// Fail instead of prompting when information is missing; also implied
        /// by every flag being present.
        #[arg(long)]
        non_interactive: bool,
    },
    /// Re-fetch the model list and re-apply configuration for installed agents.
    Update,
    /// Remove Floway configuration from every previously-configured agent.
    Uninstall,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Install {
            endpoint,
            api_key,
            agents,
            non_interactive,
        }) => {
            install::run(install::Options {
                endpoint,
                api_key,
                agents,
                non_interactive,
            })?;
            Ok(())
        }
        Some(Command::Update) => {
            update_cmd()?;
            Ok(())
        }
        Some(Command::Uninstall) => {
            uninstall_cmd()?;
            Ok(())
        }
        // No subcommand: default to the interactive install menu.
        None => {
            install::run(install::Options::default())?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// install

/// Write a file with mode 0600 via a same-directory stage + rename.
pub fn write_private_file(path: &std::path::Path, body: &str) -> Result<()> {
    fs_util::write_atomic(path, body.as_bytes(), 0o600)
}

// update

fn update_cmd() -> Result<()> {
    let store = state::Store::load()?;
    let creds = match store.credentials() {
        Some(creds) => creds.clone(),
        None => bail!("no floway state found; run `floway install` first"),
    };

    let installed: Vec<agents::AgentKind> = store.installed_agents().to_vec();
    if installed.is_empty() {
        bail!("no previously-installed agents recorded; run `floway install` first");
    }

    println!("Updating agent configuration …");
    let client = gateway::Client::new(creds.endpoint.clone(), creds.api_key.clone())?;
    let models = client
        .fetch_models()
        .context("could not reach the Floway gateway; update aborted")?;

    let mut any_failed = false;
    for agent in &installed {
        print!("{:>12}  ", agent.label());
        ui::flush();
        match agent.apply(&client, &models) {
            Ok(summary) => println!("{}", ui::green(&format!("updated — {summary}"))),
            Err(error) => {
                any_failed = true;
                println!("{}", ui::red("failed"));
                eprintln!("  {error:#}");
            }
        }
    }

    // Self-update hint: the CLI updates agents, not itself; keep `update`'s
    // contract about "their program themselves" honest by checking the binary
    // directories we know we can refresh.
    if let Some(updatable) = agents::agent_self_update_commands(&installed) {
        println!();
        println!(
            "{}",
            ui::dim("To update the agent programs themselves, re-run floway's installer or their own update commands:")
        );
        for line in updatable {
            println!("  {}", ui::dim(&line));
        }
    }

    store.save()?;
    if any_failed {
        bail!("one or more agents failed to update; see the output above");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// uninstall

fn uninstall_cmd() -> Result<()> {
    let mut store = state::Store::load()?;
    let installed = store.installed_agents();
    if installed.is_empty() {
        println!("No floway-configured agents found; nothing to uninstall.");
        return Ok(());
    }
    println!("Previously-installed agents:");
    for agent in &installed {
        println!("  - {}", agent.label());
    }
    if !menu::confirm(
        "Remove the Floway configuration written above from all of them?",
        true,
    )? {
        println!("Aborted; nothing was changed.");
        return Ok(());
    }

    let mut any_failed = false;
    for agent in installed {
        print!("{:>12}  ", agent.label());
        ui::flush();
        match agent.unconfigure() {
            Ok(Some(summary)) => println!("{}", ui::green(&format!("removed — {summary}"))),
            Ok(None) => println!("{}", ui::dim("nothing to remove")),
            Err(error) => {
                any_failed = true;
                println!("{}", ui::red("failed"));
                eprintln!("  {error:#}");
            }
        }
        store.remove_agent(&agent);
    }

    store.set_credentials_to_none();
    store
        .save()
        .context("could not persist floway state after uninstalling")?;

    if any_failed {
        bail!("one or more agents failed to unconfigure; see the output above");
    }
    println!("All Floway configuration removed.");
    Ok(())
}

// ---------------------------------------------------------------------------
