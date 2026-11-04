//! Opt-in PTY UI. The legacy interface stays independent of this entry point.
pub mod business;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::{
    io::{self, Read, Write},
    num::NonZeroU16,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

const SCROLLBACK: usize = 2_000;

pub async fn default_shell() -> String {
    if cfg!(windows) {
        if crate::command::command_exists("pwsh").await {
            "pwsh.exe".to_string()
        } else {
            "powershell.exe".to_string()
        }
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

enum PtyEvent {
    Output(Vec<u8>),
    Error(io::Error),
}

struct PtySession {
    // Drop the receiver before closing ConPTY, so its output reader can keep
    // draining without blocking on a full channel during console teardown.
    events: Option<Receiver<PtyEvent>>,
    writer: Option<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PtySession {
    fn spawn(command: CommandBuilder, size: PtySize) -> io::Result<Self> {
        let pair = portable_pty::native_pty_system()
            .openpty(size)
            .map_err(io::Error::other)?;
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let (sender, receiver) = mpsc::sync_channel(64);
        if let Err(err) = std::thread::Builder::new()
            .name("terminal-output".into())
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                let mut connected = true;
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if connected {
                                connected = sender
                                    .send(PtyEvent::Output(buffer[..count].to_vec()))
                                    .is_ok();
                            }
                        }
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        // Unix PTYs commonly report EIO when the slave closes.
                        Err(err) if cfg!(unix) && err.raw_os_error() == Some(5) => break,
                        Err(err) => {
                            let _ = sender.send(PtyEvent::Error(err));
                            break;
                        }
                    }
                }
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        Ok(Self {
            events: Some(receiver),
            writer: Some(writer),
            master: pair.master,
            child,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.write_all(bytes)?;
            writer.flush()?;
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.events.take();
        self.writer.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Responses {
    bytes: Vec<u8>,
    progress: Option<business::Progress>,
}

impl vt100::Callbacks for Responses {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if i1.is_some() || i2.is_some() {
            return;
        }
        match (
            command,
            params
                .first()
                .and_then(|param| param.first())
                .copied()
                .unwrap_or(0),
        ) {
            ('n', 6) => {
                let (row, col) = screen.cursor_position();
                self.bytes
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            ('n', 5) => self.bytes.extend_from_slice(b"\x1b[0n"),
            ('c', 0) => self.bytes.extend_from_slice(b"\x1b[?1;2c"),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let Some(progress) = &mut self.progress {
            progress.apply(params);
        }
    }
}

struct TerminalMode;
impl TerminalMode {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste) {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self)
    }
}
impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).unwrap()
}

fn regions(area: Rect) -> (Rect, Rect, Rect) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    let sidebar_width = if area.width >= 70 {
        30
    } else if area.width >= 50 {
        24
    } else {
        0
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Min(1)])
        .split(vertical[0]);
    (columns[0], columns[1], vertical[1])
}

fn pty_size(area: Rect) -> PtySize {
    let (_, pane, _) = regions(area);
    PtySize {
        rows: pane.height.saturating_sub(2).max(1),
        cols: pane.width.saturating_sub(2).max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub fn run(shell: &str, demo: bool) -> io::Result<i32> {
    let command = if demo {
        let mut command = CommandBuilder::new(std::env::current_exe()?);
        command.arg("__terminal-demo");
        command
    } else {
        let mut command = CommandBuilder::new(shell);
        if cfg!(windows) {
            command.args(["-NoLogo", "-NoProfile"]);
        }
        command
    };
    run_session(command, if demo { "交互演示" } else { shell }, None)
}

pub fn run_workflow(targets: &[String], progress: business::Progress) -> io::Result<i32> {
    let mut command = CommandBuilder::new(std::env::current_exe()?);
    command.args([
        "__terminal-worker",
        "--owner-pid",
        &std::process::id().to_string(),
    ]);
    command.args(targets);
    run_session(command, "更新流程", Some(progress))
}

fn run_session(
    mut command: CommandBuilder,
    label: &str,
    progress: Option<business::Progress>,
) -> io::Result<i32> {
    if !crate::profile::interactive_terminal() {
        return Err(io::Error::other("请在交互式终端中运行 updt terminal"));
    }
    let _guard = TerminalMode::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (width, height) = crossterm::terminal::size()?;
    let mut size = pty_size(Rect::new(0, 0, width, height));
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.cwd(std::env::current_dir()?);
    let mut session = PtySession::spawn(command, size)?;
    let mut parser = vt100::Parser::new_with_callbacks(
        nonzero(size.rows),
        nonzero(size.cols),
        SCROLLBACK,
        Responses {
            bytes: Vec::new(),
            progress,
        },
    );
    let mut exit_code = None;
    let mut confirm_exit = false;
    loop {
        for _ in 0..64 {
            match session.events.as_ref().unwrap().try_recv() {
                Ok(PtyEvent::Output(bytes)) => parser.process(&bytes),
                Ok(PtyEvent::Error(err)) => return Err(err),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let responses = std::mem::take(&mut parser.callbacks_mut().bytes);
        if !responses.is_empty() && exit_code.is_none() {
            session.write(&responses)?;
        }
        if exit_code.is_none() {
            exit_code = session
                .child
                .try_wait()?
                .map(|status| status.exit_code() as i32);
            if let (Some(code), Some(progress)) = (exit_code, &mut parser.callbacks_mut().progress)
            {
                progress.finish(code);
            }
        }
        terminal.draw(|frame| {
            render(
                frame,
                parser.screen(),
                label,
                exit_code,
                confirm_exit,
                parser.callbacks().progress.as_ref(),
            )
        })?;
        if !event::poll(Duration::from_millis(30))? {
            continue;
        }
        match event::read()? {
            Event::Resize(width, height) => {
                size = pty_size(Rect::new(0, 0, width, height));
                session.master.resize(size).map_err(io::Error::other)?;
                parser.set_size(nonzero(size.rows), nonzero(size.cols));
            }
            Event::Paste(text) if exit_code.is_none() && !confirm_exit => {
                parser.screen_mut().set_scrollback(0);
                if parser.screen().bracketed_paste() {
                    session.write(b"\x1b[200~")?;
                }
                session.write(text.as_bytes())?;
                if parser.screen().bracketed_paste() {
                    session.write(b"\x1b[201~")?;
                }
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if confirm_exit {
                    match key.code {
                        KeyCode::Char('y' | 'Y') => return Ok(130),
                        KeyCode::Esc | KeyCode::Char('n' | 'N') => confirm_exit = false,
                        _ => {}
                    }
                } else if key.code == KeyCode::F(10) {
                    if let Some(code) = exit_code {
                        return Ok(code);
                    }
                    confirm_exit = true;
                } else if key.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                {
                    let offset = parser.screen().scrollback();
                    let offset = if key.code == KeyCode::PageUp {
                        offset.saturating_add(usize::from(size.rows))
                    } else {
                        offset.saturating_sub(usize::from(size.rows))
                    };
                    parser.screen_mut().set_scrollback(offset);
                } else if let Some(code) = exit_code {
                    if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                        return Ok(code);
                    }
                } else {
                    parser.screen_mut().set_scrollback(0);
                    session.write(&encode_key(key, parser.screen().application_cursor()))?;
                }
            }
            _ => {}
        }
    }
}

fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let modifiers = 1
        + u8::from(key.modifiers.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(key.modifiers.contains(KeyModifiers::ALT))
        + 4 * u8::from(key.modifiers.contains(KeyModifiers::CONTROL));
    let arrow = match key.code {
        KeyCode::Up => Some('A'),
        KeyCode::Down => Some('B'),
        KeyCode::Right => Some('C'),
        KeyCode::Left => Some('D'),
        KeyCode::Home => Some('H'),
        KeyCode::End => Some('F'),
        _ => None,
    };
    if let Some(code) = arrow {
        return if modifiers > 1 {
            format!("\x1b[1;{modifiers}{code}")
        } else if application_cursor {
            format!("\x1bO{code}")
        } else {
            format!("\x1b[{code}")
        }
        .into_bytes();
    }
    let tilde = match key.code {
        KeyCode::Insert => Some(2),
        KeyCode::Delete => Some(3),
        KeyCode::PageUp => Some(5),
        KeyCode::PageDown => Some(6),
        KeyCode::F(n @ 5..=12) => Some([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => None,
    };
    if let Some(code) = tilde {
        return if modifiers > 1 {
            format!("\x1b[{code};{modifiers}~")
        } else {
            format!("\x1b[{code}~")
        }
        .into_bytes();
    }
    let mut bytes = match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let c = c.to_ascii_uppercase();
            match c {
                '@'..='_' => vec![c as u8 & 0x1f],
                ' ' => vec![0],
                '?' => vec![127],
                _ => Vec::new(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![27],
        KeyCode::F(n @ 1..=4) => if modifiers > 1 {
            format!("\x1b[1;{modifiers}{}", char::from(b'P' + n - 1))
        } else {
            format!("\x1bO{}", char::from(b'P' + n - 1))
        }
        .into_bytes(),
        _ => Vec::new(),
    };
    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 27);
    }
    bytes
}

fn render(
    frame: &mut ratatui::Frame<'_>,
    screen: &vt100::Screen,
    shell: &str,
    exit_code: Option<i32>,
    confirm_exit: bool,
    progress: Option<&business::Progress>,
) {
    let (sidebar, pane, footer) = regions(frame.area());
    if sidebar.width > 0 {
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(1)])
            .split(sidebar);
        let header = if let Some(progress) = progress {
            format!(
                "开始: {}\n策略: {}\n阶段: {}\n终端: {} × {}",
                progress.start_time,
                progress.profile,
                progress.phase,
                screen.size().1,
                screen.size().0
            )
        } else {
            format!(
                "真实 PTY · 业务未接入\n终端: {shell}\n尺寸: {} × {}\n左侧为占位列表",
                screen.size().1,
                screen.size().0
            )
        };
        frame.render_widget(
            Paragraph::new(header).block(
                Block::default()
                    .title(if progress.is_some() {
                        "updt · 更新流程"
                    } else {
                        "updt · Terminal MVP"
                    })
                    .borders(Borders::ALL),
            ),
            left[0],
        );
        let targets = if let Some(progress) = progress {
            progress
                .targets
                .iter()
                .map(|(target, status)| {
                    ListItem::new(format!(
                        "{:<9} {}",
                        crate::state::target_label(target),
                        status.label()
                    ))
                    .style(status_style(*status))
                })
                .collect::<Vec<_>>()
        } else {
            ["npm", "cargo", "rustup", "scoop"]
                .map(|name| {
                    ListItem::new(format!("{name:<10} 未接入"))
                        .style(Style::default().fg(Color::DarkGray))
                })
                .into_iter()
                .collect()
        };
        frame.render_widget(
            List::new(targets).block(
                Block::default()
                    .title(if progress.is_some() {
                        "目标"
                    } else {
                        "目标（占位）"
                    })
                    .borders(Borders::ALL),
            ),
            left[1],
        );
    }
    let title = exit_code.map_or_else(
        || "命令终端 · 输入直接发送".to_string(),
        |code| format!("终端已退出 · exit {code}"),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(pane);
    frame.render_widget(block, pane);
    let lines = (0..inner.height)
        .map(|row| {
            let spans = (0..inner.width)
                .filter_map(|col| {
                    let cell = screen.cell(row, col)?;
                    if cell.is_wide_continuation() {
                        return None;
                    }
                    Some(Span::styled(
                        if cell.has_contents() {
                            cell.contents()
                        } else {
                            " "
                        },
                        cell_style(cell),
                    ))
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
    if exit_code.is_none() && !confirm_exit && screen.scrollback() == 0 && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < inner.height && col < inner.width {
            frame.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
    let help = if confirm_exit {
        "结束当前终端进程并退出？ Y: 确认  N/Esc: 返回"
    } else if exit_code.is_some() && progress.is_some_and(business::Progress::has_deferred) {
        "Enter/Esc: 退出并执行收尾更新 · Shift+PgUp/PgDn: 回看输出"
    } else if exit_code.is_some() {
        "Enter / Esc / F10: 返回 · Shift+PgUp/PgDn: 回看输出"
    } else {
        "Ctrl+C: 发送中断 · Shift+PgUp/PgDn: 回看输出 · F10: 退出"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(if confirm_exit {
            Color::Yellow
        } else {
            Color::DarkGray
        })),
        footer,
    );
}

fn status_style(status: business::Status) -> Style {
    use business::Status;
    Style::default().fg(match status {
        Status::Checking | Status::Updating => Color::Cyan,
        Status::Current | Status::Updated => Color::Green,
        Status::CheckFailed | Status::Failed | Status::PartialDeferred => Color::Red,
        Status::Waiting | Status::Skipped | Status::Cancelled => Color::DarkGray,
        _ => Color::Yellow,
    })
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let color = |color| match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    };
    let mut style = Style::default()
        .fg(color(cell.fgcolor()))
        .bg(color(cell.bgcolor()));
    for (enabled, modifier) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if enabled {
            style = style.add_modifier(modifier);
        }
    }
    style
}

pub fn run_demo() -> i32 {
    println!("\x1b[1;36mupdt · Embedded terminal MVP\x1b[0m\r");
    println!("这是终端验证演示，不运行包管理器。\r");
    println!("\x1b[32m颜色输出 OK\x1b[0m  中文 / Unicode: 你好，终端！\r");
    for progress in [0, 25, 50, 75, 100] {
        print!("\r\x1b[2K进度刷新: {progress}%");
        let _ = io::stdout().flush();
        std::thread::sleep(Duration::from_millis(150));
    }
    println!("\r\n\r\n请输入任意内容后按 Enter（也可以输入 y/n）:\r");
    let mut answer = String::new();
    if let Err(err) = io::stdin().read_line(&mut answer) {
        eprintln!("无法读取输入: {err}");
        return 1;
    }
    println!("\r\n收到输入: {}\r", answer.trim_end());
    println!("\x1b[32m交互完成。可以调整窗口大小，或按 Enter 返回。\x1b[0m\r");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::time::Instant;

    #[test]
    fn structured_status_survives_chunked_terminal_output_and_stays_off_screen() {
        let progress = business::Progress::new(&["npm".into()], "windows", "now");
        let mut parser = vt100::Parser::new_with_callbacks(
            nonzero(20),
            nonzero(68),
            10,
            Responses {
                bytes: Vec::new(),
                progress: Some(progress),
            },
        );
        for chunk in b"\x1b]777;updt;npm;updating\x1b\\command output".chunks(3) {
            parser.process(chunk);
        }
        assert_eq!(
            parser.callbacks().progress.as_ref().unwrap().targets[0].1,
            business::Status::Updating
        );
        assert_eq!(parser.screen().contents(), "command output");
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    parser.screen(),
                    "更新流程",
                    None,
                    false,
                    parser.callbacks().progress.as_ref(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 8)].symbol(), "n");
        assert_eq!(buffer[(31, 1)].symbol(), "c");
    }

    #[test]
    fn forwards_text_controls_and_terminal_key_modes() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        assert_eq!(
            encode_key(key(KeyCode::Char('你'), KeyModifiers::NONE), false),
            "你".as_bytes()
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            [3]
        );
        assert_eq!(
            encode_key(key(KeyCode::Enter, KeyModifiers::NONE), false),
            b"\r"
        );
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), false),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(key(KeyCode::Left, KeyModifiers::CONTROL), false),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
    }

    #[test]
    fn renders_unicode_color_and_cursor_updates_in_right_pane() {
        let mut parser = vt100::Parser::new(nonzero(20), nonzero(68), 10);
        parser.process("\x1b[32m绿色\x1b[0m\r\nprogress 1%\r\x1b[2Kprogress 100%".as_bytes());
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, parser.screen(), "test shell", None, false, None))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(31, 1)].symbol(), "绿");
        assert_eq!(buffer[(31, 1)].fg, Color::Indexed(2));
        assert_eq!(buffer[(33, 1)].symbol(), "色");
        assert!(parser.screen().contents().contains("progress 100%"));
        assert!(!parser.screen().contents().contains("progress 1%"));
    }

    #[test]
    fn answers_cursor_queries_and_handles_small_terminal_sizes() {
        let mut parser =
            vt100::Parser::new_with_callbacks(nonzero(5), nonzero(10), 0, Responses::default());
        parser.process(b"abc\x1b[6n");
        assert_eq!(parser.callbacks().bytes, b"\x1b[1;4R");
        for (width, height) in [(1, 1), (20, 4), (50, 12), (120, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render(frame, parser.screen(), "shell", None, false, None))
                .unwrap();
            let size = pty_size(Rect::new(0, 0, width, height));
            assert!(size.rows > 0 && size.cols > 0);
        }
    }

    #[test]
    fn native_pty_accepts_input_resizes_and_preserves_child_exit_code() {
        #[cfg(windows)]
        let mut command = {
            let mut command = CommandBuilder::new("powershell.exe");
            command.args(["-NoLogo", "-NoProfile", "-Command", "[Console]::WriteLine('PTY_READY'); $line=[Console]::ReadLine(); [Console]::WriteLine('ECHO:' + $line); exit 7"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args([
                "-c",
                "printf 'PTY_READY\\n'; IFS= read -r line; printf 'ECHO:%s\\n' \"$line\"; exit 7",
            ]);
            command
        };
        command.env("TERM", "xterm-256color");
        let mut session = PtySession::spawn(
            command,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .unwrap();
        let mut parser =
            vt100::Parser::new_with_callbacks(nonzero(24), nonzero(80), 100, Responses::default());
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut sent = false;
        loop {
            assert!(
                Instant::now() < deadline,
                "PTY timed out: {}",
                parser.screen().contents()
            );
            match session
                .events
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(PtyEvent::Output(bytes)) => parser.process(&bytes),
                Ok(PtyEvent::Error(err)) => panic!("PTY read failed: {err}"),
                Err(_) => {}
            }
            let replies = std::mem::take(&mut parser.callbacks_mut().bytes);
            if !replies.is_empty() {
                session.write(&replies).unwrap();
            }
            if !sent && parser.screen().contents().contains("PTY_READY") {
                let size = PtySize {
                    rows: 16,
                    cols: 100,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                session.master.resize(size).unwrap();
                parser.set_size(nonzero(16), nonzero(100));
                session.write(b"hello\r").unwrap();
                sent = true;
            }
            if parser.screen().contents().contains("ECHO:hello") {
                break;
            }
        }
        loop {
            if let Some(status) = session.child.try_wait().unwrap() {
                assert_eq!(status.exit_code(), 7);
                break;
            }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
