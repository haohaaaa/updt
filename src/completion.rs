use clap_complete::{Shell, generate};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

use tokio::fs;

use crate::cli::build_cli;

pub fn completion_script(shell: Shell) -> Vec<u8> {
    let mut command = build_cli();
    let mut script = Vec::new();
    generate(shell, &mut command, "updt", &mut script);
    if shell == Shell::Fish {
        // The static fish generator omits positional values. Keep candidates
        // sourced from clap, including targets and completion shell names.
        for subcommand in command.get_subcommands() {
            let values = subcommand
                .get_positionals()
                .filter_map(|arg| arg.get_value_parser().possible_values())
                .flatten()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect::<Vec<_>>();
            if !values.is_empty() {
                let line = format!(
                    "complete -c updt -n '__fish_updt_using_subcommand {}' -f -a '{}'\n",
                    subcommand.get_name(),
                    values.join(" "),
                );
                script.extend_from_slice(line.as_bytes());
            }
        }
    }
    script
}

pub fn print_completion(shell: Shell) -> io::Result<()> {
    io::stdout().lock().write_all(&completion_script(shell))
}

pub async fn install_fish_completion() -> io::Result<PathBuf> {
    let Some(home) = env::var_os("HOME") else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME env is not set",
        ));
    };

    let path = PathBuf::from(home)
        .join(".config")
        .join("fish")
        .join("completions")
        .join("updt.fish");

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid completion path"))?;
    fs::create_dir_all(parent).await?;

    fs::write(&path, completion_script(Shell::Fish)).await?;
    Ok(path)
}
