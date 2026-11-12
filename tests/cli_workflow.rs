use assert_cmd::cargo::cargo_bin_cmd;
use portable_pty::{CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
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

// These targets are disabled by OS policy, so no packages can be updated.
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
fn default_cli_in_a_tty_does_not_enter_a_full_screen_ui() {
    run_check_in_pty(&[]);
}

#[test]
fn explicit_update_in_a_tty_does_not_enter_a_full_screen_ui() {
    run_check_in_pty(&["update", disabled_target()]);
}

fn run_check_in_pty(args: &[&str]) {
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
    command.args(args);
    // Prevent real package-manager checks from the default entry point.
    if args.is_empty() {
        command.env("PATH", "");
    }
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
            // Keep draining ConPTY even if the receiver has gone away.
            let _ = sender.send(buffer[..count].to_vec());
        }
    });
    let mut parser = vt100::Parser::new_with_callbacks(
        30.try_into().unwrap(),
        100.try_into().unwrap(),
        100,
        Replies::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut output = Vec::new();
    loop {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(50)) {
            parser.process(&bytes);
            output.extend(bytes);
        }
        let replies = std::mem::take(&mut parser.callbacks_mut().0);
        if !replies.is_empty() {
            writer.write_all(&replies).unwrap();
            writer.flush().unwrap();
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.exit_code(), 0, "{}", parser.screen().contents());
            if parser.screen().contents().contains("没有可升级项") {
                let output = String::from_utf8_lossy(&output);
                assert!(!output.contains("777;updt"));
                for sequence in ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h", "\x1b[2J"] {
                    assert!(
                        !output.contains(sequence),
                        "unexpected full-screen operation: {output}"
                    );
                }
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "workflow timed out, screen: {}",
            parser.screen().contents()
        );
    }
}

#[test]
fn redirected_cli_prints_plain_check_results() {
    let result = cargo_bin_cmd!("updt")
        .args(["update", disabled_target()])
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stderr("");
    let stdout = String::from_utf8_lossy(&result.get_output().stdout);
    assert!(!stdout.contains("777;updt"));
    assert!(!stdout.contains('\x1b'), "{stdout}");
    assert!(stdout.contains("没有可升级项"));
}
