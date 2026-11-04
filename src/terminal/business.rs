//! Structured progress emitted by the workflow child; terminal output stays separate.
use crate::state::{AppState, TARGET_IDS, target_state_flags};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::OnceLock;

const HISTORY_LIMIT: usize = 500;

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

// JSON escapes control characters; escape OSC's field separator as well.
fn text_payload(text: &str) -> String {
    let text = text.chars().take(4096).collect::<String>();
    serde_json::to_string(&text)
        .unwrap()
        .replace(';', "\\u003b")
}

pub fn notice(text: &str) {
    send("notice", &text_payload(text));
}

pub fn command_started(label: &str) {
    send("command", &text_payload(label));
}

pub fn command_finished(result: &str) {
    send("command-result", &text_payload(result));
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
    pub command: Option<String>,
    pub command_result: Option<String>,
    pub history: VecDeque<String>,
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
            command: None,
            command_result: None,
            history: VecDeque::new(),
        }
    }

    pub fn apply(&mut self, params: &[&[u8]]) {
        let [b"777", b"updt", target, status] = params else {
            return;
        };
        if matches!(*target, b"notice" | b"command" | b"command-result") {
            let Ok(text) = serde_json::from_slice::<String>(status) else {
                return;
            };
            let text: String = text
                .chars()
                .take(4096)
                .filter(|c| !c.is_control() || *c == '\n')
                .collect();
            if *target == b"command" {
                self.command = Some(text.replace('\n', " "));
                self.command_result = None;
                self.record(&format!("执行  $ {text}"));
            } else if *target == b"command-result" {
                self.command_result = Some(text.replace('\n', " "));
                self.record(&format!("结果  {text}"));
            } else {
                self.record(&text);
            }
            return;
        }
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

    fn record(&mut self, text: &str) {
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            // Messages are plain text. Never let control characters affect the UI.
            self.history
                .push_back(line.chars().filter(|c| !c.is_control()).collect());
            if self.history.len() > HISTORY_LIMIT {
                self.history.pop_front();
            }
        }
    }

    pub fn finish(&mut self, code: i32) {
        if self.command.is_some() && self.command_result.is_none() {
            self.command_result = Some(format!("流程已退出 · exit {code}"));
        }
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
    fn message_protocol_handles_chunking_separators_unicode_and_control_characters() {
        struct Events(Progress);
        impl vt100::Callbacks for Events {
            fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
                self.0.apply(params);
            }
        }
        let mut parser = vt100::Parser::new_with_callbacks(
            10.try_into().unwrap(),
            40.try_into().unwrap(),
            0,
            Events(Progress::new(&[], "windows", "now")),
        );
        let message = "中文; quoted \"value\"\nnext\x1b\\\x07";
        let payload = text_payload(message);
        assert!(!payload.contains(';'));
        let bytes = format!("\x1b]777;updt;notice;{payload}\x1b\\visible output");
        for chunk in bytes.as_bytes().chunks(2) {
            parser.process(chunk);
        }
        assert_eq!(parser.screen().contents(), "visible output");
        assert_eq!(parser.callbacks().0.history[0], "中文; quoted \"value\"");
        assert_eq!(parser.callbacks().0.history[1], "next\\");
    }

    #[test]
    fn command_results_reset_and_activity_history_is_bounded() {
        let mut progress = Progress::new(&[], "windows", "now");
        progress.apply(&[b"777", b"updt", b"command", br#""cargo install-update""#]);
        progress.apply(&[b"777", b"updt", b"command-result", br#""exit 1""#]);
        assert_eq!(progress.command_result.as_deref(), Some("exit 1"));
        progress.apply(&[b"777", b"updt", b"command", br#""npm update -g""#]);
        assert!(progress.command_result.is_none());
        for i in 0..HISTORY_LIMIT + 10 {
            progress.record(&i.to_string());
        }
        assert_eq!(progress.history.len(), HISTORY_LIMIT);
        assert_eq!(progress.history.front().unwrap(), "10");
        progress.finish(130);
        assert_eq!(
            progress.command_result.as_deref(),
            Some("流程已退出 · exit 130")
        );
    }

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
