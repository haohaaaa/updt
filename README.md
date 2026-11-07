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
- Inline command-line prompts for selecting which targets to upgrade, with an optional embedded terminal interface.
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

### Two interactive interfaces

The inline command-line interface and the embedded terminal interface have separate entry points:

| Command | Interface |
| --- | --- |
| `updt` | Inline CLI: printed check results, checkbox selection, then command output; no full-screen TUI |
| `updt update npm cargo` | Inline CLI for the requested targets, without the selector |
| `updt terminal` | New interface: real target status on the left, checks, confirmation and updates inside the terminal on the right |
| `updt terminal npm cargo` | New interface for the requested targets; updates the selected targets that have updates |

Without explicit targets, the embedded terminal interface asks which targets to update in the right-hand terminal. Answer package-manager prompts in their command session. Each command has its own output view, so parallel checks never interleave output from different commands. The workflow view shows parsed check results, confirmation, and the final summary. Update order and deferred Windows updates use the same business logic as the inline CLI.

The workflow submits commands through an internal Rust execution API. The UI opens a command session, displays the command, and renders its live output while the caller awaits the result. Capture commands return separate, unchanged stdout/stderr for JSON parsing. Interactive commands run in a native PTY and return the merged terminal output and exit code. Display and API results come from the same execution.

The sidebar lists command sessions; `F5` / `F6` switches between them and the workflow view. Up to 32 completed/running sessions are retained (older completed sessions are evicted first). Each session has its own terminal screen and scrollback. The header shows the selected command and its exit status. Workflow messages remain in the **activity view** (`F2`, up to 500 lines). See [the internal execution API](docs/terminal-execution.md) for implementation details.

On Windows, deferred `rustup` and `updt` updates wait for the **outer terminal interface** to exit, so its executable no longer holds the installation directory open. Once the workflow finishes, press Esc to leave panel focus, then `q` to close the interface and allow those updates to run.

The new interface uses a native PTY (ConPTY on Windows), with ANSI colors, Unicode, terminal resizing, keyboard and mouse input, paste, and up to 2,000 lines of scrollback. The standalone validation modes remain available:

```bash
updt terminal --demo
updt terminal --shell
```

`--demo` runs two harmless commands through the execution API: one produces JSON and stderr, and one asks for text input in a real PTY. The demo verifies the captured JSON and shows the interactive command's returned exit code and output size. It does not execute package-manager commands. `--shell` opens PowerShell on Windows or `$SHELL` (falling back to `/bin/sh`) on Unix without starting the update workflow. Commands you type run in that shell.

- Start in panel navigation mode. Use `h/j/k/l` or the arrow keys to move between the target list, session list, and terminal output; press Enter to focus the selected panel. A cyan border marks the selected panel; green means focused.
- Within a focused list, use `h/j/k/l` or the arrow keys to move between entries. Moving through sessions displays each session's output. Within the focused terminal, keys (including `q`, `h/j/k/l`, arrows, and `Ctrl+C`) go to the child process.
- Esc always leaves panel focus and never exits the interface or reaches the child process. Press `q` in navigation mode to exit; a running workflow requires confirmation. Enter does not close a finished terminal.
- Click a target or session to select it and focus its panel; click terminal output to focus it. The footer buttons are clickable too.
- Scroll over terminal output or activity history to review it. Scroll over the sidebar lists to browse their entries. When a focused live child requests mouse reporting, clicks, releases, dragging, motion, and wheel events are forwarded using its requested mode and pane-relative coordinates. Hold Shift while scrolling to review local history instead.
- Use `F2` to switch between terminal output and the read-only activity view in the update workflow.
- Use `F5` / `F6` to switch to the previous/next command session or workflow view. Manually selecting a session stops automatic focus changes.
- Use `Shift+PageUp` / `Shift+PageDown` to review output or activity history.
- Use `F4` to return to the active interactive command (or workflow when none is running) and resume automatic focus. Query sessions and completed commands are read-only. Keyboard input and paste are paused while browsing history, so they cannot accidentally answer a hidden prompt.
- Use `exit` to end the shell, then Esc to leave focus and `q` to return.

Command sessions run separate processes; working-directory changes and shell variables do not persist between commands. Use `--shell` for a persistent interactive shell. Mouse capture is restored on exit. Full compatibility with arbitrary terminal applications is not guaranteed.

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
