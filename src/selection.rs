use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, IsTerminal};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::output::print_exit_signal_message;
use crate::state::{AppState, target_label, updatable_items_for_target};
use crate::ui::{TerminalGuard, select_targets_tui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoopBlockedAction {
    KillAndRetry,
    Skip,
    Abort,
}

fn print_target_updatable_items(state: &AppState, target: &str) {
    for item in updatable_items_for_target(state, target) {
        println!("  - {item}");
    }
}

pub async fn select_targets_prompt(state: &AppState, upgradable_targets: &[String]) -> Vec<String> {
    let mut selected_targets = Vec::<String>::new();
    println!("逐项确认待升级项目.");
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();

    for target in upgradable_targets {
        crate::terminal::business::report(target, crate::terminal::business::Status::Confirming);
        println!("{}", target_label(target));
        print_target_updatable_items(state, target);
        match prompt_target_upgrade(&mut stdin, &mut stdout, target).await {
            PromptAnswer::Yes => {
                selected_targets.push(target.clone());
                crate::terminal::business::report(
                    target,
                    crate::terminal::business::Status::Queued,
                );
            }
            PromptAnswer::No => crate::terminal::business::report(
                target,
                crate::terminal::business::Status::Skipped,
            ),
            PromptAnswer::Abort => {
                for target in upgradable_targets {
                    crate::terminal::business::report(
                        target,
                        crate::terminal::business::Status::Cancelled,
                    );
                }
                return Vec::new();
            }
        }
    }
    selected_targets
}

enum PromptAnswer {
    Yes,
    No,
    Abort,
}

async fn prompt_target_upgrade(
    stdin: &mut BufReader<tokio::io::Stdin>,
    stdout: &mut tokio::io::Stdout,
    target: &str,
) -> PromptAnswer {
    let message = format!("是否升级 {} [Y/n]: ", target_label(target));
    if write_prompt(stdout, &message).await.is_err() {
        return PromptAnswer::Abort;
    }
    prompt_answer_from_read(read_prompt_line(stdin).await)
}

fn prompt_answer_from_read(result: io::Result<Option<String>>) -> PromptAnswer {
    match result {
        Ok(Some(answer)) => prompt_answer_from_text(&answer),
        Ok(None) => abort_prompt_after_exit_signal(),
        Err(err) => prompt_answer_from_error(err),
    }
}

fn prompt_answer_from_text(answer: &str) -> PromptAnswer {
    if default_yes_answer(answer) {
        PromptAnswer::Yes
    } else {
        PromptAnswer::No
    }
}

fn abort_prompt_after_exit_signal() -> PromptAnswer {
    print_exit_signal_message();
    PromptAnswer::Abort
}

fn prompt_answer_from_error(err: io::Error) -> PromptAnswer {
    if err.kind() == io::ErrorKind::Interrupted {
        print_exit_signal_message();
    }
    PromptAnswer::Abort
}

pub async fn select_targets(state: &AppState, upgradable_targets: &[String]) -> Vec<String> {
    if terminal_selection_available() {
        let result = select_targets_with_tui(state, upgradable_targets).await;
        if let Some(chosen) = handle_tui_selection_result(result) {
            return chosen;
        }
    }
    select_targets_prompt(state, upgradable_targets).await
}

fn terminal_selection_available() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

async fn select_targets_with_tui(
    state: &AppState,
    upgradable_targets: &[String],
) -> io::Result<Vec<String>> {
    let _guard = TerminalGuard::enter().await?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    select_targets_tui(&mut terminal, state, upgradable_targets).await
}

fn handle_tui_selection_result(result: io::Result<Vec<String>>) -> Option<Vec<String>> {
    match result {
        Ok(chosen) => Some(chosen),
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            print_exit_signal_message();
            Some(Vec::new())
        }
        Err(err) => {
            eprintln!("[ui] TUI 初始化失败, 自动回退文本交互: {err}");
            None
        }
    }
}

pub fn resolve_cli_selection(requested: &[String], upgradable_targets: &[String]) -> Vec<String> {
    let mut selected = Vec::<String>::new();
    for req in requested {
        if selected.iter().any(|x| x == req) {
            continue;
        }
        if upgradable_targets.iter().any(|x| x == req) {
            selected.push(req.clone());
            crate::terminal::business::report(req, crate::terminal::business::Status::Queued);
        } else {
            println!("[cli] {} 当前没有可升级项, 跳过.", target_label(req));
        }
    }
    selected
}

pub fn resolve_cli_selection_quiet(
    requested: &[String],
    upgradable_targets: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut selected = Vec::<String>::new();
    let mut skipped = Vec::<String>::new();
    for req in requested {
        if selected.iter().any(|x| x == req) || skipped.iter().any(|x| x == req) {
            continue;
        }
        if upgradable_targets.iter().any(|x| x == req) {
            selected.push(req.clone());
        } else {
            skipped.push(req.clone());
        }
    }
    (selected, skipped)
}

pub async fn confirm_default_yes(prompt: &str) -> Option<bool> {
    let mut stdout = tokio::io::stdout();
    if write_prompt(&mut stdout, &format!("{prompt} [Y/n]: "))
        .await
        .is_err()
    {
        return None;
    }
    let mut stdin = BufReader::new(tokio::io::stdin());
    default_yes_from_read(read_prompt_line(&mut stdin).await)
}

fn default_yes_from_read(result: io::Result<Option<String>>) -> Option<bool> {
    result
        .ok()
        .flatten()
        .map(|answer| default_yes_answer(&answer))
}

async fn write_prompt(stdout: &mut tokio::io::Stdout, message: &str) -> io::Result<()> {
    stdout.write_all(message.as_bytes()).await?;
    stdout.flush().await
}

async fn read_prompt_line(stdin: &mut BufReader<tokio::io::Stdin>) -> io::Result<Option<String>> {
    let mut answer = String::new();
    match stdin.read_line(&mut answer).await? {
        0 => Ok(None),
        _ => Ok(Some(answer)),
    }
}

fn default_yes_answer(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

pub async fn prompt_scoop_blocked_action(
    app_label: &str,
    details: &[String],
) -> ScoopBlockedAction {
    println!("[scoop] {app_label} 因运行中进程被阻塞.");
    for line in details {
        println!("{line}");
    }
    println!("可选操作: [k] Kill & Retry, [s] Skip, [a] Abort");

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();

    loop {
        match read_scoop_blocked_action(&mut stdin, &mut stdout).await {
            Some(action) => return action,
            None => println!("请输入 k、s 或 a."),
        }
    }
}

async fn read_scoop_blocked_action(
    stdin: &mut BufReader<tokio::io::Stdin>,
    stdout: &mut tokio::io::Stdout,
) -> Option<ScoopBlockedAction> {
    if write_prompt(stdout, "选择操作 [k/s/a]: ").await.is_err() {
        return Some(ScoopBlockedAction::Abort);
    }
    scoop_blocked_action_from_read(read_prompt_line(stdin).await)
}

fn scoop_blocked_action_from_read(
    result: io::Result<Option<String>>,
) -> Option<ScoopBlockedAction> {
    match result {
        Ok(Some(answer)) => parse_scoop_blocked_action(&answer),
        Ok(None) => {
            print_exit_signal_message();
            Some(ScoopBlockedAction::Abort)
        }
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            print_exit_signal_message();
            Some(ScoopBlockedAction::Abort)
        }
        Err(_) => Some(ScoopBlockedAction::Abort),
    }
}

fn parse_scoop_blocked_action(answer: &str) -> Option<ScoopBlockedAction> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "k" | "kill" | "retry" | "r" => Some(ScoopBlockedAction::KillAndRetry),
        "s" | "skip" => Some(ScoopBlockedAction::Skip),
        "a" | "abort" | "q" | "quit" => Some(ScoopBlockedAction::Abort),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ScoopBlockedAction, default_yes_answer, parse_scoop_blocked_action,
        resolve_cli_selection_quiet,
    };

    #[test]
    fn default_yes_answer_accepts_empty_and_yes() {
        assert!(default_yes_answer(""));
        assert!(default_yes_answer(" y "));
        assert!(default_yes_answer("YES"));
    }

    #[test]
    fn default_yes_answer_rejects_other_answers() {
        assert!(!default_yes_answer("n"));
        assert!(!default_yes_answer("no"));
        assert!(!default_yes_answer("later"));
    }

    #[test]
    fn parses_scoop_blocked_actions() {
        assert_eq!(
            parse_scoop_blocked_action("kill"),
            Some(ScoopBlockedAction::KillAndRetry)
        );
        assert_eq!(
            parse_scoop_blocked_action("s"),
            Some(ScoopBlockedAction::Skip)
        );
        assert_eq!(
            parse_scoop_blocked_action("Q"),
            Some(ScoopBlockedAction::Abort)
        );
        assert_eq!(parse_scoop_blocked_action(""), None);
    }

    #[test]
    fn resolves_cli_selection_quiet_dedupes_selected_and_skipped() {
        let requested = vec![
            "brew".to_string(),
            "npm".to_string(),
            "brew".to_string(),
            "missing".to_string(),
            "missing".to_string(),
        ];
        let upgradable = vec!["brew".to_string(), "cargo".to_string()];

        assert_eq!(
            resolve_cli_selection_quiet(&requested, &upgradable),
            (
                vec!["brew".to_string()],
                vec!["npm".to_string(), "missing".to_string()]
            )
        );
    }
}
