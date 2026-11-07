use crossterm::style::Color as TermColor;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressFinish, ProgressStyle};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::checks::{CheckResult, merge_check_result, run_single_check};
use crate::command::command_exists;
use crate::output::{MsgKind, color_bold, log_pkg_line, print_section};
use crate::profile::interactive_terminal;
use crate::state::{
    AppState, TARGET_IDS, TargetStateFlags, section_title, target_enabled, target_label,
    target_state_flags, target_update_summary,
};

enum CheckEvent {
    Started(String),
    Finished {
        target: String,
        kind: MsgKind,
        summary: &'static str,
    },
}

pub fn resolve_check_targets(state: &AppState, requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        TARGET_IDS
            .iter()
            .filter(|target| target_enabled(state, target))
            .map(|x| x.to_string())
            .collect()
    } else {
        let mut uniq = Vec::new();
        for t in requested {
            if !uniq.iter().any(|x: &String| x == t) {
                uniq.push(t.clone());
            }
        }
        uniq
    }
}

fn spawn_check_workers(
    targets: &[String],
) -> (JoinSet<CheckResult>, mpsc::UnboundedReceiver<CheckEvent>) {
    let mut set = JoinSet::new();
    let (tx, rx) = mpsc::unbounded_channel::<CheckEvent>();
    for target in targets {
        let t = target.clone();
        let tx_task = tx.clone();
        set.spawn(async move {
            let _ = tx_task.send(CheckEvent::Started(t.clone()));
            crate::terminal::business::report(&t, crate::terminal::business::Status::Checking);
            let result = run_single_check(&t).await;
            crate::terminal::business::checked(&t, &result.state);
            let (kind, summary) = summarize_target_status(&t, &result.state);
            let _ = tx_task.send(CheckEvent::Finished {
                target: t,
                kind,
                summary,
            });
            result
        });
    }
    drop(tx);
    (set, rx)
}

async fn finish_check_workers(
    state: &mut AppState,
    mut handles: JoinSet<CheckResult>,
) -> HashMap<String, Vec<String>> {
    let mut logs_map: HashMap<String, Vec<String>> = HashMap::new();
    while let Some(joined) = handles.join_next().await {
        if let Ok(result) = joined {
            merge_check_result(state, &result.target, result.state);
            logs_map.insert(result.target, result.logs);
        }
    }
    logs_map
}

pub async fn run_checks_plain(state: &mut AppState, targets: &[String]) {
    let (handles, mut rx) = spawn_check_workers(targets);
    print_check_header(targets);
    print_check_events(&mut rx, targets.len()).await;

    let logs_map = finish_check_workers(state, handles).await;
    print_check_logs(targets, &logs_map);
}

fn print_check_header(targets: &[String]) {
    if crate::terminal::business::is_worker() {
        return;
    }
    println!(
        "{} {}",
        color_bold("[check]", TermColor::Yellow),
        targets
            .iter()
            .map(|t| target_label(t))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

async fn print_check_events(rx: &mut mpsc::UnboundedReceiver<CheckEvent>, target_count: usize) {
    let progress = check_progress(target_count);
    while let Some(event) = rx.recv().await {
        if crate::terminal::business::is_worker() {
            continue;
        }
        let line = check_event_line(&event);
        if let Some(progress) = &progress {
            if matches!(event, CheckEvent::Finished { .. }) {
                progress.inc(1);
            }
            progress.println(line);
        } else {
            println!("{line}");
        }
    }
    if let Some(progress) = progress {
        progress.finish_and_clear();
    }
}

fn check_progress(target_count: usize) -> Option<ProgressBar> {
    if target_count == 0 || crate::terminal::business::is_worker() || !interactive_terminal() {
        return None;
    }
    let draw_target = ProgressDrawTarget::stdout_with_hz(10);
    if draw_target.is_hidden() {
        return None;
    }
    let progress = ProgressBar::with_draw_target(Some(target_count as u64), draw_target)
        .with_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} 检查更新 {pos}/{len} [{elapsed_precise}]",
            )
            .expect("valid check progress template"),
        )
        .with_finish(ProgressFinish::AndClear);
    progress.enable_steady_tick(Duration::from_millis(100));
    progress.tick();
    Some(progress)
}

fn check_event_line(event: &CheckEvent) -> String {
    match event {
        CheckEvent::Started(target) => log_pkg_line(target, "开始检查...", MsgKind::Info),
        CheckEvent::Finished {
            target,
            kind,
            summary,
        } => log_pkg_line(target, &format!("检查完成: {summary}"), *kind),
    }
}

fn print_check_logs(targets: &[String], logs_map: &HashMap<String, Vec<String>>) {
    for target in targets {
        if crate::terminal::business::is_worker() {
            println!("\n── {} · 检查结果 ──", section_title(target));
        } else {
            print_section(section_title(target));
        }
        if let Some(lines) = logs_map.get(target) {
            print_lines(lines);
        }
    }
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

pub fn build_upgradable_targets(state: &AppState) -> Vec<String> {
    TARGET_IDS
        .iter()
        .filter(|target| {
            target_state_flags(state, target)
                .map(|flags| flags.has_updates)
                .unwrap_or(false)
        })
        .map(|target| target.to_string())
        .collect()
}

pub fn any_check_failed(state: &AppState) -> bool {
    TARGET_IDS.iter().any(|target| {
        target_state_flags(state, target)
            .map(|flags| flags.check_failed)
            .unwrap_or(false)
    })
}

pub async fn cargo_update_missing(state: &AppState) -> bool {
    state.enable.cargo
        && state.cargo.installed
        && !state.cargo.updater_installed
        && command_exists("cargo").await
        && !command_exists("cargo-install-update").await
}

pub async fn cargo_binstall_missing(state: &AppState) -> bool {
    !state.is_termux
        && state.enable.cargo
        && state.cargo.installed
        && command_exists("cargo").await
        && !command_exists("cargo-binstall").await
}

pub fn summarize_target_status(target: &str, state: &AppState) -> (MsgKind, &'static str) {
    target_state_flags(state, target)
        .map(|flags| summarize_known_target_status(target, flags))
        .unwrap_or((MsgKind::Warn, "未知状态"))
}

fn summarize_known_target_status(target: &str, flags: TargetStateFlags) -> (MsgKind, &'static str) {
    if target_skipped(flags) {
        (MsgKind::Warn, "已跳过")
    } else {
        summarize_checked_target_status(target, flags)
    }
}

fn target_skipped(flags: TargetStateFlags) -> bool {
    !flags.enabled || !flags.installed
}

fn summarize_checked_target_status(
    target: &str,
    flags: TargetStateFlags,
) -> (MsgKind, &'static str) {
    if flags.needs_cargo_updater {
        (MsgKind::Warn, "缺少 cargo-update")
    } else if flags.check_failed {
        (MsgKind::Warn, "检查失败")
    } else if flags.has_updates {
        (MsgKind::Warn, target_update_summary(target))
    } else {
        (MsgKind::Ok, "当前最新")
    }
}

#[cfg(test)]
mod tests {
    use super::summarize_target_status;
    use crate::output::MsgKind;
    use crate::state::AppState;

    #[test]
    fn summarize_target_status_reports_skipped_unknown_and_updates() {
        let state = AppState::default();
        let (_, skipped_summary) = summarize_target_status("brew", &state);
        let (_, unknown_summary) = summarize_target_status("missing", &state);

        assert_eq!(skipped_summary, "已跳过");
        assert_eq!(unknown_summary, "未知状态");

        let mut updated = AppState::default();
        updated.enable.rustup = true;
        updated.rustup.installed = true;
        updated.rustup.has_updates = true;
        let (kind, summary) = summarize_target_status("rustup", &updated);

        assert!(matches!(kind, MsgKind::Warn));
        assert_eq!(summary, "发现可升级项");
    }

    #[test]
    fn summarize_target_status_prioritizes_cargo_updater_warning() {
        let mut state = AppState::default();
        state.enable.cargo = true;
        state.cargo.installed = true;
        state.cargo.check_failed = true;
        state.cargo.has_updates = true;

        let (_, summary) = summarize_target_status("cargo", &state);

        assert_eq!(summary, "缺少 cargo-update");
    }
}
