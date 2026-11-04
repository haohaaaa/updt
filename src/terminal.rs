//! Opt-in PTY UI. The legacy interface stays independent of this entry point.
pub mod business;
pub mod execution;
mod mouse;
mod navigation;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use navigation::Panel;
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
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
    master: Option<Box<dyn MasterPty + Send>>,
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
            master: Some(pair.master),
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

    fn resize(&self, size: PtySize) -> io::Result<()> {
        if let Some(master) = &self.master {
            master.resize(size).map_err(io::Error::other)?;
        }
        Ok(())
    }

    fn close_output(&mut self) {
        self.writer.take();
        if let Some(master) = self.master.take() {
            // ConPTY may flush output while closing. Keep draining its reader
            // on the UI thread while this close waits in the background.
            std::thread::spawn(move || drop(master));
        }
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
        if let Err(err) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            let _ = execute!(
                io::stdout(),
                DisableMouseCapture,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self)
    }
}
impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
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

fn content_regions(pane: Rect, workflow: bool) -> (Rect, Rect) {
    let header_height = if workflow && pane.height >= 8 { 4 } else { 0 };
    let parts =
        Layout::vertical([Constraint::Length(header_height), Constraint::Min(1)]).split(pane);
    (parts[0], parts[1])
}

fn sidebar_regions(sidebar: Rect, workflow: bool) -> (Rect, Rect, Rect) {
    let parts = Layout::vertical([Constraint::Length(7), Constraint::Min(1)]).split(sidebar);
    let session_height = if workflow && parts[1].height >= 6 {
        6.min(parts[1].height - 3)
    } else {
        0
    };
    let lists =
        Layout::vertical([Constraint::Min(3), Constraint::Length(session_height)]).split(parts[1]);
    (parts[0], lists[0], lists[1])
}

fn session_list_offset(view: &View, area: Rect) -> usize {
    view.session_offset.min(
        view.session_list
            .len()
            .saturating_sub(usize::from(Block::bordered().inner(area).height)),
    )
}

fn target_list_offset(view: &View, area: Rect) -> usize {
    view.selected_target
        .saturating_add(1)
        .saturating_sub(usize::from(Block::bordered().inner(area).height))
}

fn pty_size(area: Rect, workflow: bool) -> PtySize {
    let (_, pane, _) = regions(area);
    let (_, pane) = content_regions(pane, workflow);
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
    let progress = demo.then(|| {
        business::Progress::new(
            &[],
            "演示",
            &chrono::Local::now().format("%H:%M:%S").to_string(),
        )
    });
    run_session(command, if demo { "交互演示" } else { shell }, progress)
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
    let workflow = progress.is_some();
    let mut sessions = if workflow {
        Some(execution::Sessions::new()?)
    } else {
        None
    };
    if let Some(sessions) = &sessions {
        sessions.configure(&mut command);
    }
    let mut view = View::default();
    let mut size = pty_size(Rect::new(0, 0, width, height), workflow);
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
    let mut worker_status = None;
    let mut worker_eof = false;
    let mut confirm_exit = false;
    let mut rendered_view = None;
    loop {
        for _ in 0..64 {
            match session.events.as_ref().unwrap().try_recv() {
                Ok(PtyEvent::Output(bytes)) => {
                    let before = parser.screen().contents();
                    parser.process(&bytes);
                    if view.follow
                        && before != parser.screen().contents()
                        && let Some(sessions) = &mut sessions
                    {
                        sessions.active = None;
                    }
                }
                Ok(PtyEvent::Error(err)) => return Err(err),
                Err(TryRecvError::Disconnected) => {
                    worker_eof = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if let Some(sessions) = &mut sessions {
            sessions.poll(size, view.follow)?;
        }
        let responses = std::mem::take(&mut parser.callbacks_mut().bytes);
        if !responses.is_empty() && worker_status.is_none() {
            session.write(&responses)?;
        }
        if worker_status.is_none() {
            worker_status = session
                .child
                .try_wait()?
                .map(|status| status.exit_code() as i32);
            if worker_status.is_some() {
                session.close_output();
            }
        }
        if exit_code.is_none() && worker_eof {
            exit_code = worker_status;
            if let (Some(code), Some(progress)) = (exit_code, &mut parser.callbacks_mut().progress)
            {
                progress.finish(code);
            }
        }
        let active = sessions.as_ref().and_then(execution::Sessions::active);
        view.target_count = parser
            .callbacks()
            .progress
            .as_ref()
            .map_or(4, |p| p.targets.len());
        let (sidebar, _, _) = regions(terminal.get_frame().area());
        if (sidebar.width == 0 && view.panel != Panel::Output)
            || (view.panel == Panel::Sessions
                && (!workflow || sidebar_regions(sidebar, workflow).2.height <= 2))
        {
            view.panel = Panel::Output;
            view.focused = false;
        }
        view.session = active.map(|pane| SessionView {
            label: format!("#{} {}", pane.id, pane.label),
            code: pane.code,
            interactive: pane.accepts_input(),
        });
        if let Some(sessions) = &sessions {
            view.session_list = std::iter::once(Line::from("流程 · 确认与汇总"))
                .chain(sessions.panes.iter().map(|pane| {
                    let (status, color) = match pane.code {
                        None => ("运行", Color::Cyan),
                        Some(0) => ("完成", Color::Green),
                        Some(_) => ("完成", Color::Red),
                    };
                    Line::from(vec![
                        Span::raw(format!("#{} ", pane.id)),
                        Span::styled(status, Style::default().fg(color)),
                        Span::raw(format!(" {}", pane.label)),
                    ])
                }))
                .collect();
            let selected = sessions
                .panes
                .iter()
                .position(|pane| Some(pane.id) == sessions.active)
                .map_or(0, |i| i + 1);
            if selected != view.selected_session {
                let (sidebar, _, _) = regions(terminal.get_frame().area());
                let (_, _, list) = sidebar_regions(sidebar, workflow);
                let rows = usize::from(Block::bordered().inner(list).height).max(1);
                if selected < view.session_offset {
                    view.session_offset = selected;
                } else if selected >= view.session_offset + rows {
                    view.session_offset = selected + 1 - rows;
                }
            }
            view.selected_session = selected;
        }
        let current_view = (
            sessions.as_ref().and_then(|sessions| sessions.active),
            view.history,
            exit_code,
            view.session.as_ref().and_then(|session| session.code),
        );
        if rendered_view != Some(current_view) {
            // ConPTY can retain cells beside wide characters when unrelated
            // terminal screens are diffed. Switching screens needs a full redraw.
            terminal.clear()?;
            rendered_view = Some(current_view);
        }
        terminal.draw(|frame| {
            render(
                frame,
                active.map_or_else(|| parser.screen(), |pane| pane.parser.screen()),
                label,
                exit_code,
                confirm_exit,
                parser.callbacks().progress.as_ref(),
                &view,
            )
        })?;
        if !event::poll(Duration::from_millis(30))? {
            continue;
        }
        let input = event::read()?;
        let input = if let Event::Mouse(event) = input {
            match mouse::action(
                event,
                terminal.get_frame().area(),
                active_screen(&parser, &sessions),
                &view,
                workflow,
                confirm_exit,
                exit_code.is_none(),
            ) {
                Some(mouse::Action::Key(code)) => {
                    if code == KeyCode::Char('q') {
                        view.focused = false;
                    }
                    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
                }
                Some(mouse::Action::SelectSession(index)) => {
                    if let Some(sessions) = &mut sessions {
                        sessions.active = index.checked_sub(1).map(|i| sessions.panes[i].id);
                        view.history = false;
                        view.follow = false;
                        view.panel = Panel::Sessions;
                        view.focused = true;
                    }
                    continue;
                }
                Some(mouse::Action::SelectTarget(index)) => {
                    view.selected_target = index;
                    view.panel = Panel::Targets;
                    view.focused = true;
                    continue;
                }
                Some(mouse::Action::Focus(panel)) => {
                    view.panel = panel;
                    view.focused = true;
                    continue;
                }
                Some(mouse::Action::ScrollOutput(up)) => {
                    scroll_output(&mut parser, &mut sessions, &mut view, size, up, 3);
                    continue;
                }
                Some(mouse::Action::ScrollSessions(up)) => {
                    view.panel = Panel::Sessions;
                    view.focused = true;
                    view.follow = false;
                    view.session_offset = if up {
                        view.session_offset.saturating_sub(3)
                    } else {
                        view.session_offset.saturating_add(3)
                    };
                    let (sidebar, _, _) = regions(terminal.get_frame().area());
                    view.session_offset =
                        session_list_offset(&view, sidebar_regions(sidebar, workflow).2);
                    continue;
                }
                Some(mouse::Action::ScrollTargets(up)) => {
                    view.panel = Panel::Targets;
                    view.focused = true;
                    view.selected_target = if up {
                        view.selected_target.saturating_sub(1)
                    } else {
                        view.selected_target.saturating_add(1)
                    }
                    .min(view.target_count.saturating_sub(1));
                    continue;
                }
                Some(mouse::Action::Forward(bytes)) => {
                    view.panel = Panel::Output;
                    view.focused = true;
                    write_active(&mut session, &mut sessions, &bytes)?;
                    continue;
                }
                None => continue,
            }
        } else {
            input
        };
        match input {
            Event::Resize(width, height) => {
                size = pty_size(Rect::new(0, 0, width, height), workflow);
                session.resize(size)?;
                if let Some(sessions) = &mut sessions {
                    sessions.resize(size)?;
                }
                parser.set_size(nonzero(size.rows), nonzero(size.cols));
            }
            Event::Paste(text)
                if exit_code.is_none()
                    && view.focused
                    && view.panel == Panel::Output
                    && !confirm_exit
                    && !view.history
                    && active_screen(&parser, &sessions).scrollback() == 0 =>
            {
                let paste = active_screen(&parser, &sessions).bracketed_paste();
                let bytes = if paste {
                    format!("\x1b[200~{text}\x1b[201~").into_bytes()
                } else {
                    text.into_bytes()
                };
                write_active(&mut session, &mut sessions, &bytes)?;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc {
                    confirm_exit = false;
                    view.focused = false;
                } else if confirm_exit {
                    match key.code {
                        KeyCode::Char('y' | 'Y') => return Ok(130),
                        KeyCode::Char('n' | 'N') => confirm_exit = false,
                        _ => {}
                    }
                } else if workflow && key.code == KeyCode::F(2) {
                    view.history = !view.history;
                    view.follow = !view.history;
                    view.panel = Panel::Output;
                } else if workflow && matches!(key.code, KeyCode::F(5) | KeyCode::F(6)) {
                    view.history = false;
                    view.follow = false;
                    if let Some(sessions) = &mut sessions {
                        sessions.cycle(key.code == KeyCode::F(6));
                    }
                } else if key.code == KeyCode::F(4) {
                    view = View::default();
                    view.focused = true;
                    if let Some(sessions) = &mut sessions {
                        sessions.live();
                        if let Some(pane) = sessions.active_mut() {
                            pane.parser.screen_mut().set_scrollback(0);
                        }
                    }
                    parser.screen_mut().set_scrollback(0);
                } else if let Some(action) =
                    navigation::handle(key, &mut view, terminal.get_frame().area(), workflow)
                {
                    match action {
                        navigation::Action::Exit => {
                            if let Some(code) = exit_code {
                                return Ok(code);
                            }
                            confirm_exit = true;
                        }
                        navigation::Action::SelectSession(index) => {
                            if let Some(sessions) = &mut sessions {
                                sessions.active =
                                    index.checked_sub(1).map(|i| sessions.panes[i].id);
                                view.history = false;
                                view.follow = false;
                            }
                        }
                        navigation::Action::Handled => {
                            if view.focused && view.panel == Panel::Sessions {
                                view.follow = false;
                            }
                        }
                    }
                } else if key.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                {
                    scroll_output(
                        &mut parser,
                        &mut sessions,
                        &mut view,
                        size,
                        key.code == KeyCode::PageUp,
                        usize::from(size.rows),
                    );
                } else if view.history || active_screen(&parser, &sessions).scrollback() > 0 {
                    // Browsing is read-only: never send an unseen answer to a prompt.
                    match key.code {
                        KeyCode::Char('k') | KeyCode::Up => {
                            scroll_output(&mut parser, &mut sessions, &mut view, size, true, 1);
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            scroll_output(&mut parser, &mut sessions, &mut view, size, false, 1);
                        }
                        _ => {}
                    }
                } else if exit_code.is_none() {
                    parser.screen_mut().set_scrollback(0);
                    let bytes =
                        encode_key(key, active_screen(&parser, &sessions).application_cursor());
                    write_active(&mut session, &mut sessions, &bytes)?;
                }
            }
            _ => {}
        }
    }
}

fn scroll_output(
    parser: &mut vt100::Parser<Responses>,
    sessions: &mut Option<execution::Sessions>,
    view: &mut View,
    size: PtySize,
    up: bool,
    lines: usize,
) {
    let offset = if view.history {
        view.offset
    } else {
        active_screen(parser, sessions).scrollback()
    };
    let offset = if up {
        offset.saturating_add(lines)
    } else {
        offset.saturating_sub(lines)
    };
    if view.history {
        let count = parser
            .callbacks()
            .progress
            .as_ref()
            .map_or(0, |p| history_lines(p, size.cols).len());
        view.offset = offset.min(count.saturating_sub(usize::from(size.rows)));
    } else {
        let screen = sessions
            .as_mut()
            .and_then(execution::Sessions::active_mut)
            .map_or_else(|| parser.screen_mut(), |pane| pane.parser.screen_mut());
        screen.set_scrollback(offset);
        view.follow = screen.scrollback() == 0;
    }
}

fn active_screen<'a>(
    parser: &'a vt100::Parser<Responses>,
    sessions: &'a Option<execution::Sessions>,
) -> &'a vt100::Screen {
    sessions
        .as_ref()
        .and_then(execution::Sessions::active)
        .map_or_else(|| parser.screen(), |pane| pane.parser.screen())
}

fn write_active(
    session: &mut PtySession,
    sessions: &mut Option<execution::Sessions>,
    bytes: &[u8],
) -> io::Result<()> {
    if let Some(pane) = sessions.as_mut().and_then(execution::Sessions::active_mut) {
        pane.write(bytes)
    } else if sessions
        .as_ref()
        .is_some_and(|sessions| sessions.panes.iter().any(|pane| pane.code.is_none()))
    {
        // The workflow is waiting for another pane. Do not queue keystrokes
        // there that could accidentally answer its next prompt.
        Ok(())
    } else {
        session.write(bytes)
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

struct View {
    panel: Panel,
    focused: bool,
    history: bool,
    offset: usize,
    follow: bool,
    session: Option<SessionView>,
    session_list: Vec<Line<'static>>,
    selected_session: usize,
    session_offset: usize,
    selected_target: usize,
    target_count: usize,
}

struct SessionView {
    label: String,
    code: Option<i32>,
    interactive: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            panel: Panel::Output,
            focused: false,
            history: false,
            offset: 0,
            follow: true,
            session: None,
            session_list: Vec::new(),
            selected_session: 0,
            session_offset: 0,
            selected_target: 0,
            target_count: 0,
        }
    }
}

fn history_lines(progress: &business::Progress, width: u16) -> Vec<String> {
    let mut lines = Vec::new();
    for record in &progress.history {
        let mut line = String::new();
        let mut columns = 0;
        for ch in record.chars() {
            let char_width = Span::raw(ch.to_string()).width();
            if columns + char_width > usize::from(width.max(1)) && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                columns = 0;
            }
            line.push(ch);
            columns += char_width;
        }
        lines.push(line);
    }
    lines
}

fn render(
    frame: &mut ratatui::Frame<'_>,
    screen: &vt100::Screen,
    shell: &str,
    exit_code: Option<i32>,
    confirm_exit: bool,
    progress: Option<&business::Progress>,
    view: &View,
) {
    let (sidebar, pane, footer) = regions(frame.area());
    if sidebar.width > 0 {
        let (header_area, target_area, session_area) = sidebar_regions(sidebar, progress.is_some());
        let header = if let Some(progress) = progress {
            format!(
                "开始: {}\n策略: {}\n阶段: {}\nF2 / 点击: 流程记录",
                progress.start_time, progress.profile, progress.phase,
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
            header_area,
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
        let offset = target_list_offset(view, target_area);
        let rows = usize::from(Block::bordered().inner(target_area).height);
        let mut target_state = ratatui::widgets::ListState::default().with_selected(
            (view.panel == Panel::Targets).then_some(view.selected_target.saturating_sub(offset)),
        );
        frame.render_stateful_widget(
            List::new(targets.into_iter().skip(offset).take(rows))
                .block(
                    Block::default()
                        .title(if progress.is_some() {
                            "目标"
                        } else {
                            "目标（占位）"
                        })
                        .borders(Borders::ALL)
                        .border_style(navigation::border(view, Panel::Targets)),
                )
                .highlight_style(Style::default().bg(Color::DarkGray)),
            target_area,
            &mut target_state,
        );
        if !view.session_list.is_empty() {
            let offset = session_list_offset(view, session_area);
            let rows = usize::from(Block::bordered().inner(session_area).height);
            let list = List::new(
                view.session_list
                    .iter()
                    .skip(offset)
                    .take(rows)
                    .enumerate()
                    .map(|(index, line)| {
                        let style = if index + offset == view.selected_session {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default()
                        };
                        ListItem::new(line.clone()).style(style)
                    }),
            )
            .block(
                Block::bordered()
                    .title("会话 · 点击 / 滚轮")
                    .border_style(navigation::border(view, Panel::Sessions)),
            )
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");
            let selected = view
                .selected_session
                .checked_sub(offset)
                .filter(|index| *index < rows);
            let mut selected = ratatui::widgets::ListState::default().with_selected(selected);
            frame.render_stateful_widget(list, session_area, &mut selected);
        } else if let Some(progress) = progress {
            frame.render_widget(
                Paragraph::new(
                    progress
                        .history
                        .back()
                        .map_or("等待检查结果", String::as_str),
                )
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title("最新消息 · F2 查看记录")),
                session_area,
            );
        }
    }
    let (command_area, pane) = content_regions(pane, progress.is_some());
    if let Some(progress) = progress {
        let command = view
            .session
            .as_ref()
            .map(|session| session.label.as_str())
            .unwrap_or(match progress.phase {
                "检查" => "并行检查中 · 完成后显示检查结果",
                "确认" => "请在下方终端确认升级目标",
                "完成" | "中断或失败" => "本次流程已结束",
                _ => "流程确认与汇总",
            });
        let state = view.session.as_ref().map_or_else(
            || progress.phase.to_string(),
            |session| {
                session.code.map_or_else(
                    || {
                        if session.interactive {
                            "运行中 · 聚焦终端后输入"
                        } else {
                            "查询中 · 输出可供业务解析"
                        }
                        .to_string()
                    },
                    |code| format!("已结束 · exit {code}"),
                )
            },
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(command.to_string(), Style::default().fg(Color::Cyan)),
                Line::from(state),
            ])
            .block(Block::bordered().title("终端会话 · F5 上一个 / F6 下一个")),
            command_area,
        );
    }
    let title = if view.history {
        "流程记录 · 只读 · F2 返回输出".to_string()
    } else if screen.scrollback() > 0 {
        format!("历史输出 · 上方 {} 行 · F4 返回实时", screen.scrollback())
    } else if let Some(session) = &view.session {
        if let Some(code) = session.code {
            format!("命令输出 · exit {code} · 只读")
        } else if session.interactive {
            "命令输出 · 聚焦终端后输入".to_string()
        } else {
            "查询输出 · 只读".to_string()
        }
    } else {
        exit_code.map_or_else(
            || {
                if progress.is_some_and(|p| p.phase == "检查") {
                    "检查结果 · 正在检查，进度见左侧".to_string()
                } else {
                    "终端输出 · Enter / 点击聚焦".to_string()
                }
            },
            |code| format!("终端已退出 · exit {code}"),
        )
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(navigation::border(view, Panel::Output));
    let inner = block.inner(pane);
    frame.render_widget(block, pane);
    if view.history {
        if let Some(progress) = progress {
            let lines = history_lines(progress, inner.width);
            let start = lines
                .len()
                .saturating_sub(usize::from(inner.height))
                .saturating_sub(view.offset);
            let lines = lines
                .into_iter()
                .skip(start)
                .take(usize::from(inner.height))
                .map(Line::from)
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(lines), inner);
        }
    } else {
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
    }
    if view.focused
        && view.panel == Panel::Output
        && !view.history
        && view
            .session
            .as_ref()
            .is_none_or(|session| session.interactive)
        && exit_code.is_none()
        && !confirm_exit
        && screen.scrollback() == 0
        && !screen.hide_cursor()
    {
        let (row, col) = screen.cursor_position();
        if row < inner.height && col < inner.width {
            frame.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
    let help = if confirm_exit {
        "结束当前终端进程并退出？ Y: 确认  N/Esc: 返回"
    } else if !view.focused {
        "导航 · hjkl/方向键: 切换面板 · Enter: 聚焦 · q: 退出"
    } else if view.panel != Panel::Output {
        "面板已聚焦 · hjkl/方向键: 切换条目 · Esc: 退焦 · 退焦后 q: 退出"
    } else if view.history || screen.scrollback() > 0 {
        "只读回看 · 滚轮/Shift+PgUp/PgDn: 翻页 · F4: 实时 · Esc: 退焦"
    } else if exit_code.is_some() && progress.is_some_and(business::Progress::has_deferred) {
        "流程已结束 · Esc 退焦后 q: 退出并执行收尾更新"
    } else if exit_code.is_some() {
        "终端已结束 · Esc: 退焦 · 退焦后 q: 退出 · 滚轮: 回看输出"
    } else {
        "终端已聚焦 · Esc: 退焦 · Ctrl+C: 中断 · 滚轮: 回看输出"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(if confirm_exit {
            Color::Yellow
        } else {
            Color::DarkGray
        })),
        Rect {
            height: footer.height.min(1),
            ..footer
        },
    );
    for (area, label, _) in mouse::footer_buttons(footer, progress.is_some(), confirm_exit) {
        frame.render_widget(
            Paragraph::new(label).style(
                Style::default()
                    .fg(if confirm_exit {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })
                    .add_modifier(Modifier::UNDERLINED),
            ),
            area,
        );
    }
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

pub async fn run_demo() -> i32 {
    if execution::configured() {
        business::enable_worker(std::process::id());
        return match execution::run_demo().await {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("终端接口演示失败: {err}");
                1
            }
        };
    }
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
    println!("\x1b[32m交互完成。可以调整窗口大小，Esc 退焦后按 q 返回。\x1b[0m\r");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::time::Instant;

    #[test]
    fn output_and_activity_scroll_independently_and_clamp_at_both_ends() {
        let mut progress = business::Progress::new(&[], "test", "now");
        progress.history = (0..20).map(|i| format!("event {i}")).collect();
        let mut parser = vt100::Parser::new_with_callbacks(
            nonzero(4),
            nonzero(12),
            50,
            Responses {
                bytes: Vec::new(),
                progress: Some(progress),
            },
        );
        for i in 0..20 {
            parser.process(format!("line {i}\r\n").as_bytes());
        }
        let size = PtySize {
            rows: 4,
            cols: 12,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut view = View::default();
        let mut sessions = None;
        scroll_output(&mut parser, &mut sessions, &mut view, size, true, 3);
        assert_eq!(parser.screen().scrollback(), 3);
        assert!(!view.follow);
        view.history = true;
        scroll_output(
            &mut parser,
            &mut sessions,
            &mut view,
            size,
            true,
            usize::MAX,
        );
        assert_eq!(view.offset, 16);
        assert_eq!(parser.screen().scrollback(), 3);
        scroll_output(
            &mut parser,
            &mut sessions,
            &mut view,
            size,
            false,
            usize::MAX,
        );
        assert_eq!(view.offset, 0);
        view.history = false;
        scroll_output(
            &mut parser,
            &mut sessions,
            &mut view,
            size,
            true,
            usize::MAX,
        );
        assert!(parser.screen().scrollback() > 3);
        scroll_output(
            &mut parser,
            &mut sessions,
            &mut view,
            size,
            false,
            usize::MAX,
        );
        assert_eq!(parser.screen().scrollback(), 0);
        assert!(view.follow);
    }

    #[test]
    fn workflow_layout_keeps_commands_messages_and_pty_output_separate() {
        let mut progress = business::Progress::new(&["npm".into()], "windows", "now");
        progress.apply(&[b"777", b"updt", b"command", br#""npm update -g""#]);
        progress.apply(&[b"777", b"updt", b"notice", br#""workflow message""#]);
        let mut parser = vt100::Parser::new(nonzero(20), nonzero(68), 10);
        parser.process(b"package output\r\nContinue? [y/n]");
        for (width, height) in [(1, 1), (20, 4), (50, 12), (100, 24)] {
            for history in [false, true] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| {
                        render(
                            frame,
                            parser.screen(),
                            "workflow",
                            None,
                            false,
                            Some(&progress),
                            &View {
                                history,
                                offset: 0,
                                session: Some(SessionView {
                                    label: "npm update -g".into(),
                                    code: None,
                                    interactive: true,
                                }),
                                ..View::default()
                            },
                        )
                    })
                    .unwrap();
                let size = pty_size(Rect::new(0, 0, width, height), true);
                assert!(size.rows > 0 && size.cols > 0);
                if width == 100 {
                    let buffer = terminal.backend().buffer();
                    let row = |y| {
                        (31..99)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>()
                    };
                    assert!(row(1).contains("npm update -g"));
                    let content = (5..21).map(row).collect::<String>();
                    assert_eq!(content.contains("package output"), !history);
                    assert_eq!(content.contains("workflow message"), history);
                }
            }
        }
    }

    #[test]
    fn activity_wraps_long_commands_and_chinese_without_losing_text() {
        let mut progress = business::Progress::new(&[], "windows", "now");
        progress
            .history
            .push_back("中文 command --long-argument".into());
        let lines = history_lines(&progress, 8);
        assert_eq!(lines.concat(), "中文 command --long-argument");
        assert!(
            lines
                .iter()
                .all(|line| Line::from(line.as_str()).width() <= 8)
        );
    }

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
                    &View::default(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 8)].symbol(), "n");
        assert_eq!(buffer[(31, 5)].symbol(), "c");
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
            .draw(|frame| {
                render(
                    frame,
                    parser.screen(),
                    "test shell",
                    None,
                    false,
                    None,
                    &View::default(),
                )
            })
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
                .draw(|frame| {
                    render(
                        frame,
                        parser.screen(),
                        "shell",
                        None,
                        false,
                        None,
                        &View::default(),
                    )
                })
                .unwrap();
            let size = pty_size(Rect::new(0, 0, width, height), false);
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
                session.resize(size).unwrap();
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
