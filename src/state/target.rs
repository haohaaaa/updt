use super::{AppState, TargetStateFlags};

pub const TARGET_IDS: [&str; 10] = [
    "brew", "npm", "cargo", "rustup", "scoop", "paru", "flatpak", "pacman", "dnf", "pkg",
];

struct TargetMeta {
    id: &'static str,
    label: &'static str,
    section: &'static str,
    update_summary: &'static str,
    flags: fn(&AppState) -> TargetStateFlags,
    items: fn(&AppState) -> Vec<String>,
}

const TARGET_META: [TargetMeta; 10] = [
    TargetMeta {
        id: "brew",
        label: "Homebrew",
        section: "Homebrew",
        update_summary: "发现可升级项",
        flags: brew_flags,
        items: brew_items,
    },
    TargetMeta {
        id: "npm",
        label: "npm",
        section: "npm (global)",
        update_summary: "发现可升级项",
        flags: npm_flags,
        items: npm_items,
    },
    TargetMeta {
        id: "cargo",
        label: "cargo",
        section: "cargo",
        update_summary: "发现可升级项",
        flags: cargo_flags,
        items: cargo_items,
    },
    TargetMeta {
        id: "rustup",
        label: "rustup",
        section: "rustup",
        update_summary: "发现可升级项",
        flags: rustup_flags,
        items: rustup_items,
    },
    TargetMeta {
        id: "scoop",
        label: "scoop",
        section: "scoop",
        update_summary: "发现可升级项",
        flags: scoop_flags,
        items: scoop_items,
    },
    TargetMeta {
        id: "paru",
        label: "paru",
        section: "paru (AUR)",
        update_summary: "发现可升级项",
        flags: paru_flags,
        items: paru_items,
    },
    TargetMeta {
        id: "flatpak",
        label: "flatpak",
        section: "flatpak",
        update_summary: "发现可升级项",
        flags: flatpak_flags,
        items: flatpak_items,
    },
    TargetMeta {
        id: "pacman",
        label: "pacman",
        section: "pacman",
        update_summary: "发现可升级项",
        flags: pacman_flags,
        items: pacman_items,
    },
    TargetMeta {
        id: "dnf",
        label: "DNF",
        section: "DNF",
        update_summary: "发现可升级项",
        flags: dnf_flags,
        items: dnf_items,
    },
    TargetMeta {
        id: "pkg",
        label: "pkg",
        section: "pkg (Termux)",
        update_summary: "发现可升级项",
        flags: pkg_flags,
        items: pkg_items,
    },
];

fn target_meta(id: &str) -> Option<&'static TargetMeta> {
    TARGET_META.iter().find(|meta| meta.id == id)
}

pub fn target_label(id: &str) -> &'static str {
    target_meta(id).map(|m| m.label).unwrap_or("unknown")
}

pub fn section_title(target: &str) -> &'static str {
    target_meta(target).map(|m| m.section).unwrap_or("unknown")
}

pub fn target_update_summary(target: &str) -> &'static str {
    target_meta(target)
        .map(|m| m.update_summary)
        .unwrap_or("发现可升级项")
}

pub fn target_state_flags(state: &AppState, target: &str) -> Option<TargetStateFlags> {
    target_meta(target).map(|meta| (meta.flags)(state))
}

pub fn target_enabled(state: &AppState, target: &str) -> bool {
    target_state_flags(state, target)
        .map(|flags| flags.enabled)
        .unwrap_or(false)
}

pub fn updatable_items_for_target(state: &AppState, target: &str) -> Vec<String> {
    target_meta(target)
        .map(|meta| (meta.items)(state))
        .unwrap_or_default()
}

fn bucket_flags(
    enabled: bool,
    installed: bool,
    check_failed: bool,
    has_updates: bool,
) -> TargetStateFlags {
    TargetStateFlags {
        enabled,
        installed,
        check_failed,
        has_updates,
        needs_cargo_updater: false,
    }
}

fn brew_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.brew,
        state.brew.installed,
        state.brew.check_failed,
        state.brew.has_updates,
    )
}

fn npm_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.npm,
        state.npm.installed,
        state.npm.check_failed,
        state.npm.has_updates,
    )
}

fn cargo_flags(state: &AppState) -> TargetStateFlags {
    TargetStateFlags {
        enabled: state.enable.cargo,
        installed: state.cargo.installed,
        check_failed: state.cargo.check_failed,
        has_updates: state.cargo.has_updates,
        needs_cargo_updater: !state.cargo.updater_installed,
    }
}

fn rustup_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.rustup,
        state.rustup.installed,
        state.rustup.check_failed,
        state.rustup.has_updates,
    )
}

fn scoop_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.scoop,
        state.scoop.installed,
        state.scoop.check_failed,
        state.scoop.has_updates,
    )
}

fn paru_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.paru,
        state.paru.installed,
        state.paru.check_failed,
        state.paru.has_updates,
    )
}

fn flatpak_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.flatpak,
        state.flatpak.installed,
        state.flatpak.check_failed,
        state.flatpak.has_updates,
    )
}

fn pacman_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.pacman,
        state.pacman.installed,
        state.pacman.check_failed,
        state.pacman.has_updates,
    )
}

fn dnf_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.dnf,
        state.dnf.installed,
        state.dnf.check_failed,
        state.dnf.has_updates,
    )
}

fn pkg_flags(state: &AppState) -> TargetStateFlags {
    bucket_flags(
        state.enable.pkg,
        state.pkg.installed,
        state.pkg.check_failed,
        state.pkg.has_updates,
    )
}

fn brew_items(state: &AppState) -> Vec<String> {
    state
        .brew
        .formula_list
        .iter()
        .chain(state.brew.cask_list.iter())
        .cloned()
        .collect()
}

fn npm_items(state: &AppState) -> Vec<String> {
    state.npm.updatable_items.clone()
}

fn cargo_items(state: &AppState) -> Vec<String> {
    state.cargo.updatable_packages.clone()
}

fn rustup_items(state: &AppState) -> Vec<String> {
    state.rustup.updatable_items.clone()
}

fn scoop_items(state: &AppState) -> Vec<String> {
    state.scoop.updatable_items.clone()
}

fn paru_items(state: &AppState) -> Vec<String> {
    state.paru.updatable_items.clone()
}

fn flatpak_items(state: &AppState) -> Vec<String> {
    state.flatpak.updatable_items.clone()
}

fn pacman_items(state: &AppState) -> Vec<String> {
    state.pacman.updatable_items.clone()
}

fn dnf_items(state: &AppState) -> Vec<String> {
    state.dnf.updatable_items.clone()
}

fn pkg_items(state: &AppState) -> Vec<String> {
    state.pkg.updatable_items.clone()
}

#[cfg(test)]
mod tests {
    use super::{
        TARGET_IDS, section_title, target_enabled, target_label, target_state_flags,
        target_update_summary, updatable_items_for_target,
    };
    use crate::state::AppState;

    #[test]
    fn returns_target_metadata_and_unknown_defaults() {
        assert_eq!(target_label("brew"), "Homebrew");
        assert_eq!(target_label("dnf"), "DNF");
        assert_eq!(section_title("pkg"), "pkg (Termux)");
        assert_eq!(target_label("missing"), "unknown");
        assert_eq!(target_update_summary("missing"), "发现可升级项");
    }

    #[test]
    fn reports_enabled_state_from_flags() {
        let mut state = AppState::default();
        state.enable.rustup = true;
        state.rustup.installed = true;

        assert!(target_enabled(&state, "rustup"));
        assert!(!target_enabled(&state, "brew"));
        assert!(!target_enabled(&state, "missing"));
    }

    #[test]
    fn dnf_target_metadata_uses_dnf_state() {
        let mut state = AppState::default();
        state.enable.dnf = true;
        state.dnf.installed = true;
        state.dnf.has_updates = true;
        state.dnf.updatable_items = vec!["kernel.aarch64".to_string()];

        let flags = target_state_flags(&state, "dnf").expect("dnf target flags");

        assert!(flags.enabled);
        assert!(flags.installed);
        assert!(flags.has_updates);
        assert_eq!(
            updatable_items_for_target(&state, "dnf"),
            ["kernel.aarch64"]
        );
        assert!(TARGET_IDS.contains(&"dnf"));
    }

    #[test]
    fn cargo_flags_report_missing_cargo_update() {
        let mut state = AppState::default();
        state.enable.cargo = true;
        state.cargo.installed = true;

        let flags = target_state_flags(&state, "cargo").expect("cargo target flags");

        assert!(flags.enabled);
        assert!(flags.installed);
        assert!(flags.needs_cargo_updater);
    }

    #[test]
    fn brew_updatable_items_merge_formulae_and_casks() {
        let mut state = AppState::default();
        state.brew.formula_list = vec!["git".to_string()];
        state.brew.cask_list = vec!["visual-studio-code".to_string()];

        assert_eq!(
            updatable_items_for_target(&state, "brew"),
            vec!["git".to_string(), "visual-studio-code".to_string()]
        );
    }
}
