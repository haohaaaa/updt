mod target;

pub use target::{
    TARGET_IDS, section_title, target_enabled, target_label, target_state_flags,
    target_update_summary, updatable_items_for_target,
};

#[derive(Clone, Copy)]
pub struct TargetStateFlags {
    pub enabled: bool,
    pub installed: bool,
    pub check_failed: bool,
    pub has_updates: bool,
    pub needs_cargo_updater: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SystemProfile {
    #[default]
    Unknown,
    Windows,
    Macos,
    Arch,
    Asahi,
    Termux,
}

#[derive(Default, Clone)]
pub struct TargetBucketState {
    pub installed: bool,
    pub has_updates: bool,
    pub check_failed: bool,
    pub updatable_items: Vec<String>,
}

#[derive(Default, Clone)]
pub struct BrewState {
    pub installed: bool,
    pub has_updates: bool,
    pub check_failed: bool,
    pub formula_list: Vec<String>,
    pub cask_list: Vec<String>,
}

#[derive(Default, Clone)]
pub struct CargoState {
    pub installed: bool,
    pub has_updates: bool,
    pub check_failed: bool,
    pub updater_installed: bool,
    pub updatable_packages: Vec<String>,
}

#[derive(Default, Clone)]
pub struct EnablePolicy {
    pub brew: bool,
    pub npm: bool,
    pub cargo: bool,
    pub rustup: bool,
    pub scoop: bool,
    pub paru: bool,
    pub pacman: bool,
    pub dnf: bool,
    pub flatpak: bool,
    pub pkg: bool,
}

#[derive(Default, Clone)]
pub struct AppState {
    pub brew: BrewState,
    pub npm: TargetBucketState,
    pub cargo: CargoState,
    pub rustup: TargetBucketState,
    pub scoop: TargetBucketState,
    pub paru: TargetBucketState,
    pub flatpak: TargetBucketState,
    pub pacman: TargetBucketState,
    pub dnf: TargetBucketState,
    pub pkg: TargetBucketState,
    pub is_arch_linux: bool,
    pub is_asahi_linux: bool,
    pub is_termux: bool,
    pub system_profile: SystemProfile,
    pub enable: EnablePolicy,
}

pub fn profile_name(profile: SystemProfile) -> &'static str {
    const NAMES: &[(SystemProfile, &str)] = &[
        (SystemProfile::Unknown, "unknown"),
        (SystemProfile::Windows, "windows"),
        (SystemProfile::Macos, "macos"),
        (SystemProfile::Arch, "arch"),
        (SystemProfile::Asahi, "asahi"),
        (SystemProfile::Termux, "termux"),
    ];
    NAMES
        .iter()
        .find(|(known, _)| *known == profile)
        .map(|(_, name)| *name)
        .unwrap_or("unknown")
}
