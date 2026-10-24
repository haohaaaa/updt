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
- `dnf` upgrades may use `sudo` or `pkexec`, depending on terminal focus and desktop session state.
- Termux `pkg` checks use `apt list --upgradable`.
- `pacman` upgrades may use `sudo` or `pkexec`, depending on terminal focus and desktop session state.

## Interactive Flow

Running `updt` starts a three-stage workflow:

1. Check upgrade candidates.
2. Select targets to upgrade.
3. Run upgrades for selected targets.

The check stage runs in parallel across enabled targets. The upgrade stage runs the underlying package manager commands and forwards their output.

Selector controls:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move cursor |
| `Space` | Toggle selected target |
| `Enter` | Confirm selection |
| `q` / `Esc` | Quit selection |

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
