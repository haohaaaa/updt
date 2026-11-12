use assert_cmd::{Command, cargo::cargo_bin_cmd};
use std::time::Duration;

fn cli() -> Command {
    let mut command = cargo_bin_cmd!("updt");
    // A parsing regression must never launch installed package managers.
    command.env("PATH", "").timeout(Duration::from_secs(10));
    command
}

#[test]
fn version_matches_the_package() {
    cli()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("updt {}\n", env!("CARGO_PKG_VERSION")))
        .stderr("");
}

#[test]
fn help_lists_cli_commands_without_terminal_mode() {
    let result = cli().arg("--help").assert().success().stderr("");
    let stdout = String::from_utf8_lossy(&result.get_output().stdout);
    for command in ["update", "fish", "completions"] {
        assert!(stdout.contains(command), "{stdout}");
    }
    assert!(!stdout.contains("terminal"), "{stdout}");
}

#[test]
fn completions_cover_public_commands_and_flags_in_each_shell() {
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let result = cli()
            .args(["completions", shell])
            .assert()
            .success()
            .stderr("");
        let script = String::from_utf8_lossy(&result.get_output().stdout);
        for item in ["updt", "update", "fish", "completions"] {
            assert!(
                script.contains(item),
                "{shell} completion is missing {item}"
            );
        }
        // Static Elvish and PowerShell generators only support command/flag names.
        if matches!(shell, "bash" | "fish" | "zsh") {
            for value in ["npm", "cargo", "dnf", "powershell"] {
                assert!(
                    script.contains(value),
                    "{shell} completion is missing {value}"
                );
            }
        }
        for removed in [
            "terminal",
            "__terminal-worker",
            "__terminal-demo",
            "--demo",
            "--shell",
        ] {
            assert!(
                !script.contains(removed),
                "{shell} completion exposes {removed}"
            );
        }
        assert!(!script.contains("检查可升级项"), "{shell}: {script}");
        assert!(!script.contains('\x1b'), "{shell}: {script}");
    }
}

#[test]
fn completions_require_a_supported_shell() {
    for args in [vec!["completions"], vec!["completions", "unsupported"]] {
        cli().args(args).assert().code(2).stdout("");
    }
}

#[test]
fn invalid_update_targets_fail_before_any_checks() {
    cli()
        .args(["update", "unsupported"])
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn removed_terminal_commands_fail_before_any_checks() {
    for args in [
        vec!["terminal"],
        vec!["terminal", "npm", "cargo"],
        vec!["terminal", "--demo"],
        vec!["terminal", "--shell"],
        vec!["__terminal-demo"],
        vec!["__terminal-worker", "--owner-pid", "1"],
    ] {
        cli().args(args).assert().code(2).stdout("");
    }
}
