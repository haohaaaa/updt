pub fn first_json_payload(output: &str) -> Option<&str> {
    let start = output.find('{')?;
    let bytes = output.as_bytes();
    let mut scanner = JsonPayloadScanner::default();

    for (idx, b) in bytes.iter().enumerate().skip(start) {
        if scanner.consume(*b) {
            return output.get(start..=idx);
        }
    }
    None
}

#[derive(Default)]
struct JsonPayloadScanner {
    depth: i32,
    in_string: bool,
    escaped: bool,
}

impl JsonPayloadScanner {
    fn consume(&mut self, byte: u8) -> bool {
        if self.in_string {
            self.consume_string_byte(byte);
            false
        } else {
            self.consume_json_byte(byte)
        }
    }

    fn consume_string_byte(&mut self, byte: u8) {
        if self.escaped {
            self.escaped = false;
            return;
        }
        match byte {
            b'\\' => self.escaped = true,
            b'"' => self.in_string = false,
            _ => {}
        }
    }

    fn consume_json_byte(&mut self, byte: u8) -> bool {
        match byte {
            b'"' => self.in_string = true,
            b'{' => self.depth += 1,
            b'}' => {
                self.depth -= 1;
                return self.depth == 0;
            }
            _ => {}
        }
        false
    }
}

pub fn first_token(line: &str) -> Option<String> {
    line.split_whitespace().next().map(ToOwned::to_owned)
}

pub fn parse_cargo_list(output: &str) -> Result<Vec<String>, ()> {
    let mut pkgs = Vec::new();
    for raw in output.lines() {
        if let Some(pkg) = parse_cargo_list_line(raw)? {
            pkgs.push(pkg);
        }
    }
    Ok(pkgs)
}

fn parse_cargo_list_line(raw: &str) -> Result<Option<String>, ()> {
    let line = raw.trim();
    if cargo_list_line_ignored(line) {
        return Ok(None);
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    parse_cargo_package_row(&parts)
}

fn cargo_list_line_ignored(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("Polling registry ")
        || line.split_whitespace().collect::<Vec<_>>().as_slice()
            == ["Package", "Installed", "Latest", "Needs", "update"]
}

fn parse_cargo_package_row(parts: &[&str]) -> Result<Option<String>, ()> {
    if !cargo_package_row_shape(parts) {
        return Ok(None);
    }
    cargo_package_update(parts[0], parts[3])
}

fn cargo_package_row_shape(parts: &[&str]) -> bool {
    parts.len() == 4
        && parts[1].starts_with('v')
        && (parts[2].starts_with('v') || parts[2] == "N/A")
}

fn cargo_package_update(name: &str, needs_update: &str) -> Result<Option<String>, ()> {
    match needs_update {
        "Yes" => Ok(Some(name.to_string())),
        "No" => Ok(None),
        _ => Err(()),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScoopStatusOutput {
    pub updatable_items: Vec<String>,
    pub metadata_outdated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoopListItem {
    pub name: String,
    pub is_global: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoopBlockedProcessInfo {
    pub app_name: Option<String>,
    pub details: Vec<String>,
}

#[derive(Clone, Copy)]
struct ScoopStatusColumns {
    installed_start: usize,
    latest_start: usize,
    latest_end: usize,
}

impl ScoopStatusColumns {
    fn from_header(line: &str) -> Option<Self> {
        let installed_start = line.find("Installed Version")?;
        let latest_start = line.find("Latest Version")?;
        if !line[..installed_start].contains("Name") {
            return None;
        }

        let latest_end = line
            .find("Missing Dependencies")
            .or_else(|| line.find("Info"))
            .unwrap_or(line.len());
        Some(Self {
            installed_start,
            latest_start,
            latest_end,
        })
    }

    fn outdated_name(self, line: &str) -> Option<String> {
        let name = slice_column(line, 0, self.installed_start).trim();
        let latest = slice_column(line, self.latest_start, self.latest_end).trim();
        (!name.is_empty() && !latest.is_empty()).then(|| name.to_string())
    }
}

#[derive(Clone, Copy)]
struct ScoopListColumns {
    version_start: usize,
    info_start: Option<usize>,
}

impl ScoopListColumns {
    fn from_header(line: &str) -> Option<Self> {
        let version_start = line.find("Version")?;
        let source_start = line.find("Source")?;
        if !line[..version_start].contains("Name") || version_start >= source_start {
            return None;
        }

        Some(Self {
            version_start,
            info_start: line.find("Info"),
        })
    }

    fn parse_item(self, line: &str) -> Option<ScoopListItem> {
        let name = slice_column(line, 0, self.version_start).trim();
        if name.is_empty() {
            return None;
        }
        let info = self
            .info_start
            .map(|start| slice_column(line, start, line.len()).trim())
            .unwrap_or("");
        Some(ScoopListItem {
            name: name.to_string(),
            is_global: info.contains("Global install"),
        })
    }
}

fn slice_column(line: &str, start: usize, end: usize) -> &str {
    let len = line.len();
    if start >= len || start >= end {
        return "";
    }
    let end = end.min(len);
    line.get(start..end).unwrap_or("")
}

fn is_scoop_status_separator(line: &str) -> bool {
    line.chars().all(|ch| ch == '-' || ch == ' ' || ch == '\t')
}

fn is_scoop_status_noise(line: &str) -> bool {
    matches!(
        line,
        "Everything is ok!" | "Scoop is up to date." | "Scoop was updated successfully!"
    )
}

fn is_scoop_list_noise(line: &str) -> bool {
    line == "Installed apps:"
}

pub fn parse_scoop_status_output(output: &str) -> ScoopStatusOutput {
    let cleaned = strip_ansi_control_sequences(output);
    let mut parsed = ScoopStatusOutput::default();
    let mut columns = None;

    for raw in cleaned.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || is_scoop_status_noise(trimmed) {
            continue;
        }

        if trimmed.starts_with("WARN") {
            if trimmed.contains("Scoop out of date") || trimmed.contains("bucket(s) out of date") {
                parsed.metadata_outdated = true;
            }
            continue;
        }

        if let Some(header) = ScoopStatusColumns::from_header(line) {
            columns = Some(header);
            continue;
        }
        if is_scoop_status_separator(trimmed) {
            continue;
        }

        if let Some(columns) = columns {
            if let Some(name) = columns.outdated_name(line)
                && !parsed.updatable_items.iter().any(|item| item == &name)
            {
                parsed.updatable_items.push(name);
            }
            continue;
        }

        if let Some(name) = first_token(trimmed)
            && !matches!(name.as_str(), "Name" | "----")
            && !parsed.updatable_items.iter().any(|item| item == &name)
        {
            parsed.updatable_items.push(name);
        }
    }

    parsed
}

pub fn parse_scoop_list_output(output: &str) -> Result<Vec<ScoopListItem>, ()> {
    let cleaned = strip_ansi_control_sequences(output);
    let mut items = Vec::new();
    let mut columns = None;
    let mut saw_content = false;

    for raw in cleaned.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || is_scoop_list_noise(trimmed) {
            continue;
        }
        saw_content = true;

        if let Some(header) = ScoopListColumns::from_header(line) {
            columns = Some(header);
            continue;
        }
        if is_scoop_status_separator(trimmed) {
            continue;
        }

        let Some(columns) = columns else {
            return Err(());
        };
        let Some(item) = columns.parse_item(line) else {
            return Err(());
        };
        items.push(item);
    }

    if !saw_content {
        return Ok(items);
    }
    if columns.is_none() {
        return Err(());
    }

    Ok(items)
}

pub fn parse_scoop_blocked_process_output(output: &str) -> Option<ScoopBlockedProcessInfo> {
    const HEADER_MARKER: &str = "The following instances of \"";
    const HEADER_SUFFIX: &str = "\" are still running. Close them and try again.";
    const SKIP_MARKER: &str = "Running process detected, skip updating.";

    let cleaned = strip_ansi_control_sequences(output);
    let lines = cleaned.lines().map(str::trim_end).collect::<Vec<_>>();
    let mut header_index = None;
    let mut skip_index = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if header_index.is_none()
            && trimmed.contains(HEADER_MARKER)
            && trimmed.contains(HEADER_SUFFIX)
        {
            header_index = Some(idx);
        }
        if skip_index.is_none() && trimmed.contains(SKIP_MARKER) {
            skip_index = Some(idx);
        }
    }

    if header_index.is_none() && skip_index.is_none() {
        return None;
    }

    let app_name = header_index.and_then(|idx| {
        let line = lines[idx].trim();
        let start = line.find(HEADER_MARKER)? + HEADER_MARKER.len();
        let rest = line.get(start..)?;
        let end = rest.find(HEADER_SUFFIX)?;
        Some(rest[..end].to_string())
    });

    let details = if let Some(start) = header_index {
        let end = skip_index.unwrap_or(lines.len());
        lines[start..end]
            .iter()
            .map(|line| line.trim_end())
            .filter(|line| !line.trim().is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        vec![SKIP_MARKER.to_string()]
    };

    Some(ScoopBlockedProcessInfo { app_name, details })
}

pub fn strip_ansi_control_sequences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek().is_some_and(|next| *next == '[') {
                let _ = chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        ScoopBlockedProcessInfo, ScoopListItem, ScoopStatusOutput,
        parse_scoop_blocked_process_output, parse_scoop_list_output, parse_scoop_status_output,
    };

    #[test]
    fn parses_scoop_status_table_outdated_items() {
        let output = "\
Scoop is up to date.\n\
\n\
Name          Installed Version Latest Version Missing Dependencies Info\n\
----          ----------------- -------------- -------------------- ----\n\
7zip          24.09             25.01\n\
git           2.45.2.windows.1  2.46.0.windows.1\n\
dependency    1.0                              innounp\n";

        assert_eq!(
            parse_scoop_status_output(output),
            ScoopStatusOutput {
                updatable_items: vec!["7zip".to_string(), "git".to_string()],
                metadata_outdated: false,
            }
        );
    }

    #[test]
    fn detects_scoop_metadata_outdated_warning() {
        let output =
            "WARN  Scoop bucket(s) out of date. Run 'scoop update' to get the latest changes.\n";

        assert_eq!(
            parse_scoop_status_output(output),
            ScoopStatusOutput {
                updatable_items: Vec::new(),
                metadata_outdated: true,
            }
        );
    }

    #[test]
    fn supports_legacy_name_only_scoop_status_output() {
        let output = "7zip\ngit\n";

        assert_eq!(
            parse_scoop_status_output(output),
            ScoopStatusOutput {
                updatable_items: vec!["7zip".to_string(), "git".to_string()],
                metadata_outdated: false,
            }
        );
    }

    #[test]
    fn parses_scoop_list_local_only_item() {
        let output = "\
Installed apps:\n\
\n\
Name                     Version    Source Updated             Info\n\
----                     -------    ------ -------             ----\n\
git                      2.54.0     main   2026-04-25 12:58:13\n";

        assert_eq!(
            parse_scoop_list_output(output),
            Ok(vec![ScoopListItem {
                name: "git".to_string(),
                is_global: false,
            }])
        );
    }

    #[test]
    fn parses_current_scoop_list_without_updated_and_info_columns() {
        let output = "\
Installed apps:\n\
\n\
Name                      Version            Source\n\
----                      -------            ------\n\
7zip                      26.03              main\n\
clash-verge-rev           2.5.4              C:\\Users\\user\\scoop\\buckets\\extras\\bucket\n";

        assert_eq!(
            parse_scoop_list_output(output),
            Ok(vec![
                ScoopListItem {
                    name: "7zip".to_string(),
                    is_global: false,
                },
                ScoopListItem {
                    name: "clash-verge-rev".to_string(),
                    is_global: false,
                },
            ])
        );
    }

    #[test]
    fn parses_scoop_list_global_only_item() {
        let output = "\
Installed apps:\n\
\n\
Name                     Version    Source Updated             Info\n\
----                     -------    ------ -------             ----\n\
git                      2.54.0     main   2026-04-25 12:58:13 Global install\n";

        assert_eq!(
            parse_scoop_list_output(output),
            Ok(vec![ScoopListItem {
                name: "git".to_string(),
                is_global: true,
            }])
        );
    }

    #[test]
    fn parses_scoop_list_same_app_in_local_and_global_scope() {
        let output = "\
Installed apps:\n\
\n\
Name                     Version    Source Updated             Info\n\
----                     -------    ------ -------             ----\n\
git                      2.54.0     main   2026-04-25 12:58:13\n\
git                      2.54.0     main   2026-04-25 12:58:14 Global install\n";

        assert_eq!(
            parse_scoop_list_output(output),
            Ok(vec![
                ScoopListItem {
                    name: "git".to_string(),
                    is_global: false,
                },
                ScoopListItem {
                    name: "git".to_string(),
                    is_global: true,
                },
            ])
        );
    }

    #[test]
    fn parses_scoop_list_without_global_marker() {
        let output = "\
Installed apps:\n\
\n\
Name                     Version    Source Updated             Info\n\
----                     -------    ------ -------             ----\n\
git                      2.54.0     main   2026-04-25 12:58:13 Deprecated package\n";

        assert_eq!(
            parse_scoop_list_output(output),
            Ok(vec![ScoopListItem {
                name: "git".to_string(),
                is_global: false,
            }])
        );
    }

    #[test]
    fn parses_scoop_blocked_process_output() {
        let output = "\
ERROR The following instances of \"git\" are still running. Close them and try again.\n\
\n\
Handles  NPM(K)    PM(K)      WS(K)     CPU(s)     Id  SI ProcessName\n\
-------  ------    -----      -----     ------     --  -- -----------\n\
    123      10    10000      20000       0.10   1234   1 git\n\
\n\
Running process detected, skip updating.\n";

        assert_eq!(
            parse_scoop_blocked_process_output(output),
            Some(ScoopBlockedProcessInfo {
                app_name: Some("git".to_string()),
                details: vec![
                    "ERROR The following instances of \"git\" are still running. Close them and try again.".to_string(),
                    "Handles  NPM(K)    PM(K)      WS(K)     CPU(s)     Id  SI ProcessName".to_string(),
                    "-------  ------    -----      -----     ------     --  -- -----------".to_string(),
                    "123      10    10000      20000       0.10   1234   1 git".to_string(),
                ],
            })
        );
    }

    #[test]
    fn ignores_normal_scoop_update_output_for_blocked_process_detection() {
        let output = "\
Updating 'git' (2.54.0 -> 2.55.0)\n\
Downloading new version\n\
'git' (2.55.0) was installed successfully!\n";

        assert_eq!(parse_scoop_blocked_process_output(output), None);
    }

    #[test]
    fn ignores_unrelated_failures_for_blocked_process_detection() {
        let output = "ERROR hash check failed\nInstallation aborted.\n";

        assert_eq!(parse_scoop_blocked_process_output(output), None);
    }

    #[test]
    fn parses_cargo_list_output() {
        let output = "\
    Updating registry `https://github.com/rust-lang/crates.io-index`
    Updating git repository `https://github.com/nabijaczleweli/cargo-update`
warning: some warning message about crate locks
Package         Installed  Latest    Needs update
cargo-binstall  v1.19.1    v1.20.0   Yes
cargo-deny      v0.19.8    v0.19.9   Yes
cbindgen        v0.29.3    v0.29.4   Yes
charasay        v3.3.0     v3.3.1    Yes
updt            v0.2.0     v0.2.1    Yes
bindgen-cli     v0.72.1    v0.72.1   No
bluz            v0.3.1     v0.3.1    No
cargo-about     v0.9.0     v0.9.0    No
cargo-edit      v0.13.11   v0.13.11  No
cargo-expand    v1.0.122   v1.0.122  No
cargo-nextest   v0.9.137   v0.9.137  No
cargo-outdated  v0.19.0    v0.19.0   No
cargo-update    v20.0.2    v20.0.2   No
kaleidoscope    v0.2.0     N/A       No
mmisay          v0.1.0     v0.1.0    No
names           v0.14.0    v0.14.0   No
rusty-rain      v0.5.0     v0.5.0    No
tlrc            v1.13.1    v1.13.1   No
";
        let expected = vec![
            "cargo-binstall".to_string(),
            "cargo-deny".to_string(),
            "cbindgen".to_string(),
            "charasay".to_string(),
            "updt".to_string(),
        ];
        assert_eq!(super::parse_cargo_list(output), Ok(expected));
    }
}
