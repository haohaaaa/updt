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
    if crate::terminal::business::is_worker() {
        return run_output_streaming(program, args, false).await;
    }
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
    let output =
        run_output_streaming(program, args, crate::terminal::business::is_worker()).await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.code().unwrap_or(-1), text))
}

async fn run_output_streaming(
    program: &str,
    args: &[&str],
    inherit_input: bool,
) -> io::Result<Output> {
    print_worker_command(program, args);
    let mut child = command(program)
        .await
        .args(args)
        .stdin(if inherit_input {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
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
    let stdout = join_stream_reader(stdout_task).await?;
    let stderr = join_stream_reader(stderr_task).await?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn read_streaming_output<R>(mut reader: R) -> io::Result<Vec<u8>>
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

    Ok(bytes)
}

fn write_stream_chunk(bytes: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()
}

async fn join_stream_reader(
    handle: tokio::task::JoinHandle<io::Result<Vec<u8>>>,
) -> io::Result<Vec<u8>> {
    handle
        .await
        .map_err(|err| io::Error::other(format!("stream reader task failed: {err}")))?
}

pub async fn run_inherit(program: &str, args: &[&str]) -> io::Result<bool> {
    print_worker_command(program, args);
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

fn print_worker_command(program: &str, args: &[&str]) {
    if !crate::terminal::business::is_worker() {
        return;
    }
    let script = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("-Command"));
    let args = args[..script.unwrap_or(args.len())].join(" ");
    let label = format!("{program} {args}");
    let short = label.chars().take(180).collect::<String>();
    println!(
        "\n$ {short}{}",
        if script.is_some() { " [script]" } else { "" }
    );
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

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn streaming_preserves_separate_output_and_nonzero_exit_status() {
        #[cfg(windows)]
        let output = super::run_output_streaming(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "[Console]::Out.Write('{\"ok\":true}'); [Console]::Error.Write('progress'); exit 3",
            ],
            false,
        )
        .await
        .unwrap();
        #[cfg(not(windows))]
        let output = super::run_output_streaming(
            "/bin/sh",
            &["-c", "printf '{\"ok\":true}'; printf progress >&2; exit 3"],
            false,
        )
        .await
        .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"{\"ok\":true}");
        assert_eq!(output.stderr, b"progress");
    }
}
