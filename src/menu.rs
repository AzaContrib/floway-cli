//! The interactive agent-selection menu: a crossterm checkbox list with
//! space/enter toggling, plus a simple confirm() for yes/no questions.

use crate::agents::AgentKind;
use crate::ui;
use anyhow::{bail, Result};
use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::IsTerminal;

/// True when stdin is not an interactive terminal (pipes, CI).
pub fn noninteractive() -> bool {
    !std::io::stdin().is_terminal()
}

/// Ask a yes/no question. `default` is returned on a bare Enter.
pub fn confirm(question: &str, default: bool) -> Result<bool> {
    if noninteractive() {
        return Ok(default);
    }
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    print!("{} {} ", question, ui::dim(hint));
    ui::flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    match line.trim().to_lowercase().as_str() {
        "" => Ok(default),
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        other => bail!("please answer y or n, got {other:?}"),
    }
}

/// Checkbox menu over all agents. `preselected` starts checked (e.g. agents
/// already recorded in state).
pub fn select_agents(question: &str, preselected: &[AgentKind]) -> Result<Vec<AgentKind>> {
    if noninteractive() {
        // Non-tty selection: FLOWAY_AGENTS=claude,codex,... or FLOWAY_AGENTS=all.
        let requested = std::env::var("FLOWAY_AGENTS").unwrap_or_default();
        let requested = requested.trim();
        if requested.is_empty() {
            bail!("an interactive terminal is required to choose agents; set FLOWAY_AGENTS=claude-code,codex,oh-my-pi,opencode,zed,vscode (or FLOWAY_AGENTS=all) for non-interactive use");
        }
        let all = crate::agents::ALL_AGENTS;
        let ids: Vec<String> = all.iter().map(|a| a.id().to_string()).collect();
        if requested.eq_ignore_ascii_case("all") {
            return Ok(all.to_vec());
        }
        let mut picked = Vec::new();
        for token in requested
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let id = token.replace(' ', "-").to_lowercase();
            let agent = all.iter().find(|a| a.id() == id).or_else(|| {
                all.iter()
                    .find(|a| a.label().to_lowercase().replace(' ', "-") == id)
            });
            match agent {
                Some(agent) => {
                    if !picked.contains(agent) {
                        picked.push(*agent);
                    }
                }
                None => bail!("unknown agent id {token:?}; valid ids: {}", ids.join(", ")),
            }
        }
        return Ok(picked);
    }

    println!("{}", ui::bold(question));
    let agents: &[AgentKind; 6] = &crate::agents::ALL_AGENTS;
    let mut checked: Vec<bool> = agents
        .iter()
        .map(|agent| preselected.contains(agent))
        .collect();
    let mut cursor = 0usize;

    enable_raw_mode()?;
    let result = menu_loop(agents, &mut checked, &mut cursor);
    disable_raw_mode()?;
    result
}

fn menu_loop(
    agents: &[AgentKind; 6],
    checked: &mut [bool],
    cursor: &mut usize,
) -> Result<Vec<AgentKind>> {
    let mut first_draw = true;
    loop {
        if !first_draw {
            // Move up N+1 lines and clear, redrawing in place.
            crossterm::execute!(
                std::io::stdout(),
                crossterm::cursor::MoveUp(agents.len() as u16 + 1),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
            )?;
        }
        first_draw = false;

        for (index, agent) in agents.iter().enumerate() {
            let marker = if checked[index] { "x" } else { " " };
            let pointer = if index == *cursor { ">" } else { " " };
            let style = |text: &str| {
                if index == *cursor {
                    ui::cyan(text)
                } else {
                    text.to_string()
                }
            };
            println!("{pointer} [{marker}] {}", &style(agent.label()));
        }
        println!(
            "{}",
            ui::dim("  ↑/↓ or j/k to move, space to toggle, a to toggle all, enter to confirm, esc to cancel")
        );

        let event = read()?;
        let KeyEvent {
            code,
            modifiers,
            kind,
            ..
        } = match event {
            Event::Key(key) => key,
            _ => continue,
        };
        if kind != KeyEventKind::Press {
            continue;
        }
        match (code, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => bail!("cancelled"),
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) => bail!("cancelled"),
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                *cursor = (*cursor + agents.len() - 1) % agents.len();
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                *cursor = (*cursor + 1) % agents.len();
            }
            (KeyCode::Char(' '), _) => checked[*cursor] = !checked[*cursor],
            (KeyCode::Char('a'), _) => {
                let any_unchecked = checked.iter().any(|c| !c);
                for item in checked.iter_mut() {
                    *item = any_unchecked;
                }
            }
            (KeyCode::Enter, _) => {
                return Ok(agents
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| checked[*index])
                    .map(|(_, agent)| *agent)
                    .collect());
            }
            _ => {}
        }
    }
}

use crossterm::event::KeyEvent;
