use crossterm::style::{Attribute, Color as TermColor, Stylize};
use std::io::{self, IsTerminal};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub enum MsgKind {
    Info,
    Ok,
    Warn,
}

fn color_enabled() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| io::stdout().is_terminal())
}

pub fn color(text: &str, c: TermColor) -> String {
    if color_enabled() {
        format!("{}", text.with(c))
    } else {
        text.to_string()
    }
}

pub fn color_bold(text: &str, c: TermColor) -> String {
    if color_enabled() {
        format!("{}", text.with(c).attribute(Attribute::Bold))
    } else {
        text.to_string()
    }
}

pub fn ok_text(text: &str) -> String {
    color(text, TermColor::Green)
}

pub fn warn_text(text: &str) -> String {
    color(text, TermColor::Yellow)
}

pub fn err_text(text: &str) -> String {
    color(text, TermColor::Red)
}

fn pkg_color(pkg: &str) -> TermColor {
    let _ = pkg;
    TermColor::Cyan
}

pub fn log_pkg_line(pkg: &str, msg: &str, kind: MsgKind) -> String {
    let prefix = color_bold(&format!("[{pkg}]"), pkg_color(pkg));
    let body = match kind {
        MsgKind::Info => color(msg, TermColor::White),
        MsgKind::Ok => ok_text(msg),
        MsgKind::Warn => warn_text(msg),
    };
    format!("{prefix} {body}")
}

pub fn print_section(title: &str) {
    println!();
    println!(
        "{}",
        color_bold(&format!("==== {title} ===="), TermColor::Cyan)
    );
}

pub fn print_command(command: &str) {
    println!("\n{}", color_bold(&format!("$ {command}"), TermColor::Cyan));
}

pub fn print_exit_signal_message() {
    println!(
        "{}",
        warn_text("检测到中断信号 (Ctrl+C/Ctrl+D), 程序已安全退出.")
    );
}
