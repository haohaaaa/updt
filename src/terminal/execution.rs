//! Internal command API and the UI-owned command sessions.
//!
//! Capture commands preserve stdout/stderr for parsers. Interactive commands
//! have a native PTY and return the merged terminal byte stream. Both execute
//! once; the renderer observes the very same output returned to the caller.
use super::{PtyEvent, PtySession, Responses, SCROLLBACK, nonzero};
use portable_pty::{CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    time::Duration,
};

const ADDRESS: &str = "UPDT_TERMINAL_ADDRESS";
const TOKEN: &str = "UPDT_TERMINAL_TOKEN";
const MAX_FRAME: usize = 128 * 1024 * 1024;
const MAX_SESSIONS: usize = 32;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Mode {
    Capture,
    Interactive,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommandResult {
    pub code: i32,
    // Only capture mode separates stdout and stderr.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    // Interactive mode preserves ANSI codes, cursor movement and input echo.
    pub terminal_output: Vec<u8>,
}

impl CommandResult {
    pub fn into_output(self) -> Output {
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(self.code as u32)
        };
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(if self.code < 0 { 9 } else { self.code << 8 })
        };
        Output {
            status,
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    program: String,
    args: Vec<String>,
    label: String,
    mode: Mode,
    cwd: std::path::PathBuf,
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Output(CommandResult),
    Error(String),
}

pub fn available() -> bool {
    super::business::is_worker() && configured()
}

pub fn configured() -> bool {
    std::env::var_os(ADDRESS).is_some()
}

pub async fn run_demo() -> io::Result<()> {
    super::business::phase("checking");
    println!("终端执行接口演示：只运行演示命令，不更新软件。");
    #[cfg(windows)]
    let (program, query, interactive) = (
        "powershell.exe",
        vec![
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "[Console]::Out.Write('{\"demo\":true}'); [Console]::Error.Write('stderr demo'); exit 0",
        ],
        vec![
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::Write('输入任意文字后按 Enter: '); $answer = [Console]::ReadLine(); [Console]::WriteLine('收到: ' + $answer); exit 0",
        ],
    );
    #[cfg(not(windows))]
    let (program, query, interactive) = (
        "/bin/sh",
        vec!["-c", "printf '{\"demo\":true}'; printf 'stderr demo' >&2"],
        vec![
            "-c",
            "printf '输入任意文字后按 Enter: '; IFS= read -r answer; printf '收到: %s\\n' \"$answer\"",
        ],
    );
    let result = execute(program, &query, Mode::Capture).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&result.stdout).map_err(io::Error::other)?;
    if json["demo"] != true || result.stderr != b"stderr demo" || result.code != 0 {
        return Err(io::Error::other("捕获结果与演示预期不一致"));
    }
    super::business::notice("内部 API 已收到独立的 JSON stdout、stderr 和 exit 0。");
    super::business::phase("upgrading");
    let result = execute(program, &interactive, Mode::Interactive).await?;
    println!(
        "内部 API 收到交互命令结果：exit {}，{} 字节终端输出。",
        result.code,
        result.terminal_output.len()
    );
    super::business::phase("summary");
    if result.code != 0 {
        return Err(io::Error::other("交互命令没有正常完成"));
    }
    println!("演示完成。F5/F6 可回看两条命令，Esc 退焦后 q 退出。");
    Ok(())
}

/// Submit a command to the terminal owner and await its actual result.
/// Program and arguments stay separate: there is no simulated keyboard typing.
pub async fn execute(program: &str, args: &[&str], mode: Mode) -> io::Result<CommandResult> {
    let mut prepared = crate::command::command(program).await;
    prepared.args(args);
    let prepared = prepared.as_std();
    let request = Request {
        token: std::env::var(TOKEN).map_err(io::Error::other)?,
        program: prepared.get_program().to_string_lossy().into_owned(),
        args: prepared
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        label: crate::command::command_label(program, args),
        mode,
        cwd: std::env::current_dir()?,
    };
    let address: SocketAddr = std::env::var(ADDRESS)
        .map_err(io::Error::other)?
        .parse()
        .map_err(io::Error::other)?;
    if !address.ip().is_loopback() {
        return Err(io::Error::other("terminal owner must be local"));
    }
    super::business::command_started(&request.label);
    let result = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
        write_frame(&mut stream, &request)?;
        match read_frame(&mut stream, MAX_FRAME)? {
            Reply::Output(result) => Ok(result),
            Reply::Error(message) => Err(io::Error::other(message)),
        }
    })
    .await
    .map_err(io::Error::other)?;
    super::business::command_finished(&match &result {
        Ok(result) => format!("exit {}", result.code),
        Err(err) => format!("执行失败: {err}"),
    });
    result
}

fn write_frame(stream: &mut TcpStream, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("command result exceeds 128 MiB"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}

fn read_frame<T: serde::de::DeserializeOwned>(
    stream: &mut TcpStream,
    limit: usize,
) -> io::Result<T> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(io::Error::other("terminal message too large"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn reply(mut stream: TcpStream, result: io::Result<CommandResult>) {
    // Writing a large result must not block terminal rendering or input.
    std::thread::spawn(move || {
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let response = match result {
            Ok(output) => Reply::Output(output),
            Err(err) => Reply::Error(err.to_string()),
        };
        if let Err(err) = write_frame(&mut stream, &response) {
            let _ = write_frame(&mut stream, &Reply::Error(err.to_string()));
        }
    });
}

enum Event {
    Request(Request, TcpStream),
    Output(u64, Vec<u8>),
    Finished(u64, Result<i32, String>),
}

struct CaptureTask {
    cancel: Arc<AtomicBool>,
}
impl Drop for CaptureTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(super) struct Pane {
    pub id: u64,
    pub label: String,
    pub mode: Mode,
    pub parser: vt100::Parser<Responses>,
    pub code: Option<i32>,
    pty: Option<PtySession>,
    capture: Option<CaptureTask>,
    reply: Option<TcpStream>,
    bytes: Vec<u8>,
    eof: bool,
    exited: Option<i32>,
}

impl Pane {
    pub fn accepts_input(&self) -> bool {
        self.mode == Mode::Interactive && self.code.is_none()
    }
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.accepts_input()
            && let Some(pty) = &mut self.pty
        {
            pty.write(bytes)?;
        }
        Ok(())
    }
    fn finish(&mut self, result: Result<i32, String>) {
        let text = match &result {
            Ok(code) => format!("\r\n\x1b[0m\x1b[2m[exit {code}]\x1b[0m\r\n"),
            Err(err) => format!("\r\n\x1b[0m执行失败: {err}\r\n"),
        };
        self.parser.process(text.as_bytes());
        self.code = Some(result.unwrap_or(1));
        self.capture.take();
    }
}

/// Owns command processes and parsers independently from the workflow PTY.
pub(super) struct Sessions {
    address: SocketAddr,
    token: String,
    receiver: Receiver<Event>,
    sender: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    pub panes: Vec<Pane>,
    pub active: Option<u64>,
    next_id: u64,
}

impl Sessions {
    pub fn new() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        // Each RandomState obtains independently seeded random keys from std.
        let token = (0..4)
            .map(|i| format!("{:016x}", RandomState::new().hash_one(i)))
            .collect::<String>();
        let (sender, receiver) = mpsc::sync_channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let events = sender.clone();
        let secret = token.clone();
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // macOS may inherit the listener's nonblocking mode.
                        // Frame reads here must wait for a complete request.
                        if stream.set_nonblocking(false).is_err()
                            || stream
                                .set_read_timeout(Some(Duration::from_secs(10)))
                                .is_err()
                        {
                            continue;
                        }
                        // One local workflow owns the endpoint. Do not let an
                        // unauthenticated peer submit commands to the UI.
                        if let Ok(request) = read_frame::<Request>(&mut stream, 1024 * 1024)
                            && request.token == secret
                        {
                            let _ = stream.set_read_timeout(None);
                            if events.send(Event::Request(request, stream)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            token,
            receiver,
            sender,
            stop,
            panes: Vec::new(),
            active: None,
            next_id: 1,
        })
    }

    pub fn configure(&self, command: &mut CommandBuilder) {
        command.env(ADDRESS, self.address.to_string());
        command.env(TOKEN, &self.token);
    }

    pub fn active(&self) -> Option<&Pane> {
        self.panes.iter().find(|pane| Some(pane.id) == self.active)
    }
    pub fn active_mut(&mut self) -> Option<&mut Pane> {
        self.panes
            .iter_mut()
            .find(|pane| Some(pane.id) == self.active)
    }

    pub fn cycle(&mut self, forward: bool) {
        let count = self.panes.len() + 1;
        let current = self
            .panes
            .iter()
            .position(|pane| Some(pane.id) == self.active)
            .map_or(0, |i| i + 1);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.active = next.checked_sub(1).map(|i| self.panes[i].id);
    }

    pub fn live(&mut self) {
        self.active = self
            .panes
            .iter()
            .rev()
            .find(|pane| pane.accepts_input())
            .map(|pane| pane.id);
    }

    pub fn resize(&mut self, size: PtySize) -> io::Result<()> {
        for pane in &mut self.panes {
            if let Some(pty) = &pane.pty {
                pty.resize(size)?;
            }
            pane.parser.set_size(nonzero(size.rows), nonzero(size.cols));
        }
        Ok(())
    }

    pub fn poll(&mut self, size: PtySize, follow: bool) -> io::Result<()> {
        for _ in 0..128 {
            match self.receiver.try_recv() {
                Ok(Event::Request(request, stream)) => self.start(request, stream, size, follow),
                Ok(Event::Output(id, bytes)) => {
                    if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
                        // Pipe output uses LF; the terminal view needs CR+LF. The
                        // caller's captured bytes remain untouched.
                        let mut display = Vec::with_capacity(bytes.len());
                        for byte in bytes {
                            if byte == b'\n' {
                                display.push(b'\r');
                            }
                            display.push(byte);
                        }
                        pane.parser.process(&display);
                    }
                }
                Ok(Event::Finished(id, result)) => {
                    if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
                        pane.finish(result);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        for pane in &mut self.panes {
            let Some(pty) = &mut pane.pty else {
                continue;
            };
            for _ in 0..64 {
                match pty.events.as_ref().unwrap().try_recv() {
                    Ok(PtyEvent::Output(bytes)) => {
                        pane.parser.process(&bytes);
                        pane.bytes.extend_from_slice(&bytes);
                    }
                    Ok(PtyEvent::Error(err)) => {
                        if let Some(stream) = pane.reply.take() {
                            reply(stream, Err(err));
                        }
                        let _ = pty.child.kill();
                        pane.eof = true;
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        pane.eof = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            let responses = std::mem::take(&mut pane.parser.callbacks_mut().bytes);
            if !responses.is_empty() {
                pty.write(&responses)?;
            }
            if pane.exited.is_none()
                && let Some(status) = pty.child.try_wait()?
            {
                // Preserve the observed command exit code before closing
                // ConPTY. Teardown can otherwise replace it with CTRL_CLOSE.
                pane.exited = Some(status.exit_code() as i32);
                pty.close_output();
            }
            if let Some(code) = pane.exited
                && pane.eof
            {
                if let Some(stream) = pane.reply.take() {
                    reply(
                        stream,
                        Ok(CommandResult {
                            code,
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                            terminal_output: std::mem::take(&mut pane.bytes),
                        }),
                    );
                }
                pane.pty.take();
                pane.finish(Ok(code));
            }
        }
        Ok(())
    }

    fn start(&mut self, request: Request, stream: TcpStream, size: PtySize, follow: bool) {
        if self.panes.len() >= MAX_SESSIONS
            && let Some(index) = self.panes.iter().position(|pane| pane.code.is_some())
        {
            let removed = self.panes.remove(index);
            if self.active == Some(removed.id) {
                self.active = None;
            }
        }
        if self.panes.len() >= MAX_SESSIONS {
            reply(
                stream,
                Err(io::Error::other("all 32 terminal sessions are busy")),
            );
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut pane = Pane {
            id,
            label: request.label.clone(),
            mode: request.mode,
            parser: vt100::Parser::new_with_callbacks(
                nonzero(size.rows),
                nonzero(size.cols),
                SCROLLBACK,
                Responses::default(),
            ),
            code: None,
            pty: None,
            capture: None,
            reply: None,
            bytes: Vec::new(),
            eof: false,
            exited: None,
        };
        let label: String = request.label.chars().filter(|c| !c.is_control()).collect();
        pane.parser
            .process(format!("\x1b[1;36m$ {label}\x1b[0m\r\n").as_bytes());
        match request.mode {
            Mode::Interactive => {
                let mut command = CommandBuilder::new(&request.program);
                command.args(&request.args);
                command.cwd(&request.cwd);
                command.env("TERM", "xterm-256color");
                command.env("COLORTERM", "truecolor");
                match PtySession::spawn(command, size) {
                    Ok(pty) => {
                        pane.pty = Some(pty);
                        pane.reply = Some(stream);
                    }
                    Err(err) => {
                        pane.finish(Err(err.to_string()));
                        reply(stream, Err(err));
                    }
                }
            }
            Mode::Capture => {
                let cancel = Arc::new(AtomicBool::new(false));
                pane.capture = Some(CaptureTask {
                    cancel: cancel.clone(),
                });
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let result = capture(&request, id, &sender, &cancel);
                    let status = result
                        .as_ref()
                        .map(|result| result.code)
                        .map_err(ToString::to_string);
                    let _ = sender.send(Event::Finished(id, status));
                    reply(stream, result);
                });
            }
        }
        self.panes.push(pane);
        if follow {
            self.active = Some(id);
        }
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn capture(
    request: &Request,
    id: u64,
    events: &SyncSender<Event>,
    cancel: &AtomicBool,
) -> io::Result<CommandResult> {
    let mut command = std::process::Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(&request.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output_events = events.clone();
    let error_events = events.clone();
    let stdout = std::thread::spawn(move || read_capture(stdout, id, output_events));
    let stderr = std::thread::spawn(move || read_capture(stderr, id, error_events));
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout
        .join()
        .map_err(|_| io::Error::other("stdout reader failed"))??;
    let stderr = stderr
        .join()
        .map_err(|_| io::Error::other("stderr reader failed"))??;
    Ok(CommandResult {
        code: status.code().unwrap_or(-1),
        stdout,
        stderr,
        terminal_output: Vec::new(),
    })
}

fn read_capture(mut reader: impl Read, id: u64, events: SyncSender<Event>) -> io::Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        captured.extend_from_slice(&buffer[..count]);
        // If the UI has closed, continue draining so the child can terminate.
        let _ = events.send(Event::Output(id, buffer[..count].to_vec()));
    }
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn size() -> PtySize {
        PtySize {
            rows: 20,
            cols: 90,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    fn request(sessions: &Sessions, script: &str, mode: Mode) -> Request {
        #[cfg(windows)]
        let (program, args) = (
            "powershell.exe",
            vec!["-NoLogo", "-NoProfile", "-Command", script],
        );
        #[cfg(not(windows))]
        let (program, args) = ("/bin/sh", vec!["-c", script]);
        Request {
            token: sessions.token.clone(),
            program: program.into(),
            args: args.into_iter().map(str::to_string).collect(),
            label: format!("demo {mode:?}"),
            mode,
            cwd: std::env::current_dir().unwrap(),
        }
    }

    fn submit(sessions: &Sessions, request: Request) -> Receiver<Reply> {
        let address = sessions.address;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            write_frame(&mut stream, &request).unwrap();
            sender
                .send(read_frame(&mut stream, MAX_FRAME).unwrap())
                .unwrap();
        });
        receiver
    }

    fn wait_result(sessions: &mut Sessions, receiver: &Receiver<Reply>) -> CommandResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            sessions.poll(size(), true).unwrap();
            if let Ok(reply) = receiver.try_recv() {
                return match reply {
                    Reply::Output(result) => result,
                    Reply::Error(err) => panic!("{err}"),
                };
            }
            assert!(Instant::now() < deadline, "command timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn capture_api_and_panes_share_one_execution_without_mixing_parallel_output() {
        let mut sessions = Sessions::new().unwrap();
        #[cfg(windows)]
        let scripts = [
            "[Console]::Out.Write('{\"id\":1}'); [Console]::Error.Write('err-one'); exit 3",
            "[Console]::Out.Write('{\"id\":2}'); [Console]::Error.Write('err-two'); exit 0",
        ];
        #[cfg(not(windows))]
        let scripts = [
            "printf '{\"id\":1}'; printf err-one >&2; exit 3",
            "printf '{\"id\":2}'; printf err-two >&2; exit 0",
        ];
        let first = submit(&sessions, request(&sessions, scripts[0], Mode::Capture));
        let second = submit(&sessions, request(&sessions, scripts[1], Mode::Capture));
        let first = wait_result(&mut sessions, &first);
        let second = wait_result(&mut sessions, &second);
        sessions.poll(size(), true).unwrap();
        assert_eq!(first.stdout, br#"{"id":1}"#);
        assert_eq!(first.stderr, b"err-one");
        assert_eq!(first.code, 3);
        assert_eq!(second.stdout, br#"{"id":2}"#);
        assert_eq!(second.stderr, b"err-two");
        assert_eq!(second.code, 0);
        assert!(first.terminal_output.is_empty());
        let screens = sessions
            .panes
            .iter()
            .map(|pane| pane.parser.screen().contents())
            .collect::<Vec<_>>();
        assert_eq!(screens.len(), 2);
        assert!(
            screens
                .iter()
                .any(|screen| screen.contains("err-one") && !screen.contains("err-two"))
        );
        assert!(
            screens
                .iter()
                .any(|screen| screen.contains("err-two") && !screen.contains("err-one"))
        );
        assert!(
            screens
                .iter()
                .all(|screen| screen.contains("$ demo Capture"))
        );
        sessions.active = None;
        sessions.cycle(true);
        assert_eq!(sessions.active, Some(sessions.panes[0].id));
        sessions.cycle(false);
        assert!(sessions.active.is_none());
    }

    #[test]
    fn request_can_arrive_in_separate_header_and_payload_chunks() {
        let mut sessions = Sessions::new().unwrap();
        let request = request(&sessions, "exit 0", Mode::Capture);
        let mut stream = TcpStream::connect(sessions.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let client = std::thread::spawn(move || -> io::Result<Reply> {
            let payload = serde_json::to_vec(&request).unwrap();
            let length = (payload.len() as u32).to_be_bytes();
            // Let the server accept before sending anything, then split both
            // header and payload to exercise reads that must wait for data.
            for chunk in [
                &length[..2],
                &length[2..],
                &payload[..payload.len() / 2],
                &payload[payload.len() / 2..],
            ] {
                std::thread::sleep(Duration::from_millis(50));
                stream.write_all(chunk)?;
            }
            read_frame(&mut stream, MAX_FRAME)
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !client.is_finished() {
            sessions.poll(size(), true).unwrap();
            assert!(Instant::now() < deadline, "chunked request timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        match client
            .join()
            .unwrap()
            .expect("chunked request was rejected")
        {
            Reply::Output(result) => assert_eq!(result.code, 0),
            Reply::Error(err) => panic!("{err}"),
        }
        assert_eq!(sessions.panes.len(), 1);
    }

    #[test]
    fn unauthenticated_requests_cannot_start_a_command() {
        let mut sessions = Sessions::new().unwrap();
        let mut request = request(&sessions, "exit 0", Mode::Capture);
        request.token = "wrong-token".into();
        let mut stream = TcpStream::connect(sessions.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write_frame(&mut stream, &request).unwrap();
        let mut byte = [0];
        // Rejecting a peer can report EOF or a reset/broken pipe depending on
        // the OS; all mean the unauthenticated connection was closed.
        match stream.read(&mut byte) {
            Ok(0) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                ) => {}
            other => panic!("unauthenticated connection was not rejected: {other:?}"),
        }
        sessions.poll(size(), true).unwrap();
        assert!(sessions.panes.is_empty());
    }

    #[test]
    fn interactive_api_returns_all_pty_output_after_input_resize_and_exit() {
        let mut sessions = Sessions::new().unwrap();
        #[cfg(windows)]
        let script = "[Console]::WriteLine('READY'); $line=[Console]::ReadLine(); [Console]::WriteLine('ECHO:'+$line); 1..300 | ForEach-Object { [Console]::WriteLine('tail-line') }; [Console]::WriteLine('FINAL_TAIL'); exit 7";
        #[cfg(not(windows))]
        let script = "printf 'READY\\n'; IFS= read -r line; printf 'ECHO:%s\\n' \"$line\"; i=0; while [ $i -lt 300 ]; do printf 'tail-line\\n'; i=$((i+1)); done; printf 'FINAL_TAIL\\n'; exit 7";
        let receiver = submit(&sessions, request(&sessions, script, Mode::Interactive));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            sessions.poll(size(), true).unwrap();
            if sessions
                .active()
                .is_some_and(|pane| pane.parser.screen().contents().contains("READY"))
            {
                sessions
                    .resize(PtySize {
                        rows: 16,
                        cols: 100,
                        ..size()
                    })
                    .unwrap();
                sessions.active_mut().unwrap().write(b"hello\r").unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "PTY never became ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        let result = wait_result(&mut sessions, &receiver);
        assert_eq!(
            result.code,
            7,
            "PTY output: {:?}",
            String::from_utf8_lossy(&result.terminal_output)
        );
        assert!(result.stdout.is_empty() && result.stderr.is_empty());
        let output = String::from_utf8_lossy(&result.terminal_output);
        assert!(output.contains("ECHO:hello"), "{output}");
        assert!(
            output.contains("FINAL_TAIL"),
            "missing final output: {output}"
        );
        assert!(
            !output.contains("$ demo"),
            "UI labels must not contaminate API output"
        );
        assert_eq!(sessions.active().unwrap().code, Some(7));
    }
}
