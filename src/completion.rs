use clap_complete::Shell;
use std::io::{self, Write};
use std::path::PathBuf;

mod generate;
mod install;

pub use generate::completion_script;

pub fn print_completion(shell: Shell) -> io::Result<()> {
    io::stdout().lock().write_all(&completion_script(shell))
}

pub fn install_fish_completion() -> io::Result<PathBuf> {
    install::install_fish_completion(&completion_script(Shell::Fish))
}
