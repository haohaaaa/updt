use serde_json::Value;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::fs;

use crate::command::{command_exists, run_capture, run_cargo_install_update_capture, run_output};
use crate::output::{MsgKind, log_pkg_line};
use crate::parse::{
    ScoopExportApp, first_json_payload, first_token, parse_cargo_list, parse_scoop_export_output,
};
use crate::profile::parse_profile;
use crate::state::{AppState, SystemProfile};

const NPM_OUTDATED_ARGS: &[&str] = &["outdated", "--json", "--global", "--prefer-online"];
const DNF_CHECK_ARGS: &[&str] = &["check-update", "--quiet", "--json"];

pub struct CheckResult {
    pub target: String,
    pub state: AppState,
    pub logs: Vec<String>,
}

type CheckFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
type CheckRunner = for<'a> fn(&'a mut AppState, &'a mut Vec<String>) -> CheckFuture<'a>;
type CheckMerger = fn(&mut AppState, AppState);

struct CheckTargetHandler {
    target: &'static str,
    check: CheckRunner,
    merge: CheckMerger,
}

async fn ensure_check_target(
    enabled: bool,
    installed: &mut bool,
    command: &str,
    target: &str,
    logs: &mut Vec<String>,
) -> bool {
    if !enabled {
        logs.push(log_pkg_line(target, "按系统策略跳过.", MsgKind::Warn));
        return false;
    }
    if !command_exists(command).await {
        logs.push(log_pkg_line(target, "未安装, 跳过.", MsgKind::Warn));
        return false;
    }
    *installed = true;
    true
}

fn mark_check_failed(check_failed: &mut bool, target: &str, message: &str, logs: &mut Vec<String>) {
    *check_failed = true;
    logs.push(log_pkg_line(target, message, MsgKind::Warn));
}

fn log_item_list(logs: &mut Vec<String>, target: &str, header: &str, items: &[String]) {
    logs.push(log_pkg_line(target, header, MsgKind::Info));
    for item in items {
        logs.push(format!("  - {item}"));
    }
}

fn record_bucket_items(
    has_updates: &mut bool,
    logs: &mut Vec<String>,
    target: &str,
    items: &[String],
    ok_message: &str,
    update_header: &str,
) {
    if items.is_empty() {
        logs.push(log_pkg_line(target, ok_message, MsgKind::Ok));
    } else {
        *has_updates = true;
        log_item_list(logs, target, update_header, items);
    }
}

fn package_names_from_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(first_token)
        .collect()
}

macro_rules! check_runner {
    ($name:ident, $check:ident) => {
        fn $name<'a>(state: &'a mut AppState, logs: &'a mut Vec<String>) -> CheckFuture<'a> {
            Box::pin($check(state, logs))
        }
    };
}

check_runner!(run_brew_check, check_brew_quiet);
check_runner!(run_npm_check, check_npm_quiet);
check_runner!(run_cargo_check, check_cargo_quiet);

check_runner!(run_rustup_check, check_rustup_quiet);
check_runner!(run_scoop_check, check_scoop_quiet);
check_runner!(run_paru_check, check_paru_quiet);
check_runner!(run_flatpak_check, check_flatpak_quiet);
check_runner!(run_pacman_check, check_pacman_quiet);
check_runner!(run_dnf_check, check_dnf_quiet);
check_runner!(run_pkg_check, check_pkg_quiet);

fn merge_brew_check(state: &mut AppState, local: AppState) {
    state.brew = local.brew;
}

fn merge_npm_check(state: &mut AppState, local: AppState) {
    state.npm = local.npm;
}

fn merge_cargo_check(state: &mut AppState, local: AppState) {
    state.cargo = local.cargo;
}

fn merge_rustup_check(state: &mut AppState, local: AppState) {
    state.rustup = local.rustup;
}

fn merge_scoop_check(state: &mut AppState, local: AppState) {
    state.scoop = local.scoop;
}

fn merge_paru_check(state: &mut AppState, local: AppState) {
    state.paru = local.paru;
}

fn merge_flatpak_check(state: &mut AppState, local: AppState) {
    state.flatpak = local.flatpak;
}

fn merge_pacman_check(state: &mut AppState, local: AppState) {
    state.pacman = local.pacman;
}

fn merge_dnf_check(state: &mut AppState, local: AppState) {
    state.dnf = local.dnf;
}

fn merge_pkg_check(state: &mut AppState, local: AppState) {
    state.pkg = local.pkg;
}

static CHECK_TARGET_HANDLERS: &[CheckTargetHandler] = &[
    CheckTargetHandler {
        target: "brew",
        check: run_brew_check,
        merge: merge_brew_check,
    },
    CheckTargetHandler {
        target: "npm",
        check: run_npm_check,
        merge: merge_npm_check,
    },
    CheckTargetHandler {
        target: "cargo",
        check: run_cargo_check,
        merge: merge_cargo_check,
    },
    CheckTargetHandler {
        target: "rustup",
        check: run_rustup_check,
        merge: merge_rustup_check,
    },
    CheckTargetHandler {
        target: "scoop",
        check: run_scoop_check,
        merge: merge_scoop_check,
    },
    CheckTargetHandler {
        target: "paru",
        check: run_paru_check,
        merge: merge_paru_check,
    },
    CheckTargetHandler {
        target: "flatpak",
        check: run_flatpak_check,
        merge: merge_flatpak_check,
    },
    CheckTargetHandler {
        target: "pacman",
        check: run_pacman_check,
        merge: merge_pacman_check,
    },
    CheckTargetHandler {
        target: "dnf",
        check: run_dnf_check,
        merge: merge_dnf_check,
    },
    CheckTargetHandler {
        target: "pkg",
        check: run_pkg_check,
        merge: merge_pkg_check,
    },
];

fn check_target_handler(target: &str) -> Option<&'static CheckTargetHandler> {
    CHECK_TARGET_HANDLERS
        .iter()
        .find(|handler| handler.target == target)
}

async fn check_brew_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.brew,
        &mut state.brew.installed,
        "brew",
        "brew",
        logs,
    )
    .await
    {
        return;
    }
    logs.push(log_pkg_line(
        "brew",
        "正在检查可升级项 (brew outdated --greedy --json=v2)...",
        MsgKind::Info,
    ));

    let Some(root) = load_brew_outdated_json(&mut state.brew.check_failed, logs).await else {
        return;
    };

    state.brew.formula_list = brew_json_names(&root, "formulae");
    state.brew.cask_list = brew_json_names(&root, "casks");
    record_brew_updates(state, logs);
}

async fn load_brew_outdated_json(check_failed: &mut bool, logs: &mut Vec<String>) -> Option<Value> {
    let Ok((status, output)) = run_capture("brew", &["outdated", "--greedy", "--json=v2"]).await
    else {
        mark_check_failed(check_failed, "brew", "检查失败: 无法执行 brew 命令.", logs);
        return None;
    };
    if status != 0 {
        mark_check_failed(
            check_failed,
            "brew",
            &format!("检查失败 (brew outdated --greedy --json=v2, exit {status})."),
            logs,
        );
        return None;
    }
    let Some(json_text) = first_json_payload(&output) else {
        mark_check_failed(check_failed, "brew", "检查失败: 未找到 JSON 内容.", logs);
        return None;
    };
    let Ok(root) = serde_json::from_str::<Value>(json_text) else {
        mark_check_failed(check_failed, "brew", "检查失败: JSON 解析失败.", logs);
        return None;
    };
    Some(root)
}

fn brew_json_names(root: &Value, key: &str) -> Vec<String> {
    root.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn record_brew_updates(state: &mut AppState, logs: &mut Vec<String>) {
    record_bucket_items(
        &mut state.brew.has_updates,
        logs,
        "brew",
        &state.brew.formula_list,
        "Formula: 已是最新.",
        "Formula 可升级:",
    );
    record_bucket_items(
        &mut state.brew.has_updates,
        logs,
        "brew",
        &state.brew.cask_list,
        "Cask: 已是最新.",
        "Cask 可升级:",
    );
}

async fn check_npm_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.npm,
        &mut state.npm.installed,
        "npm",
        "npm",
        logs,
    )
    .await
    {
        return;
    }
    logs.push(log_pkg_line(
        "npm",
        "正在刷新 npm 元数据并检查全局包更新 (npm outdated --json --global --prefer-online)...",
        MsgKind::Info,
    ));
    let Some(items) = load_npm_outdated_items(&mut state.npm.check_failed, logs).await else {
        return;
    };
    state.npm.updatable_items = items;
    record_bucket_items(
        &mut state.npm.has_updates,
        logs,
        "npm",
        &state.npm.updatable_items,
        "全局包已是最新.",
        "以下全局包可升级:",
    );
}

async fn load_npm_outdated_items(
    check_failed: &mut bool,
    logs: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Ok((status, output)) = run_capture("npm", NPM_OUTDATED_ARGS).await else {
        mark_check_failed(check_failed, "npm", "检查失败: 无法执行 npm 命令.", logs);
        return None;
    };
    if status == 0 {
        return Some(Vec::new());
    }
    if status != 1 {
        mark_check_failed(
            check_failed,
            "npm",
            &format!("检查失败 (exit {status})."),
            logs,
        );
        return None;
    }
    let Some(json_text) = first_json_payload(&output) else {
        mark_check_failed(check_failed, "npm", "检查失败: JSON 解析失败.", logs);
        return None;
    };
    let Ok(root) = serde_json::from_str::<Value>(json_text) else {
        mark_check_failed(check_failed, "npm", "检查失败: JSON 解析失败.", logs);
        return None;
    };
    let Some(obj) = root.as_object() else {
        return Some(Vec::new());
    };
    Some(obj.keys().cloned().collect())
}

pub async fn check_cargo_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.cargo,
        &mut state.cargo.installed,
        "cargo",
        "cargo",
        logs,
    )
    .await
    {
        return;
    }
    if !command_exists("cargo-install-update").await {
        logs.push(log_pkg_line(
            "cargo",
            "未安装 cargo-install-update, 跳过.",
            MsgKind::Warn,
        ));
        return;
    }
    state.cargo.updater_installed = true;
    logs.push(log_pkg_line(
        "cargo",
        "正在检查已安装 crate 更新 (cargo install-update --locked --list)...",
        MsgKind::Info,
    ));
    let Some(pkgs) = load_cargo_update_list(&mut state.cargo.check_failed, logs).await else {
        return;
    };
    state.cargo.updatable_packages = pkgs;
    record_bucket_items(
        &mut state.cargo.has_updates,
        logs,
        "cargo",
        &state.cargo.updatable_packages,
        "已安装 crate 已是最新.",
        "以下 crate 可升级:",
    );
}

async fn load_cargo_update_list(
    check_failed: &mut bool,
    logs: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Ok((status, output)) = run_cargo_install_update_capture(&["--list"]).await else {
        mark_check_failed(check_failed, "cargo", "检查失败: 命令执行失败.", logs);
        return None;
    };
    if status != 0 {
        mark_check_failed(
            check_failed,
            "cargo",
            &format!("检查失败 (exit {status})."),
            logs,
        );
        return None;
    }
    let Ok(pkgs) = parse_cargo_list(&output) else {
        mark_check_failed(check_failed, "cargo", "检查失败: 输出解析失败.", logs);
        return None;
    };
    Some(pkgs)
}

async fn check_rustup_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.rustup,
        &mut state.rustup.installed,
        "rustup",
        "rustup",
        logs,
    )
    .await
    {
        return;
    }
    logs.push(log_pkg_line(
        "rustup",
        "正在检查 toolchain 更新 (rustup check --no-self-update)...",
        MsgKind::Info,
    ));
    let Some((status, output)) =
        load_rustup_check_output(&mut state.rustup.check_failed, logs).await
    else {
        return;
    };
    record_rustup_status(state, logs, status, &output);
    recommend_windows_gnu_toolchain(state, logs).await;
}

async fn load_rustup_check_output(
    check_failed: &mut bool,
    logs: &mut Vec<String>,
) -> Option<(i32, String)> {
    let Ok((status, output)) = run_capture("rustup", &["check", "--no-self-update"]).await else {
        mark_check_failed(
            check_failed,
            "rustup",
            "检查失败: 无法执行 rustup 命令.",
            logs,
        );
        return None;
    };
    Some((status, output))
}

fn is_identical_rustup_update(line: &str) -> bool {
    let lower = line.to_lowercase();
    if let Some(idx) = lower.find("update available:") {
        let versions = &line[idx + "update available:".len()..];
        if let Some(arrow_idx) = versions.find("->") {
            let left = versions[..arrow_idx].trim();
            let right = versions[arrow_idx + 2..].trim();
            return !left.is_empty() && left == right;
        }
    }
    false
}

fn record_rustup_status(state: &mut AppState, logs: &mut Vec<String>, status: i32, output: &str) {
    match status {
        0 => logs.push(log_pkg_line("rustup", "toolchain 已是最新.", MsgKind::Ok)),
        100 => {
            let mut updatable = Vec::new();
            for line in output.lines().map(str::trim).filter(|x| !x.is_empty()) {
                if !is_identical_rustup_update(line) {
                    updatable.push(line.to_string());
                }
            }

            if updatable.is_empty() {
                logs.push(log_pkg_line("rustup", "toolchain 已是最新.", MsgKind::Ok));
            } else {
                state.rustup.has_updates = true;
                logs.push(log_pkg_line(
                    "rustup",
                    "以下 toolchain 可升级:",
                    MsgKind::Info,
                ));
                for line in updatable {
                    state.rustup.updatable_items.push(line.clone());
                    logs.push(format!("  - {line}"));
                }
            }
        }
        _ => {
            mark_check_failed(
                &mut state.rustup.check_failed,
                "rustup",
                &format!("检查失败 (exit {status})."),
                logs,
            );
        }
    }
}

async fn recommend_windows_gnu_toolchain(state: &mut AppState, logs: &mut Vec<String>) {
    if state.system_profile != SystemProfile::Windows {
        return;
    }
    logs.push(log_pkg_line(
        "rustup",
        "正在检查 Windows GNU toolchain (rustup toolchain list)...",
        MsgKind::Info,
    ));
    let Ok((status, output)) = run_capture("rustup", &["toolchain", "list"]).await else {
        logs.push(log_pkg_line(
            "rustup",
            "建议检查失败: 无法执行 rustup toolchain list.",
            MsgKind::Warn,
        ));
        return;
    };
    if status != 0 {
        logs.push(log_pkg_line(
            "rustup",
            &format!("建议检查失败 (rustup toolchain list, exit {status})."),
            MsgKind::Warn,
        ));
        return;
    }
    if !rustup_toolchains_include_windows_gnu(&output) {
        logs.push(log_pkg_line(
            "rustup",
            "建议安装 GNU toolchain: rustup toolchain install stable-x86_64-pc-windows-gnu",
            MsgKind::Warn,
        ));
    }
}

fn rustup_toolchains_include_windows_gnu(output: &str) -> bool {
    output
        .lines()
        .map(str::trim)
        .any(|line| line.contains("windows-gnu"))
}

async fn check_scoop_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.scoop,
        &mut state.scoop.installed,
        "scoop",
        "scoop",
        logs,
    )
    .await
    {
        return;
    }
    if !refresh_scoop_metadata(&mut state.scoop.check_failed, logs).await {
        return;
    }
    logs.push(log_pkg_line(
        "scoop",
        "正在检查可升级项 (scoop export)...",
        MsgKind::Info,
    ));

    let Some(updatable_items) =
        check_scoop_outdated_items(&mut state.scoop.check_failed, logs).await
    else {
        return;
    };
    record_scoop_updates(state, logs, updatable_items);
}

async fn refresh_scoop_metadata(check_failed: &mut bool, logs: &mut Vec<String>) -> bool {
    logs.push(log_pkg_line(
        "scoop",
        "正在刷新 Scoop/桶元数据 (scoop update --quiet)...",
        MsgKind::Info,
    ));
    match run_capture("scoop", &["update", "--quiet"]).await {
        Ok((0, _)) => true,
        Ok((status, _)) => {
            mark_check_failed(
                check_failed,
                "scoop",
                &format!("元数据刷新失败 (scoop update --quiet, exit {status})."),
                logs,
            );
            false
        }
        Err(_) => {
            mark_check_failed(
                check_failed,
                "scoop",
                "元数据刷新失败: 无法执行 scoop update --quiet.",
                logs,
            );
            false
        }
    }
}

async fn check_scoop_outdated_items(
    check_failed: &mut bool,
    logs: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Ok((status, output)) = run_capture("scoop", &["export"]).await else {
        mark_check_failed(
            check_failed,
            "scoop",
            "检查失败: 无法执行 scoop export.",
            logs,
        );
        return None;
    };

    if status != 0 {
        mark_check_failed(
            check_failed,
            "scoop",
            &format!("检查失败 (scoop export, exit {status})."),
            logs,
        );
        return None;
    }

    let Ok(apps) = parse_scoop_export_output(&output) else {
        mark_check_failed(
            check_failed,
            "scoop",
            "检查失败: scoop export 输出解析失败.",
            logs,
        );
        return None;
    };

    let bucket_dirs = find_scoop_bucket_dirs().await;
    let mut outdated = Vec::new();

    for app in apps {
        if app.is_held() {
            continue;
        }
        if let Some(manifest_path) = find_manifest_for_app(&app, &bucket_dirs).await
            && let Some(latest_version) = read_manifest_version(&manifest_path).await
            && latest_version != app.version
        {
            outdated.push(app.name);
        }
    }

    Some(outdated)
}

async fn find_scoop_bucket_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut root_paths = Vec::new();

    if let Some(user_profile) = env::var_os("USERPROFILE") {
        let config_file = PathBuf::from(&user_profile)
            .join(".config")
            .join("scoop")
            .join("config.json");
        if let Ok(content) = fs::read_to_string(&config_file).await
            && let Ok(json) = serde_json::from_str::<Value>(&content)
        {
            if let Some(root) = json.get("root_path").and_then(Value::as_str) {
                root_paths.push(PathBuf::from(root));
            }
            if let Some(global) = json.get("global_path").and_then(Value::as_str) {
                root_paths.push(PathBuf::from(global));
            }
        }
        root_paths.push(PathBuf::from(user_profile).join("scoop"));
    }

    if let Some(scoop_env) = env::var_os("SCOOP") {
        root_paths.push(PathBuf::from(scoop_env));
    }
    if let Some(global_env) = env::var_os("SCOOP_GLOBAL") {
        root_paths.push(PathBuf::from(global_env));
    }
    root_paths.push(PathBuf::from(r"C:\ProgramData\scoop"));

    for root in root_paths {
        let buckets = root.join("buckets");
        if fs::metadata(&buckets).await.is_ok() && !dirs.contains(&buckets) {
            dirs.push(buckets);
        }
    }

    dirs
}

async fn find_manifest_for_app(app: &ScoopExportApp, bucket_dirs: &[PathBuf]) -> Option<PathBuf> {
    let file_name = format!("{}.json", app.name);

    let source_path = PathBuf::from(&app.source);
    if source_path.is_file() {
        return Some(source_path);
    }

    if !app.source.is_empty() {
        for b_dir in bucket_dirs {
            let candidate1 = b_dir.join(&app.source).join("bucket").join(&file_name);
            if fs::metadata(&candidate1).await.is_ok() {
                return Some(candidate1);
            }
            let candidate2 = b_dir.join(&app.source).join(&file_name);
            if fs::metadata(&candidate2).await.is_ok() {
                return Some(candidate2);
            }
        }
    }

    for b_dir in bucket_dirs {
        let mut entries = match fs::read_dir(b_dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let candidate1 = path.join("bucket").join(&file_name);
            if fs::metadata(&candidate1).await.is_ok() {
                return Some(candidate1);
            }
            let candidate2 = path.join(&file_name);
            if fs::metadata(&candidate2).await.is_ok() {
                return Some(candidate2);
            }
        }
    }

    None
}

async fn read_manifest_version(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).await.ok()?;
    let json = serde_json::from_str::<Value>(&content).ok()?;
    json.get("version")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn record_scoop_updates(
    state: &mut AppState,
    logs: &mut Vec<String>,
    updatable_items: Vec<String>,
) {
    state.scoop.updatable_items = updatable_items;

    if state.scoop.updatable_items.is_empty() {
        logs.push(log_pkg_line("scoop", "已是最新.", MsgKind::Ok));
    } else {
        state.scoop.has_updates = true;
        log_item_list(logs, "scoop", "以下包可升级:", &state.scoop.updatable_items);
    }
}

async fn check_paru_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.paru,
        &mut state.paru.installed,
        "paru",
        "paru",
        logs,
    )
    .await
    {
        return;
    }
    logs.push(log_pkg_line(
        "paru",
        "正在检查 AUR 可升级项 (paru -Qua)...",
        MsgKind::Info,
    ));
    let Some(output) = load_paru_updates(&mut state.paru.check_failed, logs).await else {
        return;
    };
    state.paru.updatable_items = package_names_from_lines(&output);
    record_bucket_items(
        &mut state.paru.has_updates,
        logs,
        "paru",
        &state.paru.updatable_items,
        "AUR 包已是最新.",
        "以下 AUR 包可升级:",
    );
}

async fn load_paru_updates(check_failed: &mut bool, logs: &mut Vec<String>) -> Option<String> {
    let Ok((status, output)) = run_capture("paru", &["-Qua"]).await else {
        mark_check_failed(check_failed, "paru", "检查失败: 无法执行 paru 命令.", logs);
        return None;
    };
    let trimmed_output = output.trim();
    if status != 0 {
        if status == 1 && trimmed_output.is_empty() {
            logs.push(log_pkg_line("paru", "AUR 包已是最新.", MsgKind::Ok));
            return None;
        }
        mark_check_failed(
            check_failed,
            "paru",
            &format!("检查失败 (exit {status})."),
            logs,
        );
        return None;
    }
    Some(trimmed_output.to_string())
}

async fn check_flatpak_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.flatpak,
        &mut state.flatpak.installed,
        "flatpak",
        "flatpak",
        logs,
    )
    .await
    {
        return;
    }
    logs.push(log_pkg_line(
        "flatpak",
        "正在检查可升级项 (flatpak remote-ls --updates --columns=application)...",
        MsgKind::Info,
    ));
    let Some(items) = load_flatpak_updates(&mut state.flatpak.check_failed, logs).await else {
        return;
    };
    state.flatpak.updatable_items = items;
    record_bucket_items(
        &mut state.flatpak.has_updates,
        logs,
        "flatpak",
        &state.flatpak.updatable_items,
        "已是最新.",
        "以下应用可升级:",
    );
}

async fn load_flatpak_updates(
    check_failed: &mut bool,
    logs: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Ok((status, output)) = run_capture(
        "flatpak",
        &["remote-ls", "--updates", "--columns=application"],
    )
    .await
    else {
        mark_check_failed(
            check_failed,
            "flatpak",
            "检查失败: 无法执行 flatpak 命令.",
            logs,
        );
        return None;
    };
    if status != 0 {
        mark_check_failed(
            check_failed,
            "flatpak",
            &format!("检查失败 (exit {status})."),
            logs,
        );
        return None;
    }
    Some(
        output
            .lines()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

async fn check_pacman_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.pacman,
        &mut state.pacman.installed,
        "pacman",
        "pacman",
        logs,
    )
    .await
    {
        return;
    }
    let use_checkupdates = command_exists("checkupdates").await;
    let (status, output) = load_pacman_updates(use_checkupdates).await;
    if status != 0 {
        record_pacman_nonzero(state, logs, status, &output, use_checkupdates);
        return;
    }
    state.pacman.updatable_items = package_names_from_lines(&output);
    record_bucket_items(
        &mut state.pacman.has_updates,
        logs,
        "pacman",
        &state.pacman.updatable_items,
        "已是最新.",
        "以下包可升级:",
    );
}

async fn check_dnf_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.dnf,
        &mut state.dnf.installed,
        "dnf",
        "dnf",
        logs,
    )
    .await
    {
        return;
    }

    logs.push(log_pkg_line(
        "dnf",
        "正在检查可升级项 (dnf check-update --quiet --json)...",
        MsgKind::Info,
    ));
    let output = match run_output("dnf", DNF_CHECK_ARGS).await {
        Ok(result) => result,
        Err(_) => {
            mark_check_failed(
                &mut state.dnf.check_failed,
                "dnf",
                "检查失败: 无法执行 dnf 命令.",
                logs,
            );
            return;
        }
    };

    let status = output.status.code().unwrap_or(-1);
    record_dnf_status(
        state,
        logs,
        status,
        &String::from_utf8_lossy(&output.stdout),
    );
    if state.dnf.check_failed {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            logs.push(log_pkg_line("dnf", stderr.trim(), MsgKind::Warn));
        }
    }
}

fn record_dnf_status(state: &mut AppState, logs: &mut Vec<String>, status: i32, output: &str) {
    if !dnf_status_is_success(status) {
        mark_check_failed(
            &mut state.dnf.check_failed,
            "dnf",
            &format!("检查失败 (dnf check-update --quiet --json, exit {status})."),
            logs,
        );
        return;
    }

    let Some(items) = dnf_updatable_names(output) else {
        mark_check_failed(
            &mut state.dnf.check_failed,
            "dnf",
            "检查失败: JSON 解析失败.",
            logs,
        );
        return;
    };
    // DNF5 can return 0 with available upgrades when JSON output is requested.
    state.dnf.updatable_items = items;
    record_bucket_items(
        &mut state.dnf.has_updates,
        logs,
        "dnf",
        &state.dnf.updatable_items,
        "已是最新.",
        "以下包可升级:",
    );
}

fn dnf_status_is_success(status: i32) -> bool {
    status == 0 || status == 100
}

fn dnf_updatable_names(output: &str) -> Option<Vec<String>> {
    let root = serde_json::from_str::<Value>(output).ok()?;
    let mut items = Vec::new();
    for section in root.as_object()?.values() {
        for package in section.as_array()? {
            let name = package.get("name")?.as_str()?;
            let arch = package.get("arch")?.as_str()?;
            if name.is_empty() || arch.is_empty() {
                return None;
            }
            let item = format!("{name}.{arch}");
            if !items.contains(&item) {
                items.push(item);
            }
        }
    }
    Some(items)
}

async fn load_pacman_updates(use_checkupdates: bool) -> (i32, String) {
    if use_checkupdates {
        run_capture("checkupdates", &[])
            .await
            .unwrap_or((-1, String::new()))
    } else {
        run_capture("pacman", &["-Qu"])
            .await
            .unwrap_or((-1, String::new()))
    }
}

fn record_pacman_nonzero(
    state: &mut AppState,
    logs: &mut Vec<String>,
    status: i32,
    output: &str,
    use_checkupdates: bool,
) {
    if pacman_status_means_no_updates(status, output, use_checkupdates) {
        logs.push(log_pkg_line("pacman", "已是最新.", MsgKind::Ok));
    } else {
        mark_check_failed(
            &mut state.pacman.check_failed,
            "pacman",
            &format!("检查失败 (exit {status})."),
            logs,
        );
    }
}

fn pacman_status_means_no_updates(status: i32, output: &str, use_checkupdates: bool) -> bool {
    if use_checkupdates {
        status == 2
    } else {
        status == 1 && output.trim().is_empty()
    }
}

async fn check_pkg_quiet(state: &mut AppState, logs: &mut Vec<String>) {
    if !ensure_check_target(
        state.enable.pkg,
        &mut state.pkg.installed,
        "pkg",
        "pkg",
        logs,
    )
    .await
    {
        return;
    }

    logs.push(log_pkg_line(
        "pkg",
        "正在检查可升级项 (apt list --upgradable)...",
        MsgKind::Info,
    ));
    let Some(items) = load_pkg_updates(&mut state.pkg.check_failed, logs).await else {
        return;
    };
    state.pkg.updatable_items = items;
    record_bucket_items(
        &mut state.pkg.has_updates,
        logs,
        "pkg",
        &state.pkg.updatable_items,
        "已是最新.",
        "以下包可升级:",
    );
}

async fn load_pkg_updates(check_failed: &mut bool, logs: &mut Vec<String>) -> Option<Vec<String>> {
    let Ok((status, output)) = run_capture("apt", &["list", "--upgradable"]).await else {
        mark_check_failed(check_failed, "pkg", "检查失败: 无法执行 apt 命令.", logs);
        return None;
    };
    if status != 0 {
        mark_check_failed(
            check_failed,
            "pkg",
            &format!("检查失败 (exit {status})."),
            logs,
        );
        return None;
    }
    Some(pkg_updatable_names(&output))
}

fn pkg_updatable_names(output: &str) -> Vec<String> {
    output.lines().filter_map(pkg_updatable_name).collect()
}

fn pkg_updatable_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if pkg_output_noise(trimmed) {
        return None;
    }
    let token = trimmed.split_whitespace().next().unwrap_or_default();
    pkg_name_from_token(token)
}

fn pkg_output_noise(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("WARNING:")
        || line.eq_ignore_ascii_case("listing...")
        || line.eq_ignore_ascii_case("listing")
        || line.eq_ignore_ascii_case("done")
}

fn pkg_name_from_token(token: &str) -> Option<String> {
    if token.is_empty() || !token.contains('/') {
        return None;
    }
    let name = token.split('/').next().unwrap_or(token);
    (!name.is_empty()).then(|| name.to_string())
}

pub async fn run_single_check(target: &str) -> CheckResult {
    let mut local = AppState::default();
    let mut logs = Vec::new();
    parse_profile(&mut local).await;
    if let Some(handler) = check_target_handler(target) {
        (handler.check)(&mut local, &mut logs).await;
    }
    CheckResult {
        target: target.to_string(),
        state: local,
        logs,
    }
}

pub fn merge_check_result(state: &mut AppState, target: &str, local: AppState) {
    if let Some(handler) = check_target_handler(target) {
        (handler.merge)(state, local);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dnf_status_is_success, dnf_updatable_names, ensure_check_target,
        is_identical_rustup_update, package_names_from_lines, pacman_status_means_no_updates,
        pkg_updatable_name, pkg_updatable_names, record_dnf_status, record_rustup_status,
        rustup_toolchains_include_windows_gnu,
    };

    #[test]
    fn identifies_identical_rustup_updates() {
        assert!(is_identical_rustup_update(
            "stable-x86_64-pc-windows-msvc - update available: 1.96.0 (ac68faa20 2026-05-25) -> 1.96.0 (ac68faa20 2026-05-25)"
        ));
        assert!(is_identical_rustup_update(
            "nightly-x86_64-pc-windows-msvc - update available: 1.98.0-nightly -> 1.98.0-nightly"
        ));
        assert!(!is_identical_rustup_update(
            "stable-x86_64-pc-windows-msvc - update available: 1.95.0 -> 1.96.0"
        ));
        assert!(!is_identical_rustup_update(
            "some unrelated line without update info"
        ));
    }

    #[test]
    fn record_rustup_status_filters_identical_updates() {
        use crate::state::AppState;
        let mut state = AppState::default();
        let mut logs = Vec::new();

        let output = "stable-x86_64-pc-windows-msvc - update available: 1.96.0 -> 1.96.0\n\
                      nightly-x86_64-pc-windows-msvc - update available: 1.95.0 -> 1.96.0\n";

        record_rustup_status(&mut state, &mut logs, 100, output);

        assert!(state.rustup.has_updates);
        assert_eq!(state.rustup.updatable_items.len(), 1);
        assert_eq!(
            state.rustup.updatable_items[0],
            "nightly-x86_64-pc-windows-msvc - update available: 1.95.0 -> 1.96.0"
        );

        // When all updates are identical:
        let mut state_all_identical = AppState::default();
        let mut logs_all_identical = Vec::new();
        let output_all_identical =
            "stable-x86_64-pc-windows-msvc - update available: 1.96.0 -> 1.96.0\n";

        record_rustup_status(
            &mut state_all_identical,
            &mut logs_all_identical,
            100,
            output_all_identical,
        );

        assert!(!state_all_identical.rustup.has_updates);
        assert!(state_all_identical.rustup.updatable_items.is_empty());
    }

    #[test]
    fn detects_windows_gnu_toolchain() {
        let output = "stable-x86_64-pc-windows-msvc (default)\nstable-x86_64-pc-windows-gnu\n";

        assert!(rustup_toolchains_include_windows_gnu(output));
    }

    #[test]
    fn reports_missing_windows_gnu_toolchain() {
        let output = "stable-x86_64-pc-windows-msvc (default)\nnightly-x86_64-pc-windows-msvc\n";

        assert!(!rustup_toolchains_include_windows_gnu(output));
    }

    #[test]
    fn extracts_package_names_from_first_column() {
        let output = "git 2.0 -> 2.1\n\nnodejs 20.0 -> 22.0\n";

        assert_eq!(
            package_names_from_lines(output),
            vec!["git".to_string(), "nodejs".to_string()]
        );
    }

    #[test]
    fn recognizes_pacman_no_update_statuses() {
        assert!(pacman_status_means_no_updates(2, "", true));
        assert!(pacman_status_means_no_updates(1, "", false));
        assert!(!pacman_status_means_no_updates(1, "pacman 1 -> 2", false));
        assert!(!pacman_status_means_no_updates(0, "", true));
    }

    #[test]
    fn recognizes_dnf_check_update_exit_codes() {
        assert!(dnf_status_is_success(0));
        assert!(dnf_status_is_success(100));
        assert!(!dnf_status_is_success(1));
        assert!(!dnf_status_is_success(200));
    }

    #[test]
    fn parses_dnf_json_package_names() {
        let output = r#"{
            "upgrades": [
                {"name": "chatgpt", "arch": "aarch64", "evr": "26.1021.22027-1", "repository": "openai-chatgpt"},
                {"name": "vim-enhanced", "arch": "aarch64"}
            ]
        }"#;

        assert_eq!(
            dnf_updatable_names(output),
            Some(vec![
                "chatgpt.aarch64".into(),
                "vim-enhanced.aarch64".into()
            ])
        );
    }

    #[test]
    fn dnf_json_sections_are_not_packages_and_duplicates_are_removed() {
        let output = r#"{
            "upgrades": [{"name": "kernel", "arch": "aarch64"}],
            "obsoleting": [{"name": "kernel", "arch": "aarch64",
                "obsoletes": [{"name": "old-kernel", "arch": "aarch64"}]}]
        }"#;

        assert_eq!(
            dnf_updatable_names(output),
            Some(vec!["kernel.aarch64".into()])
        );
    }

    #[test]
    fn dnf_json_reports_updates_even_with_exit_zero() {
        use crate::state::AppState;
        for status in [0, 100] {
            let mut state = AppState::default();
            let mut logs = Vec::new();
            record_dnf_status(
                &mut state,
                &mut logs,
                status,
                r#"{"upgrades":[{"name":"chatgpt","arch":"aarch64"}]}"#,
            );

            assert!(state.dnf.has_updates);
            assert!(!state.dnf.check_failed);
            assert_eq!(state.dnf.updatable_items, vec!["chatgpt.aarch64"]);
        }
    }

    #[test]
    fn empty_dnf_json_means_no_updates() {
        use crate::state::AppState;
        for output in ["{}", r#"{"upgrades":[]}"#] {
            let mut state = AppState::default();
            let mut logs = Vec::new();
            record_dnf_status(&mut state, &mut logs, 0, output);

            assert!(!state.dnf.has_updates);
            assert!(!state.dnf.check_failed);
            assert!(state.dnf.updatable_items.is_empty());
        }
    }

    #[test]
    fn invalid_dnf_json_is_a_check_failure() {
        use crate::state::AppState;
        for output in [
            "Upgrades\nchatgpt.aarch64 26.1021.22027-1 openai-chatgpt\n",
            "{",
            "[]",
            r#"{"upgrades":{}}"#,
            r#"{"upgrades":[{"name":"chatgpt"}]}"#,
            r#"{"upgrades":[{"name":"","arch":"aarch64"}]}"#,
        ] {
            let mut state = AppState::default();
            let mut logs = Vec::new();
            record_dnf_status(&mut state, &mut logs, 0, output);

            assert!(state.dnf.check_failed, "accepted invalid output: {output}");
            assert!(!state.dnf.has_updates);
            assert!(!logs.iter().any(|line| line.contains("已是最新")));
        }
    }

    #[test]
    fn dnf_command_errors_are_check_failures() {
        use crate::state::AppState;
        for status in [1, 200, -1] {
            let mut state = AppState::default();
            let mut logs = Vec::new();
            record_dnf_status(&mut state, &mut logs, status, "{}");

            assert!(state.dnf.check_failed);
            assert!(!state.dnf.has_updates);
        }
    }

    #[tokio::test]
    async fn missing_dnf_command_is_skipped_cleanly() {
        let mut installed = false;
        let mut logs = Vec::new();

        let checked = ensure_check_target(
            true,
            &mut installed,
            "__updt_test_missing_dnf_command__",
            "dnf",
            &mut logs,
        )
        .await;

        assert!(!checked);
        assert!(!installed);
        assert!(logs.iter().any(|line| line.contains("未安装, 跳过.")));
    }

    #[test]
    fn parses_pkg_updatable_names_and_ignores_noise() {
        let output = "Listing...\nlibc/stable 1.0 aarch64 [upgradable from: 0.9]\nWARNING: apt does not have a stable CLI interface.\nvim/now 9.0 aarch64 [upgradable from: 8.0]\n";

        assert_eq!(
            pkg_updatable_names(output),
            vec!["libc".to_string(), "vim".to_string()]
        );
        assert_eq!(pkg_updatable_name("not-a-package"), None);
    }
}
