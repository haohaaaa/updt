use crate::command::{
    command_exists, run_capture, run_capture_streaming, run_cargo_install_update_inherit,
    run_inherit,
};
use crate::output::{err_text, ok_text, print_section, warn_text};
#[cfg(windows)]
use crate::parse::strip_ansi_control_sequences;
use crate::parse::{
    ScoopBlockedProcessInfo, ScoopExportApp, parse_scoop_blocked_process_output,
    parse_scoop_export_output,
};
use crate::profile::{desktop_linux_session, interactive_terminal};
use crate::selection::{ScoopBlockedAction, prompt_scoop_blocked_action};
use crate::state::{AppState, target_label};
use std::collections::{HashMap, HashSet};
use std::{env, future::Future, io, pin::Pin, process};
use tokio::fs;

use std::io::Write as _;
#[cfg(windows)]
use std::process::{Command, Stdio};

const NPM_UPDATE_ARGS: &[&str] = &["update", "-g", "--prefer-online"];

pub async fn upgrade_selected(state: &AppState, selected: &[String]) -> bool {
    print_section("执行升级");
    let mut failures = UpgradeFailures::default();
    let self_pkg = env!("CARGO_PKG_NAME");
    let pacman_plan = PacmanUpgradePlan::from_selected(state, selected);

    if !run_pacman_before_standard_targets(state, pacman_plan).await {
        failures.record_target("pacman");
    }
    let mut standard_outcome =
        run_selected_standard_targets(PRE_PACMAN_STANDARD_TARGETS, state, selected, self_pkg).await;
    failures.merge(standard_outcome.failures.clone());
    if !run_pacman_after_standard_targets(state, pacman_plan).await {
        failures.record_target("pacman");
    }
    let post_pacman_outcome =
        run_selected_standard_targets(POST_PACMAN_STANDARD_TARGETS, state, selected, self_pkg)
            .await;
    failures.merge(post_pacman_outcome.failures.clone());
    standard_outcome.merge(post_pacman_outcome);
    if !upgrade_cargo_self_if_needed(self_pkg, standard_outcome.cargo_self_needs_update).await {
        failures.record_label("cargo (updt 自身)");
    }

    print_upgrade_summary(selected, &failures)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct UpgradeFailures {
    labels: Vec<String>,
}

impl UpgradeFailures {
    fn record_target(&mut self, id: &str) {
        self.record_label(target_label(id));
    }

    fn record_label(&mut self, label: &str) {
        if !self.labels.iter().any(|item| item == label) {
            self.labels.push(label.to_string());
        }
    }

    fn merge(&mut self, other: Self) {
        for label in other.labels {
            self.record_label(&label);
        }
    }

    fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

fn target_selected(selected: &[String], target: &str) -> bool {
    selected.iter().any(|id| id == target)
}

#[derive(Clone, Copy, Debug)]
struct PacmanUpgradePlan {
    selected: bool,
    run_before_standard_targets: bool,
}

impl PacmanUpgradePlan {
    fn from_selected(state: &AppState, selected: &[String]) -> Self {
        let selected = target_selected(selected, "pacman");
        Self {
            selected,
            run_before_standard_targets: state.is_arch_linux && selected,
        }
    }

    fn should_run(self, before_standard_targets: bool) -> bool {
        self.selected && self.run_before_standard_targets == before_standard_targets
    }
}

async fn run_pacman_before_standard_targets(state: &AppState, plan: PacmanUpgradePlan) -> bool {
    run_pacman_at_position(state, plan, true).await
}

async fn run_pacman_after_standard_targets(state: &AppState, plan: PacmanUpgradePlan) -> bool {
    run_pacman_at_position(state, plan, false).await
}

async fn run_pacman_at_position(
    state: &AppState,
    plan: PacmanUpgradePlan,
    before_standard_targets: bool,
) -> bool {
    if plan.should_run(before_standard_targets) {
        run_pacman_upgrade(state).await
    } else {
        true
    }
}

#[derive(Clone, Copy)]
struct StandardUpgradeTarget {
    id: &'static str,
    run: StandardUpgradeFn,
}

type StandardUpgradeFuture<'a> = Pin<Box<dyn Future<Output = StandardUpgradeOutcome> + 'a>>;
type StandardUpgradeFn = for<'a> fn(&'a AppState, &'a str) -> StandardUpgradeFuture<'a>;
const RUSTUP_UPDATE_ARGS: &[&str] = &["update", "--no-self-update"];

const PRE_PACMAN_STANDARD_TARGETS: &[StandardUpgradeTarget] = &[
    StandardUpgradeTarget {
        id: "brew",
        run: run_brew_target,
    },
    StandardUpgradeTarget {
        id: "npm",
        run: run_npm_target,
    },
    StandardUpgradeTarget {
        id: "cargo",
        run: run_cargo_target,
    },
    StandardUpgradeTarget {
        id: "rustup",
        run: run_rustup_target,
    },
    StandardUpgradeTarget {
        id: "scoop",
        run: run_scoop_target,
    },
    StandardUpgradeTarget {
        id: "paru",
        run: run_paru_target,
    },
    StandardUpgradeTarget {
        id: "flatpak",
        run: run_flatpak_target,
    },
];

const POST_PACMAN_STANDARD_TARGETS: &[StandardUpgradeTarget] = &[StandardUpgradeTarget {
    id: "pkg",
    run: run_pkg_target,
}];

#[derive(Clone, Debug, Default)]
struct StandardUpgradeOutcome {
    failed: bool,
    failures: UpgradeFailures,
    cargo_self_needs_update: bool,
}

impl StandardUpgradeOutcome {
    fn from_success(success: bool) -> Self {
        Self {
            failed: !success,
            failures: UpgradeFailures::default(),
            cargo_self_needs_update: false,
        }
    }

    fn from_cargo(outcome: CargoUpgradeOutcome) -> Self {
        Self {
            failed: outcome.failed,
            failures: UpgradeFailures::default(),
            cargo_self_needs_update: outcome.self_needs_update,
        }
    }

    fn merge(&mut self, outcome: Self) {
        self.failed |= outcome.failed;
        self.failures.merge(outcome.failures);
        self.cargo_self_needs_update |= outcome.cargo_self_needs_update;
    }
}

async fn run_selected_standard_targets(
    targets: &'static [StandardUpgradeTarget],
    state: &AppState,
    selected: &[String],
    self_pkg: &str,
) -> StandardUpgradeOutcome {
    let mut outcome = StandardUpgradeOutcome::default();
    for target in selected_standard_targets(targets, selected) {
        let target_outcome = (target.run)(state, self_pkg).await;
        if target_outcome.failed {
            outcome.failures.record_target(target.id);
        }
        outcome.merge(target_outcome);
    }
    outcome
}

fn selected_standard_targets<'a>(
    targets: &'static [StandardUpgradeTarget],
    selected: &'a [String],
) -> impl Iterator<Item = &'static StandardUpgradeTarget> + 'a {
    targets
        .iter()
        .filter(move |target| target_selected(selected, target.id))
}

fn run_brew_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_brew().await) })
}

fn run_npm_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_npm().await) })
}

fn run_cargo_target<'a>(state: &'a AppState, self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async move {
        StandardUpgradeOutcome::from_cargo(upgrade_cargo_packages(state, self_pkg).await)
    })
}

fn run_rustup_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_rustup().await) })
}

fn run_scoop_target<'a>(state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(
        async move { StandardUpgradeOutcome::from_success(upgrade_scoop_packages(state).await) },
    )
}

fn run_paru_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_paru().await) })
}

fn run_flatpak_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_flatpak().await) })
}

fn run_pkg_target<'a>(_state: &'a AppState, _self_pkg: &'a str) -> StandardUpgradeFuture<'a> {
    Box::pin(async { StandardUpgradeOutcome::from_success(upgrade_pkg().await) })
}

async fn upgrade_cargo_self_if_needed(self_pkg: &str, cargo_self_needs_update: bool) -> bool {
    if cargo_self_needs_update {
        upgrade_cargo_self(self_pkg).await
    } else {
        true
    }
}

async fn upgrade_brew() -> bool {
    println!("[brew] 正在刷新索引: brew update --quiet");
    match run_inherit("brew", &["update", "--quiet"]).await {
        Ok(true) => {}
        _ => {
            println!("[brew] 升级失败: brew update 失败.");
            return false;
        }
    }

    println!("[brew] 正在执行: brew upgrade --greedy");
    match run_inherit("brew", &["upgrade", "--greedy"]).await {
        Ok(true) => {
            println!("[brew] 升级完成.");
            true
        }
        _ => {
            println!("[brew] 升级失败.");
            false
        }
    }
}

async fn upgrade_npm() -> bool {
    run_logged_inherit(
        "npm",
        "npm",
        NPM_UPDATE_ARGS,
        "npm update -g --prefer-online",
        "[npm] 全局包升级完成.",
        "[npm] 全局包升级失败.",
    )
    .await
}

struct CargoUpgradeOutcome {
    failed: bool,
    self_needs_update: bool,
}

async fn upgrade_cargo_packages(state: &AppState, self_pkg: &str) -> CargoUpgradeOutcome {
    let self_needs_update = state
        .cargo
        .updatable_packages
        .iter()
        .any(|pkg| pkg.as_str() == self_pkg);
    let targets = cargo_packages_excluding_self(state, self_pkg);

    if targets.is_empty() {
        if self_needs_update {
            println!("[cargo] 检测到 updt 自身可升级, 将在最后单独升级.");
        } else {
            println!("[cargo] 无可升级 crate, 跳过.");
        }
        return CargoUpgradeOutcome {
            failed: false,
            self_needs_update,
        };
    }

    let failed = !upgrade_cargo_package_targets(&targets).await;
    if self_needs_update {
        println!("[cargo] updt 自身将放到最后单独升级.");
    }

    CargoUpgradeOutcome {
        failed,
        self_needs_update,
    }
}

fn cargo_packages_excluding_self(state: &AppState, self_pkg: &str) -> Vec<String> {
    state
        .cargo
        .updatable_packages
        .iter()
        .filter(|pkg| pkg.as_str() != self_pkg)
        .cloned()
        .collect()
}

async fn upgrade_cargo_package_targets(targets: &[String]) -> bool {
    let mut args = Vec::with_capacity(targets.len());
    for pkg in targets {
        args.push(pkg.as_str());
    }
    println!(
        "[cargo] 正在执行: cargo install-update --locked {}",
        targets.join(" ")
    );
    match run_cargo_install_update_inherit(&args).await {
        Ok(true) => {
            println!("[cargo] 其他已安装 crate 升级完成.");
            true
        }
        _ => {
            println!("[cargo] 已安装 crate 升级失败.");
            false
        }
    }
}

async fn upgrade_rustup() -> bool {
    run_logged_inherit(
        "rustup",
        "rustup",
        RUSTUP_UPDATE_ARGS,
        "rustup update --no-self-update",
        "[rustup] toolchain 升级完成.",
        "[rustup] toolchain 升级失败.",
    )
    .await
}

async fn upgrade_paru() -> bool {
    notify_paru_password_attention_if_needed().await;
    run_logged_inherit(
        "paru",
        "paru",
        &["-Sua"],
        "paru -Sua",
        "[paru] AUR 包升级完成.",
        "[paru] AUR 包升级失败.",
    )
    .await
}

async fn upgrade_flatpak() -> bool {
    run_logged_inherit(
        "flatpak",
        "flatpak",
        &["update"],
        "flatpak update",
        "[flatpak] 应用升级完成.",
        "[flatpak] 应用升级失败.",
    )
    .await
}

async fn upgrade_pkg() -> bool {
    println!("[pkg] 正在执行: pkg update");
    match run_inherit("pkg", &["update"]).await {
        Ok(true) => {}
        _ => {
            println!("[pkg] 升级失败: pkg update 失败.");
            return false;
        }
    }

    println!("[pkg] 正在执行: pkg upgrade");
    match run_inherit("pkg", &["upgrade"]).await {
        Ok(true) => {
            println!("[pkg] 包升级完成.");
            true
        }
        _ => {
            println!("[pkg] 包升级失败.");
            false
        }
    }
}

#[cfg(windows)]
async fn upgrade_cargo_self(self_pkg: &str) -> bool {
    println!(
        "[cargo] 即将单独升级 updt: 先退出当前 updt, 再执行 cargo install-update --locked updt"
    );
    match schedule_windows_self_update(self_pkg).await {
        Ok(()) => {
            println!("[cargo] 已启动前台自更新窗口, 本次 updt 退出后会显示升级过程.");
            true
        }
        Err(err) => {
            println!("[cargo] 启动前台自更新窗口失败: {err}");
            println!("[cargo] 可手动执行: cargo install-update --locked updt");
            false
        }
    }
}

#[cfg(not(windows))]
async fn upgrade_cargo_self(self_pkg: &str) -> bool {
    println!("[cargo] 正在执行: cargo install-update --locked updt");
    match run_cargo_install_update_inherit(&[self_pkg]).await {
        Ok(true) => {
            println!("[cargo] updt 自身升级完成.");
            true
        }
        _ => {
            println!("[cargo] updt 自身升级失败.");
            false
        }
    }
}

async fn run_logged_inherit(
    prefix: &str,
    program: &str,
    args: &[&str],
    command_label: &str,
    success_message: &str,
    failure_message: &str,
) -> bool {
    println!("[{prefix}] 正在执行: {command_label}");
    match run_inherit(program, args).await {
        Ok(true) => {
            println!("{success_message}");
            true
        }
        _ => {
            println!("{failure_message}");
            false
        }
    }
}

fn print_upgrade_summary(selected: &[String], failures: &UpgradeFailures) -> bool {
    print_section("汇总");
    println!(
        "已选择升级项: {}",
        selected
            .iter()
            .map(|id| target_label(id))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !failures.is_empty() {
        println!("{}", err_text("存在升级失败项."));
        println!("失败升级项:");
        for label in &failures.labels {
            println!("  - {label}");
        }
        return false;
    }
    println!("{}", ok_text("所有已选升级项执行完成."));
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ScoopInstallScope {
    Local,
    Global,
}

impl ScoopInstallScope {
    fn suffix(self) -> &'static str {
        match self {
            Self::Local => "",
            Self::Global => " (global)",
        }
    }

    fn update_args(self, app: &str) -> Vec<String> {
        let mut args = vec!["update".to_string(), app.to_string()];
        if matches!(self, Self::Global) {
            args.push("--global".to_string());
        }
        args
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScoopUpdateTask {
    app: String,
    scope: ScoopInstallScope,
}

impl ScoopUpdateTask {
    fn display_name(&self) -> String {
        format!("{}{}", self.app, self.scope.suffix())
    }

    fn update_args(&self) -> Vec<String> {
        self.scope.update_args(&self.app)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScoopTaskOutcome {
    Updated,
    SkippedInUse,
    FailedOther,
    Aborted,
}

fn build_scoop_update_tasks(
    updatable_items: &[String],
    installed_items: &[ScoopExportApp],
) -> Vec<ScoopUpdateTask> {
    let mut scopes_by_name = HashMap::<&str, Vec<ScoopInstallScope>>::new();
    for item in installed_items {
        let scope = if item.is_global() {
            ScoopInstallScope::Global
        } else {
            ScoopInstallScope::Local
        };
        scopes_by_name
            .entry(item.name.as_str())
            .or_default()
            .push(scope);
    }

    let mut tasks = Vec::new();
    let mut seen = HashSet::<(String, ScoopInstallScope)>::new();
    let mut handled_apps = HashSet::<String>::new();

    for app in updatable_items {
        if !handled_apps.insert(app.clone()) {
            continue;
        }

        let scopes = scopes_by_name
            .get(app.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[ScoopInstallScope::Local]);

        for scope in [ScoopInstallScope::Local, ScoopInstallScope::Global] {
            if !scopes.contains(&scope) {
                continue;
            }
            if seen.insert((app.clone(), scope)) {
                tasks.push(ScoopUpdateTask {
                    app: app.clone(),
                    scope,
                });
            }
        }
    }

    tasks
}

async fn upgrade_scoop_packages(state: &AppState) -> bool {
    let Some(tasks) = scoop_update_tasks_for_state(state).await else {
        return false;
    };

    let outcome = run_scoop_update_batch(tasks, interactive_terminal()).await;
    outcome.print_summary();
    outcome.success()
}

async fn scoop_update_tasks_for_state(state: &AppState) -> Option<Vec<ScoopUpdateTask>> {
    let installed_items = match load_scoop_installed_items().await {
        Ok(items) => items,
        Err(err) => {
            println!(
                "{}",
                warn_text(&format!(
                    "[scoop] 无法获取已安装包详细清单 ({err}), 默认按用户范围执行升级."
                ))
            );
            Vec::new()
        }
    };
    let tasks = build_scoop_update_tasks(&state.scoop.updatable_items, &installed_items);
    if tasks.is_empty() {
        println!("[scoop] 未解析到待升级包任务.");
        None
    } else {
        Some(tasks)
    }
}

#[derive(Default)]
struct ScoopBatchOutcome {
    updated: Vec<String>,
    skipped_in_use: Vec<String>,
    failed_other: Vec<String>,
    aborted: bool,
}

impl ScoopBatchOutcome {
    fn record(&mut self, task: &ScoopUpdateTask, outcome: ScoopTaskOutcome) {
        match outcome {
            ScoopTaskOutcome::Updated => self.updated.push(task.display_name()),
            ScoopTaskOutcome::SkippedInUse => self.skipped_in_use.push(task.display_name()),
            ScoopTaskOutcome::FailedOther => self.failed_other.push(task.display_name()),
            ScoopTaskOutcome::Aborted => self.aborted = true,
        }
    }

    fn print_summary(&self) {
        print_nonempty_scoop_list(
            !self.updated.is_empty(),
            &format!("[scoop] 已更新 {} 个包.", self.updated.len()),
            &[],
        );
        print_nonempty_scoop_list(
            !self.skipped_in_use.is_empty(),
            "[scoop] 以下包因运行中进程未完成更新:",
            &self.skipped_in_use,
        );
        print_nonempty_scoop_list(
            !self.failed_other.is_empty(),
            "[scoop] 以下包更新失败:",
            &self.failed_other,
        );
        if self.aborted {
            println!("[scoop] 用户中止了后续 Scoop 包更新.");
        }
    }

    fn success(&self) -> bool {
        !self.aborted && self.skipped_in_use.is_empty() && self.failed_other.is_empty()
    }
}

async fn run_scoop_update_batch(
    tasks: Vec<ScoopUpdateTask>,
    allow_prompt: bool,
) -> ScoopBatchOutcome {
    let mut batch = ScoopBatchOutcome::default();
    for task in tasks {
        let outcome = run_scoop_update_task(&task, allow_prompt).await;
        batch.record(&task, outcome);
        if batch.aborted {
            break;
        }
    }
    batch
}

fn print_nonempty_scoop_list(should_print: bool, header: &str, items: &[String]) {
    if should_print {
        println!("{header}");
        for item in items {
            println!("  - {item}");
        }
    }
}

async fn load_scoop_installed_items() -> io::Result<Vec<ScoopExportApp>> {
    let (status, output) = run_capture("scoop", &["export"]).await?;
    if status != 0 {
        return Err(io::Error::other(format!(
            "scoop export 失败 (exit {status})"
        )));
    }
    parse_scoop_export_output(&output).map_err(|_| io::Error::other("scoop export 输出解析失败"))
}

async fn run_scoop_update_task(task: &ScoopUpdateTask, allow_prompt: bool) -> ScoopTaskOutcome {
    loop {
        let Some((status, output)) = run_scoop_update_attempt(task).await else {
            return ScoopTaskOutcome::FailedOther;
        };

        if let Some(blocked) = parse_scoop_blocked_process_output(&output) {
            match handle_scoop_blocked_update(task, &blocked, allow_prompt).await {
                ScoopBlockedUpdate::Retry => continue,
                ScoopBlockedUpdate::Done(outcome) => return outcome,
            }
        }

        return scoop_status_outcome(task, status);
    }
}

async fn run_scoop_update_attempt(task: &ScoopUpdateTask) -> Option<(i32, String)> {
    let args = task.update_args();
    println!("[scoop] 正在执行: {}", scoop_command_label(&args));
    match run_scoop_capture(&args).await {
        Ok(result) => Some(result),
        Err(err) => {
            println!("[scoop] {} 更新失败: {err}", task.display_name());
            None
        }
    }
}

enum ScoopBlockedUpdate {
    Retry,
    Done(ScoopTaskOutcome),
}

async fn handle_scoop_blocked_update(
    task: &ScoopUpdateTask,
    blocked: &ScoopBlockedProcessInfo,
    allow_prompt: bool,
) -> ScoopBlockedUpdate {
    if !allow_prompt {
        println!("[scoop] {} 因运行中进程被跳过.", task.display_name());
        return ScoopBlockedUpdate::Done(ScoopTaskOutcome::SkippedInUse);
    }

    notify_scoop_blocked(task, blocked).await;
    match prompt_scoop_blocked_action(&task.display_name(), &blocked.details).await {
        ScoopBlockedAction::KillAndRetry => retry_scoop_blocked_update(task, blocked).await,
        ScoopBlockedAction::Skip => ScoopBlockedUpdate::Done(ScoopTaskOutcome::SkippedInUse),
        ScoopBlockedAction::Abort => ScoopBlockedUpdate::Done(ScoopTaskOutcome::Aborted),
    }
}

async fn retry_scoop_blocked_update(
    task: &ScoopUpdateTask,
    blocked: &ScoopBlockedProcessInfo,
) -> ScoopBlockedUpdate {
    if let Err(err) = kill_scoop_task_processes(task, blocked).await {
        println!("[scoop] 结束 {} 关联进程失败: {err}", task.display_name());
    }
    println!("[scoop] 正在重试 {}...", task.display_name());
    ScoopBlockedUpdate::Retry
}

fn scoop_status_outcome(task: &ScoopUpdateTask, status: i32) -> ScoopTaskOutcome {
    if status == 0 {
        ScoopTaskOutcome::Updated
    } else {
        println!("[scoop] {} 更新失败 (exit {status}).", task.display_name());
        ScoopTaskOutcome::FailedOther
    }
}

fn scoop_command_label(args: &[String]) -> String {
    #[cfg(windows)]
    let program = "gsudo scoop";
    #[cfg(not(windows))]
    let program = "scoop";

    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

#[cfg(windows)]
fn print_captured_command_output(output: &str) {
    if output.is_empty() {
        return;
    }
    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
}

async fn run_scoop_capture(args: &[String]) -> io::Result<(i32, String)> {
    #[cfg(windows)]
    {
        if !command_exists("gsudo").await {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Windows Scoop 更新需要 gsudo, 但未找到 gsudo",
            ));
        }

        let mut elevated_args = Vec::with_capacity(args.len() + 1);
        elevated_args.push("scoop".to_string());
        elevated_args.extend_from_slice(args);
        run_capture_streaming("gsudo", &borrow_string_args(&elevated_args)).await
    }

    #[cfg(not(windows))]
    {
        run_capture_streaming("scoop", &borrow_string_args(args)).await
    }
}

fn borrow_string_args(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[cfg(windows)]
async fn kill_scoop_task_processes(
    task: &ScoopUpdateTask,
    _blocked: &ScoopBlockedProcessInfo,
) -> io::Result<()> {
    let app_dir = resolve_scoop_app_dir(task).await?;
    let shell = if command_exists("pwsh").await {
        "pwsh"
    } else {
        "powershell.exe"
    };
    let app_dir_literal = ps_single_quote(&app_dir);
    let script = format!(
        "$ErrorActionPreference='Stop'; \
$appDir = Convert-Path '{app_dir_literal}'; \
$prefix = $appDir.TrimEnd('\\') + '\\'; \
$targets = @(Get-Process | Where-Object {{ \
    $_.Path -and \
    ([System.IO.Path]::GetFullPath($_.Path)).StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) \
}} | Sort-Object Id -Unique); \
if ($targets.Count -eq 0) {{ \
    Write-Output 'No matching processes found.'; \
    exit 0; \
}}; \
$targets | ForEach-Object {{ Write-Output ('PID {{0}}: {{1}}' -f $_.Id, $_.Path) }}; \
$targets | Stop-Process -Force -ErrorAction Stop; \
Write-Output ('Stopped {{0}} process(es).' -f $targets.Count)"
    );
    let args = vec![
        shell.to_string(),
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-Command".to_string(),
        script,
    ];

    let (status, output) = run_capture("gsudo", &borrow_string_args(&args)).await?;
    print_captured_command_output(&output);
    if status != 0 {
        return Err(io::Error::other(format!("结束进程失败 (exit {status})")));
    }
    Ok(())
}

#[cfg(not(windows))]
async fn kill_scoop_task_processes(
    _task: &ScoopUpdateTask,
    _blocked: &ScoopBlockedProcessInfo,
) -> io::Result<()> {
    Err(io::Error::other("仅支持 Windows Scoop 进程恢复"))
}

#[cfg(windows)]
async fn resolve_scoop_app_dir(task: &ScoopUpdateTask) -> io::Result<String> {
    let scoop_core_dir = scoop_core_dir().await?;
    let shell = scoop_resolver_shell().await;
    let script = scoop_app_dir_script(task, &scoop_core_dir);
    let args = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ];
    let (status, output) = run_capture(shell, &args).await?;
    if status != 0 {
        return Err(io::Error::other(format!(
            "无法解析 {} 安装目录 (exit {status})",
            task.display_name()
        )));
    }

    first_nonempty_output_line(&output)
        .ok_or_else(|| io::Error::other(format!("无法解析 {} 安装目录", task.display_name())))
}

#[cfg(windows)]
async fn scoop_core_dir() -> io::Result<String> {
    let (status, output) = run_capture("scoop", &["prefix", "scoop"]).await?;
    ensure_scoop_prefix_success(status)?;
    first_nonempty_output_line(&output).ok_or_else(|| io::Error::other("未找到 Scoop core 目录"))
}

#[cfg(windows)]
fn ensure_scoop_prefix_success(status: i32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "scoop prefix scoop 失败 (exit {status})"
        )))
    }
}

#[cfg(windows)]
async fn scoop_resolver_shell() -> &'static str {
    if command_exists("pwsh").await {
        "pwsh"
    } else {
        "powershell.exe"
    }
}

#[cfg(windows)]
fn scoop_app_dir_script(task: &ScoopUpdateTask, scoop_core_dir: &str) -> String {
    let scoop_core_literal = ps_single_quote(scoop_core_dir);
    let app_literal = ps_single_quote(&task.app);
    let global = scoop_scope_bool_literal(task.scope);
    format!(
        "$ErrorActionPreference='Stop'; \
. '{scoop_core_literal}\\lib\\core.ps1'; \
. '{scoop_core_literal}\\lib\\versions.ps1'; \
$path = appdir '{app_literal}' ${global}; \
if (Test-Path $path) {{ Convert-Path $path }} else {{ exit 2 }}"
    )
}

#[cfg(windows)]
fn scoop_scope_bool_literal(scope: ScoopInstallScope) -> &'static str {
    if matches!(scope, ScoopInstallScope::Global) {
        "true"
    } else {
        "false"
    }
}

#[cfg(windows)]
fn first_nonempty_output_line(output: &str) -> Option<String> {
    strip_ansi_control_sequences(output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(windows)]
fn ps_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(windows)]
async fn notify_scoop_blocked(task: &ScoopUpdateTask, blocked: &ScoopBlockedProcessInfo) {
    ring_terminal_bell();

    let title = "updt";
    let body = if let Some(app_name) = &blocked.app_name {
        format!(
            "{app_name}{} is still running. Return to the terminal to choose Kill, Skip, or Abort.",
            task.scope.suffix()
        )
    } else {
        format!(
            "{} is waiting for your decision in the terminal.",
            task.display_name()
        )
    };

    let _ = spawn_windows_message_box(title, &body).await;
}

#[cfg(not(windows))]
async fn notify_scoop_blocked(_task: &ScoopUpdateTask, _blocked: &ScoopBlockedProcessInfo) {}

fn ring_terminal_bell() {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x07");
    let _ = stdout.flush();
}

async fn notify_paru_password_attention_if_needed() {
    if !desktop_linux_session() {
        return;
    }

    let focus_state = terminal_focus_state().await;
    let Some(reason) = paru_password_attention_reason(focus_state) else {
        return;
    };

    println!("[paru] {reason}");
    notify_desktop_attention(
        "updt",
        "paru AUR 升级可能正在终端等待 sudo 密码, 请回到终端确认.",
    )
    .await;
}

fn paru_password_attention_reason(state: TerminalFocusState) -> Option<&'static str> {
    match state {
        TerminalFocusState::Focused => None,
        TerminalFocusState::NotFocused => {
            Some("terminal 未处于桌面焦点, paru 可能需要输入 sudo 密码, 已发送提醒.")
        }
        TerminalFocusState::Unknown => {
            Some("无法确认 terminal 处于桌面焦点, paru 可能需要输入 sudo 密码, 已发送提醒.")
        }
    }
}

async fn notify_desktop_attention(title: &str, body: &str) {
    ring_terminal_bell();

    if !command_exists("notify-send").await {
        println!("[notify] 未安装 notify-send, 无法发送桌面提醒.");
        return;
    }

    if let Err(err) = run_capture("notify-send", &[title, body]).await {
        println!("[notify] 发送桌面提醒失败: {err}");
    }
}

#[cfg(windows)]
async fn spawn_windows_message_box(title: &str, body: &str) -> io::Result<()> {
    let shell = if command_exists("powershell.exe").await {
        "powershell.exe"
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "未找到可用的 PowerShell",
        ));
    };

    let title_literal = ps_single_quote(title);
    let body_literal = ps_single_quote(body);
    let script = format!(
        "Add-Type -AssemblyName PresentationFramework; \
[System.Windows.MessageBox]::Show('{body_literal}', '{title_literal}') | Out-Null"
    );
    tokio::task::block_in_place(|| {
        Command::new(shell)
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-Command")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    })
}

async fn run_pacman_upgrade(state: &AppState) -> bool {
    let (privilege_command, reason) = pacman_privilege_command(state).await;

    if !pacman_privilege_command_available(privilege_command).await {
        return false;
    }

    if let Some(reason) = reason {
        println!("[pacman] {reason}");
    }
    println!("[pacman] 正在执行: {privilege_command} pacman -Syu");
    print_pacman_upgrade_result(run_inherit(privilege_command, &["pacman", "-Syu"]).await)
}

async fn pacman_privilege_command_available(privilege_command: &str) -> bool {
    if privilege_command != "pkexec" || command_exists(privilege_command).await {
        return true;
    }
    println!("[pacman] 未安装 pkexec, 无法使用 GUI 提权.");
    println!("[pacman] 包升级失败.");
    false
}

fn print_pacman_upgrade_result(result: io::Result<bool>) -> bool {
    if let Ok(true) = result {
        println!("[pacman] 包升级完成.");
        true
    } else {
        println!("[pacman] 包升级失败.");
        false
    }
}

async fn pacman_privilege_command(state: &AppState) -> (&'static str, Option<&'static str>) {
    if !state.is_arch_linux || !desktop_linux_session() {
        return ("sudo", None);
    }

    pacman_privilege_for_focus(terminal_focus_state().await)
}

fn pacman_privilege_for_focus(state: TerminalFocusState) -> (&'static str, Option<&'static str>) {
    match state {
        TerminalFocusState::Focused => ("sudo", None),
        TerminalFocusState::NotFocused => {
            ("pkexec", Some("terminal 未处于桌面焦点, 使用 GUI 提权."))
        }
        TerminalFocusState::Unknown => (
            "pkexec",
            Some("无法确认 terminal 处于桌面焦点, 使用 GUI 提权."),
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalFocusState {
    Focused,
    NotFocused,
    Unknown,
}

async fn terminal_focus_state() -> TerminalFocusState {
    if !interactive_terminal() {
        return TerminalFocusState::NotFocused;
    }

    terminal_focus_state_from_probes().await
}

async fn terminal_focus_state_from_probes() -> TerminalFocusState {
    if let Some(state) = terminal_focus_state_from_x11_window_id().await {
        return state;
    }
    terminal_focus_state_from_active_pid()
        .await
        .unwrap_or(TerminalFocusState::Unknown)
}

async fn terminal_focus_state_from_x11_window_id() -> Option<TerminalFocusState> {
    terminal_focused_by_x11_window_id()
        .await
        .map(focus_state_from_bool)
}

async fn terminal_focus_state_from_active_pid() -> Option<TerminalFocusState> {
    let pid = active_window_pid().await?;
    Some(focus_state_from_bool(
        current_process_belongs_to_window(pid).await,
    ))
}

fn focus_state_from_bool(focused: bool) -> TerminalFocusState {
    if focused {
        TerminalFocusState::Focused
    } else {
        TerminalFocusState::NotFocused
    }
}

async fn terminal_focused_by_x11_window_id() -> Option<bool> {
    let terminal_window_id = terminal_window_id_from_env()?;
    if !x11_window_id_probe_available(terminal_window_id).await {
        return None;
    }

    let active_window_id = active_x11_window_id().await?;
    Some(active_window_id == terminal_window_id)
}

fn terminal_window_id_from_env() -> Option<u64> {
    env::var("WINDOWID").ok()?.trim().parse::<u64>().ok()
}

async fn x11_window_id_probe_available(terminal_window_id: u64) -> bool {
    terminal_window_id != 0 && command_exists("xdotool").await
}

async fn active_x11_window_id() -> Option<u64> {
    let (status, output) = run_capture("xdotool", &["getactivewindow"]).await.ok()?;
    if status == 0 {
        output.trim().parse::<u64>().ok()
    } else {
        None
    }
}

async fn active_window_pid() -> Option<u32> {
    if let Some(pid) = active_window_pid_from_hyprland().await {
        Some(pid)
    } else {
        active_window_pid_from_x11().await
    }
}

async fn active_window_pid_from_hyprland() -> Option<u32> {
    if !hyprland_pid_probe_available().await {
        return None;
    }

    let output = hyprland_active_window_output().await?;
    pid_from_hyprland_window_json(&output)
}

async fn hyprland_pid_probe_available() -> bool {
    env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() && command_exists("hyprctl").await
}

async fn hyprland_active_window_output() -> Option<String> {
    let (status, output) = run_capture("hyprctl", &["activewindow", "-j"]).await.ok()?;
    (status == 0).then_some(output)
}

fn pid_from_hyprland_window_json(output: &str) -> Option<u32> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()?
        .get("pid")?
        .as_u64()
        .and_then(|pid| pid.try_into().ok())
}

async fn active_window_pid_from_x11() -> Option<u32> {
    if env::var_os("DISPLAY").is_none() || !command_exists("xdotool").await {
        return None;
    }

    let (status, output) = run_capture("xdotool", &["getactivewindow", "getwindowpid"])
        .await
        .ok()?;
    if status != 0 {
        return None;
    }
    output.trim().parse::<u32>().ok()
}

async fn current_process_belongs_to_window(window_pid: u32) -> bool {
    let mut next = Some(process::id());
    for _ in 0..64 {
        let Some(pid) = next else {
            break;
        };
        if pid == window_pid {
            return true;
        }
        next = parent_pid(pid).await;
    }
    false
}

async fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).await.ok()?;
    parent_pid_from_proc_stat(&stat)
}

fn parent_pid_from_proc_stat(stat: &str) -> Option<u32> {
    let after_comm = stat.rsplit_once(") ")?.1;
    let mut fields = after_comm.split_whitespace();
    fields.next()?;
    let parent = fields.next()?.parse::<u32>().ok()?;
    (parent != 0).then_some(parent)
}

#[cfg(windows)]
async fn schedule_windows_self_update(pkg: &str) -> io::Result<()> {
    let parent_pid = process::id();
    let script = format!(
        "$ErrorActionPreference='Continue'; \
$parentPid={parent_pid}; \
while (Get-Process -Id $parentPid -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 200 }}; \
cargo install-update --locked {pkg}; \
Write-Host ''; \
Write-Host 'Self-update finished. Press Enter to close this window.'; \
[void](Read-Host); \
exit"
    );

    let shell = if command_exists("pwsh").await {
        "pwsh"
    } else {
        "powershell.exe"
    };

    let primary = tokio::task::block_in_place(|| {
        Command::new("cmd.exe")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(shell)
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    });

    if primary.is_ok() {
        return Ok(());
    }

    tokio::task::block_in_place(|| {
        Command::new("cmd.exe")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg("powershell.exe")
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        RUSTUP_UPDATE_ARGS, ScoopInstallScope, ScoopUpdateTask, TerminalFocusState,
        UpgradeFailures, build_scoop_update_tasks, paru_password_attention_reason,
    };
    use crate::parse::ScoopExportApp;

    #[test]
    fn records_failure_labels_once() {
        let mut failures = UpgradeFailures::default();

        failures.record_target("brew");
        failures.record_label("cargo (updt 自身)");
        failures.record_target("brew");

        assert_eq!(
            failures.labels,
            vec![String::from("Homebrew"), String::from("cargo (updt 自身)")]
        );
    }

    #[test]
    fn paru_password_attention_skips_focused_terminal() {
        assert_eq!(
            paru_password_attention_reason(TerminalFocusState::Focused),
            None
        );
    }

    #[test]
    fn paru_password_attention_warns_when_focus_is_missing_or_unknown() {
        assert!(
            paru_password_attention_reason(TerminalFocusState::NotFocused)
                .is_some_and(|reason| reason.contains("未处于桌面焦点"))
        );
        assert!(
            paru_password_attention_reason(TerminalFocusState::Unknown)
                .is_some_and(|reason| reason.contains("无法确认"))
        );
    }

    #[test]
    fn rustup_upgrade_disables_self_update() {
        assert_eq!(RUSTUP_UPDATE_ARGS, ["update", "--no-self-update"]);
    }

    #[test]
    fn builds_one_local_scoop_task() {
        let tasks =
            build_scoop_update_tasks(&[String::from("git")], &[ScoopExportApp::new("git", false)]);

        assert_eq!(
            tasks,
            vec![ScoopUpdateTask {
                app: "git".to_string(),
                scope: ScoopInstallScope::Local,
            }]
        );
    }

    #[test]
    fn builds_one_global_scoop_task() {
        let tasks =
            build_scoop_update_tasks(&[String::from("git")], &[ScoopExportApp::new("git", true)]);

        assert_eq!(
            tasks,
            vec![ScoopUpdateTask {
                app: "git".to_string(),
                scope: ScoopInstallScope::Global,
            }]
        );
    }

    #[test]
    fn builds_local_and_global_scoop_tasks_for_same_app() {
        let tasks = build_scoop_update_tasks(
            &[String::from("git")],
            &[
                ScoopExportApp::new("git", false),
                ScoopExportApp::new("git", true),
            ],
        );

        assert_eq!(
            tasks,
            vec![
                ScoopUpdateTask {
                    app: "git".to_string(),
                    scope: ScoopInstallScope::Local,
                },
                ScoopUpdateTask {
                    app: "git".to_string(),
                    scope: ScoopInstallScope::Global,
                },
            ]
        );
    }
}
