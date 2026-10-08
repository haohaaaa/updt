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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoopExportApp {
    pub name: String,
    pub version: String,
    pub source: String,
    pub info: String,
}

impl ScoopExportApp {
    #[cfg(test)]
    pub fn new(name: &str, is_global: bool) -> Self {
        Self {
            name: name.to_string(),
            version: String::new(),
            source: String::new(),
            info: if is_global {
                "Global install".to_string()
            } else {
                String::new()
            },
        }
    }

    pub fn is_global(&self) -> bool {
        self.info.contains("Global install")
    }

    pub fn is_held(&self) -> bool {
        self.info.contains("Held package")
    }
}

pub fn parse_scoop_export_output(output: &str) -> Result<Vec<ScoopExportApp>, ()> {
    let json_text = first_json_payload(output).unwrap_or(output);
    let root = serde_json::from_str::<serde_json::Value>(json_text).map_err(|_| ())?;
    let apps = root.get("apps").and_then(|v| v.as_array()).ok_or(())?;
    let mut items = Vec::with_capacity(apps.len());
    for item in apps {
        let name = item
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let version = item
            .get("Version")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let source = item
            .get("Source")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let info = item
            .get("Info")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        items.push(ScoopExportApp {
            name: name.to_string(),
            version: version.to_string(),
            source: source.to_string(),
            info: info.to_string(),
        });
    }
    Ok(items)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoopBlockedProcessInfo {
    pub app_name: Option<String>,
    pub details: Vec<String>,
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
        ScoopBlockedProcessInfo, ScoopExportApp, parse_scoop_blocked_process_output,
        parse_scoop_export_output,
    };

    #[test]
    fn creates_scoop_export_app() {
        let app = ScoopExportApp::new("git", true);
        assert_eq!(app.name, "git");
        assert!(app.is_global());
        assert!(!app.is_held());
    }

    #[test]
    fn parses_scoop_export_apps() {
        let output = r#"{
            "apps": [
                {
                    "Name": "7zip",
                    "Version": "26.03",
                    "Source": "main",
                    "Info": ""
                },
                {
                    "Name": "git",
                    "Version": "2.54.0",
                    "Source": "main",
                    "Info": "Global install"
                },
                {
                    "Name": "curl",
                    "Version": "8.0.0",
                    "Source": "main",
                    "Info": "Held package"
                }
            ]
        }"#;

        let apps = parse_scoop_export_output(output).unwrap();
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].name, "7zip");
        assert!(!apps[0].is_global());
        assert!(!apps[0].is_held());

        assert_eq!(apps[1].name, "git");
        assert!(apps[1].is_global());
        assert!(!apps[1].is_held());

        assert_eq!(apps[2].name, "curl");
        assert!(!apps[2].is_global());
        assert!(apps[2].is_held());
    }

    #[test]
    fn parses_scoop_export_apps_with_leading_noise() {
        let output = r#"WARN  Scoop bucket(s) out of date.
{
    "apps": [
        {
            "Name": "nodejs",
            "Version": "22.0.0"
        }
    ]
}"#;
        let apps = parse_scoop_export_output(output).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "nodejs");
        assert_eq!(apps[0].version, "22.0.0");
        assert!(!apps[0].is_global());
        assert!(!apps[0].is_held());
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
