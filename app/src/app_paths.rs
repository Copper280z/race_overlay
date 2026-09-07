//! Writable platform paths used when a project does not provide its own path.

use std::path::PathBuf;

/// Storage for imagery downloaded before an Analysis workspace has been saved.
///
/// Saved workspaces keep using their adjacent `.assets` directory. This fallback
/// must be absolute because Finder-launched macOS applications can inherit `/` as
/// their working directory.
pub(crate) fn unsaved_analysis_imagery_dir() -> PathBuf {
    application_data_dir().join("Imagery")
}

#[cfg(target_os = "macos")]
fn application_data_dir() -> PathBuf {
    absolute_env_path("HOME")
        .map(|home| home.join("Library/Application Support/Race Overlay"))
        .unwrap_or_else(fallback_data_dir)
}

#[cfg(target_os = "windows")]
fn application_data_dir() -> PathBuf {
    absolute_env_path("LOCALAPPDATA")
        .map(|root| root.join("Race Overlay"))
        .unwrap_or_else(fallback_data_dir)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn application_data_dir() -> PathBuf {
    absolute_env_path("XDG_DATA_HOME")
        .map(|root| root.join("race-overlay"))
        .or_else(|| absolute_env_path("HOME").map(|home| home.join(".local/share/race-overlay")))
        .unwrap_or_else(fallback_data_dir)
}

fn absolute_env_path(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name)?);
    path.is_absolute().then_some(path)
}

fn fallback_data_dir() -> PathBuf {
    std::env::temp_dir().join("race-overlay")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsaved_imagery_storage_does_not_depend_on_the_working_directory() {
        let path = unsaved_analysis_imagery_dir();
        assert!(path.is_absolute(), "{} is not absolute", path.display());
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Imagery")
        );
    }
}
