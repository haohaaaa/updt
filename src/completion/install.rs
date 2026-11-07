use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::PathBuf;

pub fn install_fish_completion(script: &[u8]) -> io::Result<PathBuf> {
    let config_home = env::var_os("XDG_CONFIG_HOME");
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE"));
    let path = fish_completion_path(config_home.as_deref(), home.as_deref())?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid completion path"))?;
    fs::create_dir_all(parent)?;
    match fs::read(&path) {
        Ok(current) if current == script => return Ok(path),
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::write(&path, script)?;
    Ok(path)
}

fn fish_completion_path(config_home: Option<&OsStr>, home: Option<&OsStr>) -> io::Result<PathBuf> {
    // XDG requires an absolute path; ignore empty or relative overrides.
    let config = config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no absolute XDG_CONFIG_HOME, HOME or USERPROFILE is available",
            )
        })?;
    Ok(config.join("fish").join("completions").join("updt.fish"))
}

#[cfg(test)]
mod tests {
    use super::fish_completion_path;
    use std::ffi::OsStr;
    use std::io;

    #[test]
    fn xdg_config_takes_precedence_over_home() {
        let root = std::env::temp_dir();
        let config = root.join("custom-config");
        let home = root.join("home");
        assert_eq!(
            fish_completion_path(Some(config.as_os_str()), Some(home.as_os_str())).unwrap(),
            config.join("fish/completions/updt.fish")
        );
    }

    #[test]
    fn invalid_xdg_config_falls_back_to_home() {
        let home = std::env::temp_dir().join("home");
        for config in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                fish_completion_path(config, Some(home.as_os_str())).unwrap(),
                home.join(".config/fish/completions/updt.fish")
            );
        }
    }

    #[test]
    fn missing_or_relative_config_roots_are_rejected() {
        for home in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                fish_completion_path(None, home).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
        }
    }
}
