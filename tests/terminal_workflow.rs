use portable_pty::{CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Events(Vec<(String, String)>, Vec<u8>);

impl vt100::Callbacks for Events {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        _: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if command == 'n' && params.first().and_then(|value| value.first()) == Some(&6) {
            let (row, col) = screen.cursor_position();
            self.1
                .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        }
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let [b"777", b"updt", target, status] = params {
            self.0.push((
                String::from_utf8_lossy(target).into_owned(),
                String::from_utf8_lossy(status).into_owned(),
            ));
        }
    }
}

// This target is disabled by the OS policy, so these tests never refresh
// repositories, install tools or update any packages.
fn disabled_target() -> &'static str {
    if cfg!(windows) {
        "dnf"
    } else if cfg!(target_os = "macos") {
        "pacman"
    } else {
        "brew"
    }
}

struct ChildGuard(Box<dyn portable_pty::Child + Send + Sync>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn business_worker_runs_inside_native_pty_and_reports_real_check_results() {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_updt"));
    command.args([
        "__terminal-worker",
        "--owner-pid",
        &std::process::id().to_string(),
        disabled_target(),
    ]);
    command.env("TERM", "xterm-256color");
    let mut child = ChildGuard(pair.slave.spawn_command(command).unwrap());
    drop(pair.slave);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            // Keep draining until ConPTY closes even if the test has failed.
            let _ = sender.send(buffer[..count].to_vec());
        }
    });
    let mut parser = vt100::Parser::new_with_callbacks(
        30.try_into().unwrap(),
        100.try_into().unwrap(),
        100,
        Events::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(50)) {
            parser.process(&bytes);
        }
        let replies = std::mem::take(&mut parser.callbacks_mut().1);
        if !replies.is_empty() {
            writer.write_all(&replies).unwrap();
            writer.flush().unwrap();
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.exit_code(), 0, "{}", parser.screen().contents());
            if parser
                .callbacks()
                .0
                .contains(&(disabled_target().into(), "skipped".into()))
            {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "workflow timed out, events: {:?}, screen: {}",
            parser.callbacks().0,
            parser.screen().contents()
        );
    }
    let events = &parser.callbacks().0;
    assert!(events.contains(&(disabled_target().into(), "checking".into())));
    assert!(events.contains(&(disabled_target().into(), "skipped".into())));
    assert!(!events.iter().any(|(_, status)| status == "updating"));
}

#[test]
fn legacy_text_entry_does_not_emit_embedded_terminal_protocol() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_updt"))
        .args(["update", disabled_target()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("777;updt"));
    assert!(stdout.contains("没有可升级项"));
}

#[test]
fn validation_modes_cannot_be_combined_with_update_targets() {
    for args in [
        vec!["terminal", "--demo", "npm"],
        vec!["terminal", "--shell", "npm"],
        vec!["terminal", "--demo", "--shell"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_updt"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
}

#[test]
fn terminal_demo_executes_via_internal_api_routes_input_and_switches_sessions() {
    run_terminal_demo(false, None);
}

#[test]
fn terminal_demo_supports_mouse_focus_session_selection_and_exit() {
    run_terminal_demo(true, None);
}

#[test]
fn terminal_demo_supports_hjkl_panel_and_session_navigation() {
    run_terminal_demo(false, Some(b"hj\rjj"));
}

#[test]
fn terminal_demo_supports_arrow_panel_and_session_navigation() {
    run_terminal_demo(false, Some(b"\x1b[D\x1b[B\r\x1b[B\x1b[B"));
}

fn click_ascii(screen: &vt100::Screen, text: &str, max_col: u16) -> Vec<u8> {
    let (rows, cols) = screen.size();
    for row in 0..rows.get() {
        for col in 0..cols
            .get()
            .min(max_col)
            .saturating_sub(text.len() as u16 - 1)
        {
            if text.chars().enumerate().all(|(i, ch)| {
                screen
                    .cell(row, col + i as u16)
                    .is_some_and(|cell| cell.contents() == ch.to_string())
            }) {
                return format!(
                    "\x1b[<0;{};{}M\x1b[<0;{};{}m",
                    col + 1,
                    row + 1,
                    col + 1,
                    row + 1
                )
                .into_bytes();
            }
        }
    }
    panic!("missing clickable text {text}: {}", screen.contents());
}

fn run_terminal_demo(mouse: bool, navigation: Option<&[u8]>) {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 32,
            cols: 130,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_updt"));
    command.args(["terminal", "--demo"]);
    command.env("TERM", "xterm-256color");
    let mut child = ChildGuard(pair.slave.spawn_command(command).unwrap());
    drop(pair.slave);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        while let Ok(count) = reader.read(&mut bytes) {
            if count == 0 {
                break;
            }
            let _ = sender.send(bytes[..count].to_vec());
        }
    });
    let mut parser = vt100::Parser::new_with_callbacks(
        32.try_into().unwrap(),
        130.try_into().unwrap(),
        100,
        Events::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut answered = false;
    let mut switched = false;
    let mut exited = false;
    let mut blurred = false;
    let mut navigation_started = false;
    loop {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
            parser.process(&bytes);
        }
        let replies = std::mem::take(&mut parser.callbacks_mut().1);
        if !replies.is_empty() {
            writer.write_all(&replies).unwrap();
            writer.flush().unwrap();
        }
        let screen = parser.screen().contents();
        if !answered && screen.contains("输入任意文字后按 Enter") {
            if mouse {
                writer.write_all(b"\x1b[<0;40;10M\x1b[<0;40;10m").unwrap();
            } else {
                writer.write_all(b"\r").unwrap(); // Enter focuses the output pane.
            }
            writer.write_all(b"hello qhjkl\r").unwrap();
            writer.flush().unwrap();
            answered = true;
        }
        if answered
            && !switched
            && screen.contains("内部 API 收到交互命令结果")
            && screen.contains("终端已退出")
            && (!mouse || screen.contains("#2 完成"))
        {
            if navigation.is_some() && !navigation_started {
                writer.write_all(b"\x1b").unwrap();
                writer.flush().unwrap();
                navigation_started = true;
            } else if navigation.is_none() || screen.contains("导航 · hjkl/方向键") {
                if mouse {
                    writer
                        .write_all(&click_ascii(parser.screen(), "#2", 30))
                        .unwrap();
                } else if let Some(keys) = navigation {
                    writer.write_all(keys).unwrap();
                } else {
                    writer.write_all(b"\x1b[15~").unwrap(); // F5: last command session
                }
                writer.flush().unwrap();
                switched = true;
            }
        }
        if switched
            && !blurred
            && screen.contains("收到: hello qhjkl")
            && screen.contains("命令输出")
            && (!mouse || screen.contains("[q 退出]"))
        {
            if mouse {
                writer
                    .write_all(&click_ascii(parser.screen(), "q ", 130))
                    .unwrap();
                exited = true;
            } else {
                // Enter no longer exits a completed workflow. Esc only blurs it.
                writer.write_all(b"\x1b[17~\r\x1b").unwrap();
            }
            writer.flush().unwrap();
            blurred = true;
        }
        if blurred && !exited && screen.contains("导航 · hjkl/方向键") {
            writer.write_all(b"q").unwrap();
            writer.flush().unwrap();
            exited = true;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.exit_code(), 0, "{screen}");
            assert!(
                answered && switched && exited,
                "demo exited early: {screen}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "demo timed out: answered={answered}, switched={switched}, exited={exited}\n{screen}"
        );
    }
}
