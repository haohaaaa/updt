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

It checks supported targets in parallel, shows the results in a terminal selector, and upgrades only the targets you confirm. It does not replace any package manager or maintain its own package database; it orchestrates the tools already installed on your machine.

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

Install fish completions:

```bash
updt fish
```

## Features

- Parallel checks across enabled targets for faster detection.
- Interactive terminal UI for selecting which targets to upgrade.
- Direct non-interactive updates with explicit target names.
- OS profile policy that only enables targets that make sense for the current platform.
- Fish shell completion installer.
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
- `dnf` upgrades may use `sudo` or `pkexec`, depending on terminal focus and desktop session state.
- Termux `pkg` checks refresh repository metadata with `pkg update` and inspect candidates with `apt list --upgradable`.
- `pacman` upgrades may use `sudo` or `pkexec`, depending on terminal focus and desktop session state.

## Interactive Flow

Running `updt` starts a three-stage workflow:

1. Check upgrade candidates.
2. Select targets to upgrade.
3. Run upgrades for selected targets.

The check stage runs in parallel across enabled targets. The upgrade stage runs the underlying package manager commands and forwards their output.

On Windows, Scoop's `rustup` package update and cargo's `updt` self-update are deferred until all other updates finish. A single foreground window waits for the current `updt` process to exit, then updates Scoop's `rustup` package (preserving local/global scope), followed by `updt` itself. The main summary lists these as pending; their final results appear in the separate window. Rust toolchain updates (`rustup update --no-self-update`) still run during the normal upgrade stage.

For other Scoop updates blocked by running processes, `Kill & Retry` stops processes running from the package's installation directory before retrying. If that includes the current `updt` process, it skips the package without stopping any processes and prints a command to run after `updt` exits. If stopping processes fails, the package is reported as failed without retrying. Deferred Scoop updates report any remaining process blockers in the final update window without stopping them automatically.

Selector controls:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move cursor |
| `Space` | Toggle selected target |
| `Enter` | Confirm selection |
| `q` / `Esc` | Quit selection |

## CLI Reference

### Two interactive interfaces

The original interface and the embedded terminal interface have separate entry points:

| Command | Interface |
| --- | --- |
| `updt` | Original TUI: checks, target selection and confirmation, then command output |
| `updt update npm cargo` | Original interface for the requested targets |
| `updt terminal` | New interface: real target status on the left, checks, confirmation and updates inside the terminal on the right |
| `updt terminal npm cargo` | New interface for the requested targets; updates the selected targets that have updates |

Without explicit targets, the new interface asks which targets to update in the right-hand terminal. Answer package-manager prompts in their command session. Each command has its own output view, so parallel checks never interleave output from different commands. The workflow view shows parsed check results, confirmation, and the final summary. Update order and deferred Windows updates use the same business logic as the original interface.

The workflow submits commands through an internal Rust execution API. The UI opens a command session, displays the command, and renders its live output while the caller awaits the result. Capture commands return separate, unchanged stdout/stderr for JSON parsing. Interactive commands run in a native PTY and return the merged terminal output and exit code. Display and API results come from the same execution.

The sidebar lists command sessions; `F5` / `F6` switches between them and the workflow view. Up to 32 completed/running sessions are retained (older completed sessions are evicted first). Each session has its own terminal screen and scrollback. The header shows the selected command and its exit status. Workflow messages remain in the **activity view** (`F2`, up to 500 lines). See [the internal execution API](docs/terminal-execution.md) for implementation details.

On Windows, deferred `rustup` and `updt` updates wait for the **outer terminal interface** to exit, so its executable no longer holds the installation directory open. Once the workflow finishes, press Enter or Esc to close the interface and allow those updates to run.

The new interface uses a native PTY (ConPTY on Windows), with ANSI colors, Unicode, terminal resizing, keyboard input, paste, and up to 2,000 lines of scrollback. The standalone validation modes remain available:

```bash
updt terminal --demo
updt terminal --shell
```

`--demo` runs two harmless commands through the execution API: one produces JSON and stderr, and one asks for text input in a real PTY. The demo verifies the captured JSON and shows the interactive command's returned exit code and output size. It does not execute package-manager commands. `--shell` opens PowerShell on Windows or `$SHELL` (falling back to `/bin/sh`) on Unix without starting the update workflow. Commands you type run in that shell.

- Type directly in the terminal; `Ctrl+C` is forwarded to the child terminal.
- Use `F2` to switch between terminal output and the read-only activity view in the update workflow.
- Use `F5` / `F6` to switch to the previous/next command session or workflow view. Manually selecting a session stops automatic focus changes.
- Use `Shift+PageUp` / `Shift+PageDown` to review output or activity history.
- Use `F4` to return to the active interactive command (or workflow when none is running) and resume automatic focus. Query sessions and completed commands are read-only. Keyboard input and paste are paused while browsing history, so they cannot accidentally answer a hidden prompt.
- Use `exit` to end the shell, then `Enter` or `Esc` to return.
- `F10` asks for confirmation before ending a running terminal; it closes a finished terminal immediately.

Command sessions run separate processes; working-directory changes and shell variables do not persist between commands. Use `--shell` for a persistent interactive shell. Mouse forwarding and full compatibility with arbitrary terminal applications are not yet validated.

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

Install fish completion to `~/.config/fish/completions/updt.fish`:

```bash
updt fish
```

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
