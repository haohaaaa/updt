use super::{confirm_default_yes, select_targets};
use crate::state::AppState;
use portable_pty::{CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct TerminalReplies(Vec<u8>);

impl vt100::Callbacks for TerminalReplies {
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
            self.0
                .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        }
    }
}

// Only the test executable can enter this fixture. It selects synthetic targets
// directly, so no package-manager commands or real updates can run.
#[tokio::test(flavor = "multi_thread")]
async fn inline_prompt_fixture() {
    let Some(mode) = std::env::var_os("UPDT_INLINE_PROMPT_FIXTURE") else {
        return;
    };
    let mut state = AppState::default();
    state.npm.updatable_items = vec!["example".into()];
    let targets = vec!["npm".into(), "cargo".into(), "rustup".into()];
    println!("fixture-history-before-prompt");
    let selected = select_targets(&state, &targets).await;
    println!("fixture-selected:{}", selected.join(","));
    if mode == "pipe" {
        return;
    }
    // A normal line prompt must still work after the inline selector returns.
    let answer = confirm_default_yes("fixture-continue").await;
    println!("fixture-answer:{answer:?}");
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

fn run_inline_prompt(keys: &[u8], expected: &str) {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "selection::prompt_tests::inline_prompt_fixture",
        "--nocapture",
    ]);
    command.env("UPDT_INLINE_PROMPT_FIXTURE", "1");
    command.env("TERM", "xterm-256color");
    let mut child = ChildGuard(pair.slave.spawn_command(command).unwrap());
    drop(pair.slave);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let _ = sender.send(buffer[..count].to_vec());
        }
    });
    let mut bytes = Vec::new();
    let mut parser = vt100::Parser::new_with_callbacks(
        30.try_into().unwrap(),
        120.try_into().unwrap(),
        100,
        TerminalReplies::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut selected = false;
    let mut continued = false;
    loop {
        if let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(20)) {
            parser.process(&chunk);
            bytes.extend(chunk);
        }
        let replies = std::mem::take(&mut parser.callbacks_mut().0);
        if !replies.is_empty() {
            writer.write_all(&replies).unwrap();
            writer.flush().unwrap();
        }
        let output = String::from_utf8_lossy(&bytes);
        if !selected && output.contains("Esc: 退出") {
            writer.write_all(keys).unwrap();
            writer.flush().unwrap();
            selected = true;
        }
        if selected && !continued && output.contains("fixture-continue [Y/n]:") {
            writer.write_all(b"n\r").unwrap();
            writer.flush().unwrap();
            continued = true;
        }
        if let Some(status) = child.0.try_wait().unwrap()
            && output.contains("fixture-answer:")
        {
            assert_eq!(status.exit_code(), 0, "{output}");
            assert!(selected && continued, "fixture exited early: {output}");
            assert!(
                parser
                    .screen()
                    .contents()
                    .lines()
                    .any(|line| { line.trim_end() == format!("fixture-selected:{expected}") }),
                "{output}"
            );
            assert!(output.contains("fixture-answer:Some(false)"), "{output}");
            assert!(
                parser
                    .screen()
                    .contents()
                    .contains("fixture-history-before-prompt")
            );
            assert!(!parser.screen().alternate_screen(), "{output}");
            assert!(!parser.screen().hide_cursor(), "{output}");
            for sequence in ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h", "\x1b[2J"] {
                assert!(
                    !output.contains(sequence),
                    "unexpected full-screen operation: {output}"
                );
            }
            break;
        }
        assert!(
            Instant::now() < deadline,
            "inline prompt timed out: {output}"
        );
    }
}

#[test]
fn inline_selection_supports_arrows_space_and_enter_without_clearing_history() {
    // All targets start selected. Move down, deselect cargo, then confirm.
    run_inline_prompt(b"\x1b[B \r", "npm,rustup");
}

#[test]
fn inline_selection_can_deselect_all() {
    run_inline_prompt(b"\x1b[D\r", "");
}

#[test]
fn inline_selection_can_cancel() {
    run_inline_prompt(b"\x1b", "");
}

#[test]
fn inline_selection_restores_cursor_after_interrupt() {
    run_inline_prompt(b"\x03", "");
}

fn run_piped_prompt(input: &[u8], expected: &str) {
    use std::process::{Command, Stdio};

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "selection::prompt_tests::inline_prompt_fixture",
            "--nocapture",
        ])
        .env("UPDT_INLINE_PROMPT_FIXTURE", "pipe")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[Y/n]:"), "{stdout}");
    assert!(
        stdout.contains(&format!("fixture-selected:{expected}\n")),
        "{stdout}"
    );
    assert!(!stdout.contains('\x1b'), "{stdout}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn redirected_input_uses_line_prompts_and_accepts_the_default() {
    run_piped_prompt(b"n\ny\n\n", "cargo,rustup");
}

#[test]
fn redirected_input_eof_cancels_previous_selections() {
    run_piped_prompt(b"y\n", "");
}
