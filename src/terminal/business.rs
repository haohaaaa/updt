//! Structured progress emitted by the workflow child; terminal output stays separate.
use crate::state::{AppState, TARGET_IDS, target_state_flags};
use std::io::{self, Write};
use std::sync::OnceLock;

static OWNER_PID: OnceLock<u32> = OnceLock::new();

pub fn enable_worker(owner_pid: u32) {
    let _ = OWNER_PID.set(owner_pid);
}

pub fn is_worker() -> bool {
    OWNER_PID.get().is_some()
}

#[cfg(windows)]
pub fn update_owner_pid() -> u32 {
    OWNER_PID.get().copied().unwrap_or_else(std::process::id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Checking,
    Current,
    Available,
    CheckFailed,
    NeedsTool,
    Skipped,
    Confirming,
    Queued,
    Updating,
    Updated,
    Failed,
    Deferred,
    PartialDeferred,
    Cancelled,
}

impl Status {
    fn id(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Checking => "checking",
            Self::Current => "current",
            Self::Available => "available",
            Self::CheckFailed => "check-failed",
            Self::NeedsTool => "needs-tool",
            Self::Skipped => "skipped",
            Self::Confirming => "confirming",
            Self::Queued => "queued",
            Self::Updating => "updating",
            Self::Updated => "updated",
            Self::Failed => "failed",
            Self::Deferred => "deferred",
            Self::PartialDeferred => "partial-deferred",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(id: &[u8]) -> Option<Self> {
        [
            Self::Waiting,
            Self::Checking,
            Self::Current,
            Self::Available,
            Self::CheckFailed,
            Self::NeedsTool,
            Self::Skipped,
            Self::Confirming,
            Self::Queued,
            Self::Updating,
            Self::Updated,
            Self::Failed,
            Self::Deferred,
            Self::PartialDeferred,
            Self::Cancelled,
        ]
        .into_iter()
        .find(|status| status.id().as_bytes() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "等待检查",
            Self::Checking => "检查中",
            Self::Current => "当前最新",
            Self::Available => "发现更新",
            Self::CheckFailed => "检查失败",
            Self::NeedsTool => "缺少 cargo-update",
            Self::Skipped => "已跳过",
            Self::Confirming => "等待确认",
            Self::Queued => "等待更新",
            Self::Updating => "更新中",
            Self::Updated => "更新完成",
            Self::Failed => "更新失败",
            Self::Deferred => "等待退出后更新",
            Self::PartialDeferred => "失败，待收尾",
            Self::Cancelled => "已取消",
        }
    }
}

fn send(target: &str, status: &str) {
    if !is_worker() {
        return;
    }
    let mut output = io::stdout().lock();
    let _ = write!(output, "\x1b]777;updt;{target};{status}\x1b\\");
    let _ = output.flush();
}

pub fn report(target: &str, status: Status) {
    send(target, status.id());
}

pub fn phase(phase: &'static str) {
    send("phase", phase);
}

pub fn checked(target: &str, state: &AppState) {
    report(target, check_status(target, state));
}

fn check_status(target: &str, state: &AppState) -> Status {
    let Some(flags) = target_state_flags(state, target) else {
        return Status::CheckFailed;
    };
    if !flags.enabled || !flags.installed {
        Status::Skipped
    } else if flags.check_failed {
        Status::CheckFailed
    } else if flags.needs_cargo_updater {
        Status::NeedsTool
    } else if flags.has_updates {
        Status::Available
    } else {
        Status::Current
    }
}

pub struct Progress {
    pub profile: String,
    pub start_time: String,
    pub targets: Vec<(String, Status)>,
    pub phase: &'static str,
}

impl Progress {
    pub fn new(targets: &[String], profile: &str, start_time: &str) -> Self {
        Self {
            profile: profile.to_string(),
            start_time: start_time.to_string(),
            targets: targets
                .iter()
                .map(|target| (target.clone(), Status::Waiting))
                .collect(),
            phase: "启动",
        }
    }

    pub fn apply(&mut self, params: &[&[u8]]) {
        let [b"777", b"updt", target, status] = params else {
            return;
        };
        if *target == b"phase" {
            self.phase = match *status {
                b"checking" => "检查",
                b"preparing" => "准备",
                b"selecting" => "确认",
                b"upgrading" => "更新",
                b"summary" => "汇总",
                _ => return,
            };
            return;
        }
        let Some(status) = Status::parse(status) else {
            return;
        };
        if !TARGET_IDS.iter().any(|id| id.as_bytes() == *target) {
            return;
        }
        if let Some((_, current)) = self
            .targets
            .iter_mut()
            .find(|(id, _)| id.as_bytes() == *target)
        {
            *current = status;
        }
    }

    pub fn finish(&mut self, code: i32) {
        self.phase = if code == 0 {
            "完成"
        } else {
            "中断或失败"
        };
        for (_, status) in &mut self.targets {
            if matches!(
                status,
                Status::Waiting
                    | Status::Checking
                    | Status::Confirming
                    | Status::Queued
                    | Status::Updating
            ) {
                *status = if code == 0 {
                    Status::Skipped
                } else {
                    Status::Cancelled
                };
            }
        }
    }

    pub fn has_deferred(&self) -> bool {
        self.targets
            .iter()
            .any(|(_, status)| matches!(status, Status::Deferred | Status::PartialDeferred))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_results_distinguish_skipped_current_updates_and_failures() {
        let mut state = AppState::default();
        assert_eq!(check_status("npm", &state), Status::Skipped);
        state.enable.npm = true;
        state.npm.installed = true;
        assert_eq!(check_status("npm", &state), Status::Current);
        state.npm.has_updates = true;
        assert_eq!(check_status("npm", &state), Status::Available);
        state.npm.check_failed = true;
        assert_eq!(check_status("npm", &state), Status::CheckFailed);
    }

    #[test]
    fn accepts_only_known_target_and_phase_events() {
        let mut progress = Progress::new(&["npm".into()], "windows", "now");
        progress.apply(&[b"777", b"updt", b"npm", b"updating"]);
        assert_eq!(progress.targets[0].1, Status::Updating);
        progress.apply(&[b"777", b"updt", b"phase", b"upgrading"]);
        assert_eq!(progress.phase, "更新");
        progress.apply(&[b"777", b"other", b"npm", b"failed"]);
        progress.apply(&[b"777", b"updt", b"npm", b"unknown"]);
        progress.apply(&[b"777", b"updt", b"cargo", b"failed"]);
        assert_eq!(progress.targets, vec![("npm".into(), Status::Updating)]);
        progress.finish(130);
        assert_eq!(progress.targets[0].1, Status::Cancelled);
    }

    #[test]
    fn keeps_deferred_updates_pending_after_worker_exits() {
        let mut progress = Progress::new(&["scoop".into(), "cargo".into()], "windows", "now");
        progress.apply(&[b"777", b"updt", b"scoop", b"deferred"]);
        progress.apply(&[b"777", b"updt", b"cargo", b"updated"]);
        progress.finish(0);
        assert!(progress.has_deferred());
        assert_eq!(progress.targets[0].1, Status::Deferred);
        assert_eq!(progress.targets[1].1, Status::Updated);
    }
}
