use clap_complete::Shell;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

#[path = "src/cli/definition.rs"]
mod cli;
#[path = "src/completion/generate.rs"]
mod generate;
#[path = "src/completion/install.rs"]
mod install;
#[path = "src/targets.rs"]
mod targets;

fn main() -> io::Result<()> {
    for path in [
        "build.rs",
        "src/cli/definition.rs",
        "src/completion/generate.rs",
        "src/completion/install.rs",
        "src/targets.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    for name in ["XDG_CONFIG_HOME", "HOME", "UPDT_INSTALL_FISH_COMPLETION"] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let native = env::var_os("HOST").is_some_and(|host| Some(host) == env::var_os("TARGET"));
    if !matches!(target_os.as_str(), "linux" | "macos")
        || !native
        || env::var_os("UPDT_INSTALL_FISH_COMPLETION").is_some_and(|value| value == "0")
    {
        return Ok(());
    }

    let script = generate::completion_script(Shell::Fish);
    let out_dir = env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("OUT_DIR is not set"))?;
    fs::write(PathBuf::from(out_dir).join("updt.fish"), &script)?;
    if let Err(err) = install::install_fish_completion(&script) {
        println!(
            "cargo:warning=Could not install fish completion: {err}. Run `updt fish` to retry."
        );
    }
    Ok(())
}
