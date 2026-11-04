use chrono::Local;
use crossterm::style::Color as TermColor;
use std::io;
use std::process;

mod checks;
mod cli;
mod command;
mod completion;
mod flow;
mod output;
mod parse;
mod profile;
mod selection;
mod state;
mod terminal;
mod ui;
mod upgrade;

use crate::cli::{CliCommand, parse_cli};
use crate::completion::install_fish_completion;
use crate::flow::check::{
    any_check_failed, build_upgradable_targets, resolve_check_targets, run_checks, run_checks_plain,
};
use crate::flow::interactive::{
    InteractiveResult, offer_install_cargo_binstall, offer_install_cargo_update,
    run_interactive_flow,
};
use crate::output::{color_bold, ok_text, print_exit_signal_message, print_section, warn_text};
use crate::profile::{interactive_terminal, parse_profile};
use crate::selection::{resolve_cli_selection, select_targets, select_targets_prompt};
use crate::state::{AppState, profile_name};
use crate::upgrade::upgrade_selected;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    process::exit(run_app().await);
}

struct RunContext {
    state: AppState,
    requested_updates: Vec<String>,
    start_time: String,
    force_text_flow: bool,
}

async fn run_app() -> i32 {
    let cli = parse_cli();
    if let Some(code) = handle_fish_command(&cli).await {
        return code;
    }
    if matches!(cli, CliCommand::TerminalDemo) {
        return terminal::run_demo().await;
    }
    if let CliCommand::TerminalWorker { owner_pid, .. } = &cli {
        terminal::business::enable_worker(*owner_pid);
    }
    if let CliCommand::Terminal {
        targets,
        demo,
        shell,
    } = &cli
    {
        let result = if *demo || *shell {
            let shell = terminal::default_shell().await;
            tokio::task::block_in_place(|| terminal::run(&shell, *demo))
        } else {
            let ctx = RunContext::new(targets.clone()).await;
            let check_targets = resolve_check_targets(&ctx.state, targets);
            let progress = terminal::business::Progress::new(
                &check_targets,
                profile_name(ctx.state.system_profile),
                &ctx.start_time,
            );
            tokio::task::block_in_place(|| terminal::run_workflow(targets, progress))
        };
        return match result {
            Ok(code) => code,
            Err(err) => {
                eprintln!("[terminal] 无法运行嵌入式终端: {err}");
                1
            }
        };
    }

    let mut ctx = RunContext::new(requested_updates(&cli)).await;
    if let Some(code) = try_interactive_start(&mut ctx).await {
        return code;
    }

    print_text_header_if_needed(&ctx);
    run_checks_for_context(&mut ctx).await;
    terminal::business::phase("preparing");
    offer_install_cargo_update(&mut ctx.state).await;
    offer_install_cargo_binstall(&ctx.state).await;
    finish_after_checks(&mut ctx).await
}

impl RunContext {
    async fn new(requested_updates: Vec<String>) -> Self {
        let mut state = AppState::default();
        parse_profile(&mut state).await;
        let start_time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Self {
            state,
            requested_updates,
            start_time,
            force_text_flow: terminal::business::is_worker(),
        }
    }
}

async fn handle_fish_command(cli: &CliCommand) -> Option<i32> {
    if !matches!(cli, CliCommand::Fish) {
        return None;
    }
    Some(match install_fish_completion().await {
        Ok(path) => {
            println!("fish completion 已写入: {}", path.display());
            0
        }
        Err(err) => {
            eprintln!("[fish] 写入失败: {err}");
            1
        }
    })
}

fn requested_updates(cli: &CliCommand) -> Vec<String> {
    match cli {
        CliCommand::Update(v) | CliCommand::TerminalWorker { targets: v, .. } => v.clone(),
        _ => Vec::new(),
    }
}

async fn try_interactive_start(ctx: &mut RunContext) -> Option<i32> {
    if interactive_terminal() && !terminal::business::is_worker() {
        return handle_interactive_result(
            run_interactive_flow(&mut ctx.state, &ctx.requested_updates, &ctx.start_time).await,
            ctx,
        )
        .await;
    }
    None
}

async fn handle_interactive_result(
    result: io::Result<InteractiveResult>,
    ctx: &mut RunContext,
) -> Option<i32> {
    match result {
        Ok(InteractiveResult::Exit(code)) => Some(code),
        Ok(InteractiveResult::RunUpgrade(selected_targets)) => {
            Some(upgrade_exit_code(&ctx.state, &selected_targets).await)
        }
        Err(err) => {
            if err.kind() == io::ErrorKind::Interrupted {
                print_exit_signal_message();
                return Some(0);
            }
            eprintln!("[ui] TUI 运行失败, 自动回退文本流程: {err}");
            ctx.force_text_flow = true;
            None
        }
    }
}

async fn upgrade_exit_code(state: &AppState, selected_targets: &[String]) -> i32 {
    terminal::business::phase("upgrading");
    if upgrade_selected(state, selected_targets).await {
        0
    } else {
        1
    }
}

fn print_text_header_if_needed(ctx: &RunContext) {
    if terminal::business::is_worker() {
        return;
    }
    if !interactive_terminal() || ctx.force_text_flow {
        print_section("检查可升级项");
        println!(
            "{}: {}",
            color_bold("开始时间", TermColor::Blue),
            ctx.start_time
        );
        println!(
            "{}: {}",
            color_bold("系统策略", TermColor::Blue),
            profile_name(ctx.state.system_profile)
        );
    }
}

async fn run_checks_for_context(ctx: &mut RunContext) {
    terminal::business::phase("checking");
    if ctx.force_text_flow {
        let targets = resolve_check_targets(&ctx.state, &ctx.requested_updates);
        run_checks_plain(&mut ctx.state, &targets).await;
    } else {
        run_checks(&mut ctx.state, &ctx.requested_updates, &ctx.start_time).await;
    }
}

async fn finish_after_checks(ctx: &mut RunContext) -> i32 {
    let upgradable_targets = build_upgradable_targets(&ctx.state);

    if upgradable_targets.is_empty() {
        return no_upgrades_exit_code(&ctx.state);
    }

    if !interactive_terminal() {
        print_section("选择要升级的项目");
    }
    let selected_targets = select_after_checks(ctx, &upgradable_targets).await;
    selected_upgrade_exit_code(&ctx.state, &selected_targets).await
}

fn no_upgrades_exit_code(state: &AppState) -> i32 {
    terminal::business::phase("summary");
    print_section("汇总");
    println!("{}", ok_text("没有可升级项."));
    if any_check_failed(state) {
        println!("{}", warn_text("但有检查失败, 请根据上方日志排查."));
        1
    } else {
        0
    }
}

async fn select_after_checks(ctx: &RunContext, upgradable_targets: &[String]) -> Vec<String> {
    terminal::business::phase("selecting");
    if !ctx.requested_updates.is_empty() {
        return resolve_cli_selection(&ctx.requested_updates, upgradable_targets);
    }
    if ctx.force_text_flow {
        select_targets_prompt(&ctx.state, upgradable_targets).await
    } else {
        select_targets(&ctx.state, upgradable_targets).await
    }
}

async fn selected_upgrade_exit_code(state: &AppState, selected_targets: &[String]) -> i32 {
    if selected_targets.is_empty() {
        println!("{}", warn_text("未选择任何升级项, 已退出."));
        return 0;
    }

    upgrade_exit_code(state, selected_targets).await
}
