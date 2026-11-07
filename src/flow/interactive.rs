use std::io;

use crate::checks::{check_cargo_quiet, merge_check_result};
use crate::command::run_inherit;
use crate::flow::check::{cargo_binstall_missing, cargo_update_missing};
use crate::output::{err_text, print_exit_signal_message, print_section, warn_text};
use crate::profile::{interactive_terminal, parse_profile};
use crate::selection::confirm_default_yes;
use crate::state::{AppState, section_title};

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
