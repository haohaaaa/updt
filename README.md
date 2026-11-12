# updt

[![Crates.io](https://img.shields.io/crates/v/updt.svg)](https://crates.io/crates/updt)
[![Docs.rs](https://docs.rs/updt/badge.svg)](https://docs.rs/updt)
[![CI](https://github.com/haohaaaa/updt/actions/workflows/ci.yml/badge.svg)](https://github.com/haohaaaa/updt/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`updt` is a Rust command-line tool that checks and runs updates across your development package managers from one interactive workflow.

Instead of remembering separate update commands for Homebrew, npm, cargo, rustup, Scoop, paru, pacman, DNF, Flatpak, and Termux `pkg`, run `updt`, review what has updates, choose the targets you want, and let the original package managers do the work.

```bash
cargo install updt
updt
```

## Project Links

- Crates.io package: https://crates.io/crates/updt
- API documentation: https://docs.rs/updt
- GitHub repository: https://github.com/haohaaaa/updt

## What It Does

`updt` answers one question quickly: which of my system and developer tools can be upgraded right now?

It checks supported targets in parallel, shows the results with an inline command-line selector, and upgrades only the targets you confirm. It does not replace any package manager or maintain its own package database; it orchestrates the tools already installed on your machine.

## Why Use It

- One command for routine update checks across multiple ecosystems.
- Interactive selection before anything is upgraded.
- Direct target updates for scripts or muscle memory, such as `updt update npm cargo`.
- Platform-aware defaults for macOS, Windows, Arch Linux, Fedora Asahi Remix, and Termux.
- Small Rust binary with no shell script bootstrap step.

## Quick Start

Install the latest release from crates.io:

```bash
cargo install updt
```

Run the interactive update flow:

```bash
updt
```

Update specific targets without opening the selector:

```bash
updt update npm cargo
updt update npm,cargo
```

Native Cargo builds on Linux and macOS automatically install fish completions to `$XDG_CONFIG_HOME/fish/completions/updt.fish`, or `~/.config/fish/completions/updt.fish` by default. This includes `cargo install`, `cargo build`, and `cargo test` when the build script runs. Windows and cross-compilation skip automatic installation. A completion write failure produces a warning and does not stop the build.

To disable automatic installation:

```bash
UPDT_INSTALL_FISH_COMPLETION=0 cargo install updt
```

Cargo may reuse a cached build script. Install or refresh fish completions manually when needed:

```bash
updt fish
```

## Features

- Parallel checks across enabled targets with an inline spinner and completed-target count.
- Inline command-line prompts for selecting which targets to upgrade.
- Direct non-interactive updates with explicit target names.
- OS profile policy that only enables targets that make sense for the current platform.
- Shell completion generation for Bash, Elvish, fish, PowerShell, and Zsh, with automatic fish installation during native Linux and macOS builds.
- Uses existing package manager commands and forwards their output.

## Supported Update Targets

| Target | What it updates | Notes |
| --- | --- | --- |
| `brew` | Homebrew formulae and casks | macOS profile |
| `npm` | npm global packages | Requires `npm` |
| `cargo` | cargo-installed crates | Requires `cargo-install-update` |
| `rustup` | Rust toolchains | Requires `rustup` |
| `scoop` | Scoop packages | Windows profile |
| `paru` | AUR packages | Arch Linux profile |
| `dnf` | Fedora system packages | Fedora Asahi Remix profile |
| `flatpak` | Flatpak apps | Arch Linux profile |
| `pacman` | pacman packages | Arch Linux profile |
| `pkg` | Termux packages | Termux profile |

Windows system package managers `winget` and `choco` are intentionally unsupported.

## Platform Defaults

`updt` enables targets by OS profile:

| Profile | Enabled targets |
| --- | --- |
| macOS | `brew`, `npm`, `cargo`, `rustup` |
| Windows | `npm`, `cargo`, `rustup`, `scoop` |
| Arch Linux | `npm`, `cargo`, `rustup`, `paru`, `pacman`, `flatpak` |
| Fedora Asahi Remix | `dnf`, `npm`, `cargo`, `rustup`, `flatpak` |
| Termux | `pkg`, `npm`, `cargo` |
| Other systems | None |

Unsupported or missing commands are skipped during checks.

## Requirements

Install the package managers you want `updt` to control. For cargo package checks, install `cargo-update` so `cargo-install-update` is available:

```bash
cargo install cargo-update
```

Target-specific notes:

- `cargo` checks require `cargo-install-update` in `PATH`.
- `cargo-binstall` is recommended for faster binary installation of cargo-managed tools, except on Android (Termux).
- `dnf` checks require support for `dnf check-update --json` (DNF5).
- `dnf` upgrades refresh repository metadata with `--refresh` so the elevated command sees current updates.
- `dnf` upgrades use `sudo`, with authentication handled in the terminal.
- Termux `pkg` checks refresh repository metadata with `pkg update` and inspect candidates with `apt list --upgradable`.
- `pacman` upgrades use `sudo`, with authentication handled in the terminal.

## Interactive Flow

Running `updt` starts a three-stage workflow:

1. Check upgrade candidates.
2. Select targets to upgrade.
3. Run upgrades for selected targets.

The check stage runs in parallel across enabled targets and prints results above an inline spinner with a completed-target count. Redirected output uses plain log lines. Selection uses an inline checkbox prompt, like a typical `npx` setup script. It does not open a full-screen TUI or switch to an alternate screen, so previous output stays in your terminal history. Available targets are checked by default; press Enter to confirm and start upgrading. The upgrade stage runs the underlying package manager commands and forwards their output. With redirected input or output, selection uses sequential `[Y/n]` prompts instead.

Immediately before selection, a pending-update checklist groups all detected updates by target and lists every known package or toolchain. The selector includes item counts and a short name preview; the full checklist stays in terminal history. Explicit `updt update ...` commands show the same checklist before upgrading. If a check only reports that updates exist, the checklist says details are unavailable instead of showing a misleading zero count. Check failures remain visible in the logs, with a reminder that the detected update list may be incomplete.

On Windows, Scoop's `rustup` package update and cargo's `updt` self-update are deferred until all other updates finish. A single foreground window waits for the current `updt` process to exit, then updates Scoop's `rustup` package (preserving local/global scope), followed by `updt` itself. The main summary lists these as pending; their final results appear in the separate window. Rust toolchain updates (`rustup update --no-self-update`) still run during the normal upgrade stage.

For other Scoop updates blocked by running processes, `Kill & Retry` stops processes running from the package's installation directory before retrying. If that includes the current `updt` process, it skips the package without stopping any processes and prints a command to run after `updt` exits. If stopping processes fails, the package is reported as failed without retrying. Deferred Scoop updates report any remaining process blockers in the final update window without stopping them automatically.

Selector controls:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move cursor |
| `Space` | Toggle selected target |
| `Left` / `Right` | Deselect or select all targets |
| `Enter` | Confirm selection |
| `Esc` / `Ctrl+C` | Quit selection |

## CLI Reference

Show the installed version:

```bash
updt --version
```

Run the default interactive workflow:

```bash
updt
```

Update selected targets without the interactive selector:

```bash
updt update npm
updt update npm,cargo
updt update npm cargo
```

Install or refresh fish completion in `$XDG_CONFIG_HOME/fish/completions` (default: `~/.config/fish/completions`):

```bash
updt fish
```

Generate a shell completion script to stdout:

```bash
updt completions bash
updt completions elvish
updt completions fish
updt completions powershell
updt completions zsh
```

Save or source the generated script according to your shell's completion setup. `updt fish` installs the same generated fish script automatically. Completion scripts are generated from the public CLI definition. Bash, fish, and Zsh suggest target names and shell values as well as commands and options. The static Elvish and PowerShell generators currently suggest command and option names only.

## Development

Build the project:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Run formatting and lint checks before opening a pull request:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

## Contributing

Issues and pull requests are welcome. Please include the operating system, the target package manager, and the command output when reporting an update detection or upgrade problem.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and [SECURITY.md](SECURITY.md) for responsible vulnerability reporting.

## License

This project is licensed under the [MIT License](LICENSE).
