use std::env;
use std::io;
use std::path::PathBuf;

use tokio::fs;

use crate::state::TARGET_IDS;

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

    let targets = TARGET_IDS.join(" ");
    let script = format!(
        "set -l __updt_targets {targets}\n\n\
complete -c updt -f\n\
complete -c updt -s h -l help -d 'Print help'\n\
complete -c updt -s V -l version -d 'Print version'\n\
complete -c updt -n '__fish_use_subcommand' -a 'update terminal fish'\n\
complete -c updt -n '__fish_seen_subcommand_from update' -x -a \"$__updt_targets\"\n\
complete -c updt -n '__fish_seen_subcommand_from update' -s h -l help -d 'Print help'\n\
complete -c updt -n '__fish_seen_subcommand_from terminal' -l demo -d 'Try terminal without updates'\n\
complete -c updt -n '__fish_seen_subcommand_from terminal' -l shell -d 'Open a shell without updates'\n\
complete -c updt -n '__fish_seen_subcommand_from terminal' -x -a \"$__updt_targets\"\n"
    );

    fs::write(&path, script).await?;
    Ok(path)
}
