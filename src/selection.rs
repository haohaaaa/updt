use inquire::{MultiSelect, error::InquireError};
use std::io::{self, IsTerminal};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::output::print_exit_signal_message;
use crate::profile::interactive_terminal;
use crate::state::{AppState, target_label, updatable_items_for_target};

#[cfg(test)]
#[path = "selection_tests.rs"]
mod prompt_tests;

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
        println!("{}", target_label(target));
        print_target_updatable_items(state, target);
        match prompt_target_upgrade(&mut stdin, &mut stdout, target).await {
            PromptAnswer::Yes => {
                selected_targets.push(target.clone());
            }
            PromptAnswer::No => {}
            PromptAnswer::Abort => return Vec::new(),
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
    if upgradable_targets.is_empty() {
        return Vec::new();
    }
    if !interactive_terminal() || !io::stderr().is_terminal() {
        return select_targets_prompt(state, upgradable_targets).await;
    }

    let items: Vec<_> = upgradable_targets
        .iter()
        .map(|target| {
            let count = updatable_items_for_target(state, target).len();
            format!("{} ({count} 项更新)", target_label(target))
        })
        .collect();
    let result = tokio::task::block_in_place(|| {
        MultiSelect::new("选择要升级的项目", items)
            .with_all_selected_by_default()
            .without_filtering()
            .with_help_message("↑/↓: 移动  Space: 勾选  ←: 全不选  →: 全选  Enter: 确认  Esc: 退出")
            .raw_prompt_skippable()
    });
    match result {
        Ok(Some(options)) => options
            .into_iter()
            .map(|option| upgradable_targets[option.index].clone())
            .collect(),
        Ok(None) => Vec::new(),
        Err(InquireError::OperationInterrupted) => {
            print_exit_signal_message();
            Vec::new()
        }
        Err(err) => {
            eprintln!("[prompt] 无法读取选择: {err}");
            Vec::new()
        }
    }
}

pub fn resolve_cli_selection(requested: &[String], upgradable_targets: &[String]) -> Vec<String> {
    let (selected, skipped) = resolve_cli_selection_quiet(requested, upgradable_targets);
    for target in &skipped {
        println!("[cli] {} 当前没有可升级项, 跳过.", target_label(target));
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
