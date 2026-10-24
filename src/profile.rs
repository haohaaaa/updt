use std::env;
use std::io::{self, IsTerminal};
use tokio::fs;

use crate::state::{AppState, SystemProfile};

pub async fn parse_profile(state: &mut AppState) {
    let prefix = env::var("PREFIX").unwrap_or_default();
    state.is_termux = prefix.contains("com.termux")
        || fs::metadata("/data/data/com.termux/files/usr/bin/pkg")
            .await
            .is_ok();
    state.is_arch_linux = fs::metadata("/etc/arch-release").await.is_ok();
    state.is_asahi_linux = if env::consts::OS == "linux" {
        fs::read_to_string("/etc/os-release")
            .await
            .ok()
            .and_then(|content| os_release_id(&content).map(|id| id == "fedora-asahi-remix"))
            .unwrap_or(false)
    } else {
        false
    };
    apply_profile_policy(
        state,
        detect_profile(
            env::consts::OS,
            state.is_termux,
            state.is_arch_linux,
            state.is_asahi_linux,
        ),
    );
}

fn os_release_id(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "ID" {
            return None;
        }
        let value = value.trim();
        Some(
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(value),
        )
    })
}

fn detect_profile(
    os: &str,
    is_termux: bool,
    is_arch_linux: bool,
    is_asahi_linux: bool,
) -> SystemProfile {
    if is_termux {
        return SystemProfile::Termux;
    }
    match os {
        "windows" => SystemProfile::Windows,
        "macos" => SystemProfile::Macos,
        "linux" if is_asahi_linux => SystemProfile::Asahi,
        "linux" if is_arch_linux => SystemProfile::Arch,
        _ => SystemProfile::Unknown,
    }
}

fn apply_profile_policy(state: &mut AppState, profile: SystemProfile) {
    state.system_profile = profile;
    match profile {
        SystemProfile::Termux => apply_termux_policy(state),
        SystemProfile::Windows => apply_windows_policy(state),
        SystemProfile::Macos => apply_macos_policy(state),
        SystemProfile::Arch => apply_arch_policy(state),
        SystemProfile::Asahi => apply_asahi_policy(state),
        SystemProfile::Unknown => {}
    }
}

fn apply_termux_policy(state: &mut AppState) {
    state.enable.pkg = true;
    state.enable.npm = true;
    state.enable.cargo = true;
    state.enable.rustup = false;
}

fn apply_windows_policy(state: &mut AppState) {
    state.enable.npm = true;
    state.enable.cargo = true;
    state.enable.rustup = true;
    state.enable.scoop = true;
}

fn apply_macos_policy(state: &mut AppState) {
    state.enable.brew = true;
    state.enable.npm = true;
    state.enable.cargo = true;
    state.enable.rustup = true;
}

fn apply_arch_policy(state: &mut AppState) {
    state.enable.npm = true;
    state.enable.cargo = true;
    state.enable.rustup = true;
    state.enable.paru = true;
    state.enable.pacman = true;
    state.enable.flatpak = true;
}

fn apply_asahi_policy(state: &mut AppState) {
    state.enable.dnf = true;
    state.enable.npm = true;
    state.enable.cargo = true;
    state.enable.rustup = true;
    state.enable.flatpak = true;
}

pub fn interactive_terminal() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

pub fn desktop_linux_session() -> bool {
    env::consts::OS == "linux"
        && [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "DESKTOP_SESSION",
        ]
        .iter()
        .any(|key| env::var_os(key).is_some_and(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::{apply_profile_policy, detect_profile, os_release_id};
    use crate::state::{AppState, SystemProfile, profile_name};

    #[test]
    fn recognizes_fedora_asahi_remix_release_id() {
        let release = "NAME=\"Fedora Linux Asahi Remix\"\nID=fedora-asahi-remix\nID_LIKE=fedora\n";

        assert_eq!(os_release_id(release), Some("fedora-asahi-remix"));
        assert_eq!(
            detect_profile("linux", false, false, true),
            SystemProfile::Asahi
        );
    }

    #[test]
    fn generic_fedora_and_missing_release_ids_remain_unknown() {
        assert_eq!(os_release_id("ID=fedora\n"), Some("fedora"));
        assert_eq!(os_release_id("NAME=Fedora Linux\n"), None);
        assert_eq!(
            detect_profile("linux", false, false, false),
            SystemProfile::Unknown
        );
    }

    #[test]
    fn asahi_policy_enables_its_supported_targets() {
        let mut state = AppState::default();

        apply_profile_policy(&mut state, SystemProfile::Asahi);

        assert_eq!(state.system_profile, SystemProfile::Asahi);
        assert_eq!(profile_name(state.system_profile), "asahi");
        assert!(state.enable.dnf);
        assert!(state.enable.npm);
        assert!(state.enable.cargo);
        assert!(state.enable.rustup);
        assert!(state.enable.flatpak);
        assert!(!state.enable.brew);
        assert!(!state.enable.paru);
        assert!(!state.enable.pacman);
        assert!(!state.enable.pkg);
    }
}
