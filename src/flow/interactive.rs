use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use tokio::task::block_in_place;

use crate::checks::{check_cargo_quiet, merge_check_result};
use crate::command::run_inherit;
use crate::flow::check::{
    any_check_failed, build_upgradable_targets, cargo_binstall_missing, cargo_update_missing,
    resolve_check_targets, run_checks_tui,
};
use crate::output::{err_text, print_exit_signal_message, print_section, warn_text};
use crate::profile::{interactive_terminal, parse_profile};
use crate::selection::{confirm_default_yes, resolve_cli_selection_quiet};
use crate::state::{AppState, section_title, target_label};
use crate::ui::{
    AppTerminal, ChecksConfirmView, SelectionConfirmView, TerminalGuard,
    select_targets_tui_with_checks, wait_tui_float_on_checks, wait_tui_float_on_selection,
    wait_tui_message, wait_tui_message_on_checks,
};

async fn run_inherit_outside_tui(
    terminal: &mut AppTerminal,
    program: &str,
    args: &[&str],
) -> io::Result<bool> {
    suspend_tui_for_inherited_command()?;
    let command_result = run_inherit(program, args).await;
    restore_tui_after_inherited_command(terminal)?;
    command_result
}

fn suspend_tui_for_inherited_command() -> io::Result<()> {
    block_in_place(|| {
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)
    })
}

fn restore_tui_after_inherited_command(terminal: &mut AppTerminal) -> io::Result<()> {
    let restore_error = restore_raw_mode_error().or(restore_alternate_screen_error());
    if restore_error.is_none() {
        let _ = block_in_place(|| terminal.clear());
    }
    restore_error.map_or(Ok(()), Err)
}

fn restore_raw_mode_error() -> Option<io::Error> {
    block_in_place(enable_raw_mode).err()
}

fn restore_alternate_screen_error() -> Option<io::Error> {
    block_in_place(|| execute!(io::stdout(), EnterAlternateScreen)).err()
}

pub async fn offer_install_cargo_update_tui(
    terminal: &mut AppTerminal,
    state: &mut AppState,
) -> io::Result<()> {
    if !cargo_update_missing(state).await {
        return Ok(());
    }

    if !confirm_cargo_update_install_tui(terminal).await? {
        return Ok(());
    }

    let install_result =
        run_inherit_outside_tui(terminal, "cargo", &["install", "cargo-update"]).await;
    handle_cargo_update_install_tui_result(terminal, state, install_result).await
}

pub async fn offer_install_cargo_binstall_tui(
    terminal: &mut AppTerminal,
    state: &AppState,
    targets: &[String],
    start_time: &str,
) -> io::Result<()> {
    if !cargo_binstall_missing(state).await {
        return Ok(());
    }

    if !confirm_cargo_binstall_install_tui(terminal, state, targets, start_time).await? {
        return Ok(());
    }

    let install_result =
        run_inherit_outside_tui(terminal, "cargo", &["install", "cargo-binstall"]).await;
    show_cargo_binstall_tui_result(terminal, install_result).await
}

async fn confirm_cargo_update_install_tui(terminal: &mut AppTerminal) -> io::Result<bool> {
    wait_tui_message(
        terminal,
        "cargo-update",
        &[
            "未安装 cargo-install-update, 无法检查已安装 crate 更新.".to_string(),
            "是否执行 cargo install cargo-update? 默认: Yes".to_string(),
            "".to_string(),
            "Enter/Y: 直连终端执行    N/q/Esc: 跳过".to_string(),
        ],
    )
    .await
}

async fn confirm_cargo_binstall_install_tui(
    terminal: &mut AppTerminal,
    state: &AppState,
    targets: &[String],
    start_time: &str,
) -> io::Result<bool> {
    let lines = [
        "未安装 cargo-binstall.".to_string(),
        "建议安装它, 以便 cargo 管理的软件可优先使用预编译二进制快速安装或升级.".to_string(),
        "是否执行 cargo install cargo-binstall?".to_string(),
        "".to_string(),
        "左右键: 选择按钮    Enter: 确认".to_string(),
    ];
    let confirm_view = ChecksConfirmView {
        state,
        targets,
        start_time,
        title: "安装 cargo-binstall",
        lines: &lines,
    };
    wait_tui_float_on_checks(terminal, &confirm_view).await
}

async fn show_cargo_binstall_tui_result(
    terminal: &mut AppTerminal,
    install_result: io::Result<bool>,
) -> io::Result<()> {
    let message = match install_result {
        Ok(true) => "cargo-binstall 安装完成.".to_string(),
        Ok(false) => "cargo-binstall 安装失败 (退出码非 0).".to_string(),
        Err(err) => format!("cargo-binstall 安装失败: {err}"),
    };
    let _ = wait_tui_message(
        terminal,
        "cargo-binstall",
        &[
            message,
            "".to_string(),
            "Enter: 继续    q/Esc: 继续".to_string(),
        ],
    )
    .await?;
    Ok(())
}

async fn handle_cargo_update_install_tui_result(
    terminal: &mut AppTerminal,
    state: &mut AppState,
    install_result: io::Result<bool>,
) -> io::Result<()> {
    match cargo_update_tui_outcome(install_result) {
        CargoUpdateTuiOutcome::Installed => show_cargo_update_tui_success(terminal, state).await,
        CargoUpdateTuiOutcome::Failed(message) => {
            show_cargo_update_tui_failure(terminal, state, &message).await
        }
    }
}

enum CargoUpdateTuiOutcome {
    Installed,
    Failed(String),
}

fn cargo_update_tui_outcome(install_result: io::Result<bool>) -> CargoUpdateTuiOutcome {
    match install_result {
        Ok(true) => CargoUpdateTuiOutcome::Installed,
        Ok(false) => {
            CargoUpdateTuiOutcome::Failed("cargo-update 安装失败 (退出码非 0).".to_string())
        }
        Err(err) => CargoUpdateTuiOutcome::Failed(format!("cargo-update 安装失败: {err}")),
    }
}

async fn show_cargo_update_tui_success(
    terminal: &mut AppTerminal,
    state: &mut AppState,
) -> io::Result<()> {
    let logs = recheck_cargo_after_update_install(state).await;
    let lines = cargo_update_tui_success_lines(logs);
    let _ = wait_tui_message(terminal, "cargo-update", &lines).await?;
    Ok(())
}

async fn show_cargo_update_tui_failure(
    terminal: &mut AppTerminal,
    state: &mut AppState,
    message: &str,
) -> io::Result<()> {
    state.cargo.check_failed = true;
    let lines = cargo_update_tui_failure_lines(message);
    let _ = wait_tui_message(terminal, "cargo-update", &lines).await?;
    Ok(())
}

async fn recheck_cargo_after_update_install(state: &mut AppState) -> Vec<String> {
    state.cargo.has_updates = false;
    state.cargo.check_failed = false;
    state.cargo.updater_installed = false;
    state.cargo.updatable_packages.clear();

    let mut logs = Vec::new();
    let mut local = AppState::default();
    parse_profile(&mut local).await;
    check_cargo_quiet(&mut local, &mut logs).await;
    merge_check_result(state, "cargo", local);
    logs
}

fn cargo_update_tui_success_lines(logs: Vec<String>) -> Vec<String> {
    let mut lines = vec!["cargo-update 安装完成, 已重新检查 cargo.".to_string()];
    lines.extend(logs);
    lines.extend(cargo_update_tui_continue_lines());
    lines
}

fn cargo_update_tui_failure_lines(message: &str) -> Vec<String> {
    let mut lines = vec![message.to_string()];
    lines.extend(cargo_update_tui_continue_lines());
    lines
}

fn cargo_update_tui_continue_lines() -> [String; 2] {
    ["".to_string(), "Enter: 继续    q/Esc: 继续".to_string()]
}

pub async fn offer_install_cargo_update(state: &mut AppState) {
    if !interactive_terminal() || !cargo_update_missing(state).await {
        return;
    }

    print_section("cargo-update");
    println!("未安装 cargo-install-update, 无法检查已安装 crate 更新.");
    if !confirm_cargo_update_install_text().await {
        return;
    }

    crate::output::print_command("cargo install cargo-update");
    let install_result = run_inherit("cargo", &["install", "cargo-update"]).await;
    handle_cargo_update_install_text_result(state, install_result).await;
}

pub async fn offer_install_cargo_binstall(state: &AppState) {
    if !interactive_terminal() || !cargo_binstall_missing(state).await {
        return;
    }

    print_section("cargo-binstall");
    println!("未安装 cargo-binstall.");
    println!("建议安装它, 以便 cargo 管理的软件可优先使用预编译二进制快速安装或升级.");
    if !confirm_cargo_binstall_install_text().await {
        return;
    }

    crate::output::print_command("cargo install cargo-binstall");
    match run_inherit("cargo", &["install", "cargo-binstall"]).await {
        Ok(true) => println!("[cargo] cargo-binstall 安装完成."),
        _ => println!("{}", err_text("[cargo] cargo-binstall 安装失败.")),
    }
}

async fn confirm_cargo_update_install_text() -> bool {
    match confirm_default_yes("是否执行 cargo install cargo-update").await {
        Some(true) => true,
        Some(false) => {
            println!("{}", warn_text("已跳过 cargo-update 安装."));
            false
        }
        None => exit_after_interrupted_prompt(),
    }
}

async fn confirm_cargo_binstall_install_text() -> bool {
    match confirm_default_yes("是否执行 cargo install cargo-binstall").await {
        Some(true) => true,
        Some(false) => {
            println!("{}", warn_text("已跳过 cargo-binstall 安装."));
            false
        }
        None => exit_after_interrupted_prompt(),
    }
}

fn exit_after_interrupted_prompt() -> ! {
    print_exit_signal_message();
    std::process::exit(0);
}

async fn handle_cargo_update_install_text_result(
    state: &mut AppState,
    install_result: io::Result<bool>,
) {
    if let Ok(true) = install_result {
        println!("[cargo] cargo-update 安装完成, 正在重新检查 cargo.");
        let logs = recheck_cargo_after_update_install(state).await;
        print_section(section_title("cargo"));
        print_lines(logs);
        return;
    }
    state.cargo.check_failed = true;
    println!("{}", err_text("[cargo] cargo-update 安装失败."));
}

fn print_lines(lines: Vec<String>) {
    for line in lines {
        println!("{line}");
    }
}

pub enum InteractiveResult {
    Exit(i32),
    RunUpgrade(Vec<String>),
}

struct InteractiveTerminal {
    _guard: TerminalGuard,
    terminal: AppTerminal,
}

pub async fn run_interactive_flow(
    state: &mut AppState,
    requested_updates: &[String],
    start_time: &str,
) -> io::Result<InteractiveResult> {
    let mut session = enter_interactive_terminal().await?;

    let targets = resolve_check_targets(state, requested_updates);
    run_checks_tui(&mut session.terminal, state, &targets, start_time).await?;
    offer_install_cargo_update_tui(&mut session.terminal, state).await?;
    offer_install_cargo_binstall_tui(&mut session.terminal, state, &targets, start_time).await?;
    continue_interactive_after_checks(
        &mut session.terminal,
        state,
        requested_updates,
        start_time,
        &targets,
    )
    .await
}

async fn enter_interactive_terminal() -> io::Result<InteractiveTerminal> {
    let guard = TerminalGuard::enter().await?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(InteractiveTerminal {
        _guard: guard,
        terminal,
    })
}

async fn continue_interactive_after_checks(
    terminal: &mut AppTerminal,
    state: &AppState,
    requested_updates: &[String],
    start_time: &str,
    targets: &[String],
) -> io::Result<InteractiveResult> {
    let upgradable_targets = build_upgradable_targets(state);
    if upgradable_targets.is_empty() {
        return no_upgrades_interactive_result(terminal, state, targets, start_time).await;
    }

    interactive_selection_flow(
        terminal,
        state,
        requested_updates,
        start_time,
        targets,
        &upgradable_targets,
    )
    .await
}

async fn no_upgrades_interactive_result(
    terminal: &mut AppTerminal,
    state: &AppState,
    targets: &[String],
    start_time: &str,
) -> io::Result<InteractiveResult> {
    let exit_code = no_upgrades_exit_code(state);
    let lines = no_upgrades_lines(exit_code);
    let _ =
        wait_tui_message_on_checks(terminal, state, targets, start_time, "汇总", &lines).await?;
    Ok(InteractiveResult::Exit(exit_code))
}

fn no_upgrades_exit_code(state: &AppState) -> i32 {
    if any_check_failed(state) { 1 } else { 0 }
}

fn no_upgrades_lines(exit_code: i32) -> Vec<String> {
    let mut lines = vec!["没有可升级项.".to_string()];
    if exit_code != 0 {
        lines.push("但有检查失败, 请根据检查结果排查.".to_string());
    }
    lines.push("".to_string());
    lines.push("Enter/q/Esc: 退出".to_string());
    lines
}

async fn interactive_selection_flow(
    terminal: &mut AppTerminal,
    state: &AppState,
    requested_updates: &[String],
    start_time: &str,
    targets: &[String],
    upgradable_targets: &[String],
) -> io::Result<InteractiveResult> {
    loop {
        match interactive_selection_step(
            terminal,
            state,
            requested_updates,
            start_time,
            targets,
            upgradable_targets,
        )
        .await?
        {
            InteractiveSelectionStep::Continue => {}
            InteractiveSelectionStep::Exit(code) => return Ok(InteractiveResult::Exit(code)),
            InteractiveSelectionStep::RunUpgrade(targets) => {
                return Ok(InteractiveResult::RunUpgrade(targets));
            }
        }
    }
}

enum InteractiveSelectionStep {
    Continue,
    Exit(i32),
    RunUpgrade(Vec<String>),
}

async fn interactive_selection_step(
    terminal: &mut AppTerminal,
    state: &AppState,
    requested_updates: &[String],
    start_time: &str,
    targets: &[String],
    upgradable_targets: &[String],
) -> io::Result<InteractiveSelectionStep> {
    let selected_targets = select_targets_for_interactive_upgrade(
        terminal,
        state,
        requested_updates,
        start_time,
        targets,
        upgradable_targets,
    )
    .await?;
    if selected_targets.is_empty() {
        let result = empty_selection_interactive_result(terminal).await?;
        return Ok(InteractiveSelectionStep::Exit(interactive_exit_code(
            result,
        )));
    }
    if confirm_interactive_upgrade(
        terminal,
        state,
        start_time,
        targets,
        upgradable_targets,
        &selected_targets,
    )
    .await?
    {
        return Ok(InteractiveSelectionStep::RunUpgrade(selected_targets));
    }
    Ok(interactive_selection_after_declined_confirmation(
        requested_updates,
    ))
}

fn interactive_exit_code(result: InteractiveResult) -> i32 {
    match result {
        InteractiveResult::Exit(code) => code,
        InteractiveResult::RunUpgrade(_) => 0,
    }
}

fn interactive_selection_after_declined_confirmation(
    requested_updates: &[String],
) -> InteractiveSelectionStep {
    if requested_updates.is_empty() {
        InteractiveSelectionStep::Continue
    } else {
        InteractiveSelectionStep::Exit(0)
    }
}

async fn select_targets_for_interactive_upgrade(
    terminal: &mut AppTerminal,
    state: &AppState,
    requested_updates: &[String],
    start_time: &str,
    targets: &[String],
    upgradable_targets: &[String],
) -> io::Result<Vec<String>> {
    if requested_updates.is_empty() {
        return select_targets_tui_with_checks(
            terminal,
            state,
            upgradable_targets,
            targets,
            start_time,
        )
        .await;
    }
    selected_cli_targets_for_interactive_upgrade(terminal, requested_updates, upgradable_targets)
        .await
}

async fn selected_cli_targets_for_interactive_upgrade(
    terminal: &mut AppTerminal,
    requested_updates: &[String],
    upgradable_targets: &[String],
) -> io::Result<Vec<String>> {
    let (selected, skipped) = resolve_cli_selection_quiet(requested_updates, upgradable_targets);
    show_skipped_cli_targets(terminal, &skipped).await?;
    Ok(selected)
}

async fn show_skipped_cli_targets(
    terminal: &mut AppTerminal,
    skipped: &[String],
) -> io::Result<()> {
    if skipped.is_empty() {
        return Ok(());
    }
    let lines = skipped_cli_target_lines(skipped);
    let _ = wait_tui_message(terminal, "CLI 选择", &lines).await?;
    Ok(())
}

fn skipped_cli_target_lines(skipped: &[String]) -> Vec<String> {
    let mut lines = vec!["以下请求目标当前没有可升级项:".to_string()];
    lines.extend(
        skipped
            .iter()
            .map(|target| format!("  - {}", target_label(target))),
    );
    lines.push("".to_string());
    lines.push("Enter: 继续    q/Esc: 继续".to_string());
    lines
}

async fn empty_selection_interactive_result(
    terminal: &mut AppTerminal,
) -> io::Result<InteractiveResult> {
    let _ = wait_tui_message(terminal, "汇总", &empty_selection_lines()).await?;
    Ok(InteractiveResult::Exit(0))
}

fn empty_selection_lines() -> [String; 3] {
    [
        "未选择任何升级项, 已退出.".to_string(),
        "".to_string(),
        "Enter/q/Esc: 退出".to_string(),
    ]
}

async fn confirm_interactive_upgrade(
    terminal: &mut AppTerminal,
    state: &AppState,
    start_time: &str,
    targets: &[String],
    upgradable_targets: &[String],
    selected_targets: &[String],
) -> io::Result<bool> {
    let lines = selected_upgrade_lines(selected_targets);
    let confirm_view = SelectionConfirmView {
        state,
        check_targets: targets,
        start_time,
        upgradable_targets,
        selected_targets,
        title: "执行升级",
        lines: &lines,
    };
    wait_tui_float_on_selection(terminal, &confirm_view).await
}

fn selected_upgrade_lines(selected_targets: &[String]) -> Vec<String> {
    let mut lines = vec!["已选择升级项:".to_string()];
    lines.extend(
        selected_targets
            .iter()
            .map(|target| format!("  - {}", target_label(target))),
    );
    lines.push("".to_string());
    lines.push("左右键: 选择按钮    Enter: 确认".to_string());
    lines
}
