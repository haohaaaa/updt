use std::env;
use std::io;
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

pub async fn command_exists(name: &str) -> bool {
    resolve_command_path(name).await.is_some()
}

pub async fn resolve_command_path(name: &str) -> Option<PathBuf> {
    if command_name_is_path(name) {
        return executable_path(name).await;
    }

    let candidates = command_name_candidates(name);
    find_command_in_path(&candidates).await
}

fn command_name_is_path(name: &str) -> bool {
    name.contains('/') || name.contains('\\')
}

async fn executable_path(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    is_executable(path).await.then(|| path.to_path_buf())
}

async fn find_command_in_path(candidates: &[String]) -> Option<PathBuf> {
    for dir in env::split_paths(&env::var_os("PATH")?) {
        for candidate_name in candidates {
            let candidate = dir.join(candidate_name);
            if is_executable(&candidate).await {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn command_name_candidates(name: &str) -> Vec<String> {
    let path = Path::new(name);
    if path.extension().is_some() {
        return vec![name.to_string()];
    }

    let pathext = env::var_os("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut candidates = Vec::new();
    for ext in pathext
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
    {
        candidates.push(format!("{name}{ext}"));
    }
    candidates.push(name.to_string());
    candidates
}

#[cfg(not(windows))]
fn command_name_candidates(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

async fn command_program(program: &str) -> PathBuf {
    resolve_command_path(program)
        .await
        .unwrap_or_else(|| PathBuf::from(program))
}

async fn command(program: &str) -> Command {
    let program_path = command_program(program).await;
    #[cfg(windows)]
    {
        if program_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        {
            let mut cmd = Command::new("cmd.exe");
            cmd.arg("/D").arg("/C").arg("call").arg(program_path);
            return cmd;
        }
    }
    Command::new(program_path)
}

async fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path).await else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub async fn run_output(program: &str, args: &[&str]) -> io::Result<Output> {
    command(program).await.args(args).output().await
}

pub async fn run_capture(program: &str, args: &[&str]) -> io::Result<(i32, String)> {
    let output = run_output(program, args).await?;
    let code = output.status.code().unwrap_or(-1);
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((code, text))
}

pub async fn run_capture_streaming(program: &str, args: &[&str]) -> io::Result<(i32, String)> {
    let mut child = command(program)
        .await
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("stderr pipe unavailable"))?;

    let stdout_task = tokio::spawn(read_streaming_output(stdout));
    let stderr_task = tokio::spawn(read_streaming_output(stderr));
    let status = child.wait().await?;
    let stdout_text = join_stream_reader(stdout_task).await?;
    let stderr_text = join_stream_reader(stderr_task).await?;

    let mut text = String::new();
    text.push_str(&stdout_text);
    text.push_str(&stderr_text);
    Ok((status.code().unwrap_or(-1), text))
}

async fn read_streaming_output<R>(mut reader: R) -> io::Result<String>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }

        write_stream_chunk(&buffer[..read])?;
        bytes.extend_from_slice(&buffer[..read]);
    }

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_stream_chunk(bytes: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()
}

async fn join_stream_reader(
    handle: tokio::task::JoinHandle<io::Result<String>>,
) -> io::Result<String> {
    handle
        .await
        .map_err(|err| io::Error::other(format!("stream reader task failed: {err}")))?
}

pub async fn run_inherit(program: &str, args: &[&str]) -> io::Result<bool> {
    let status = command(program)
        .await
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    Ok(status.success())
}

pub async fn run_cargo_install_update_capture(args: &[&str]) -> io::Result<(i32, String)> {
    let mut proxy_args = vec!["install-update", "--locked"];
    proxy_args.extend_from_slice(args);
    run_capture("cargo-install-update", &proxy_args).await
}

pub async fn run_cargo_install_update_inherit(args: &[&str]) -> io::Result<bool> {
    let mut proxy_args = vec!["install-update", "--locked"];
    proxy_args.extend_from_slice(args);
    run_inherit("cargo-install-update", &proxy_args).await
}
