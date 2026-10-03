//! Joining and leaving a logger's hotspot (`AiM-MYC6-<serial>`, open, no
//! WPA) for "switch Wi-Fi for me" downloads.
//!
//! Linux drives NetworkManager through `nmcli`, macOS uses CoreWLAN (which
//! reveals network names only with Location permission), and other platforms
//! have no Wi-Fi control: users join the hotspot themselves.

// Compiled into tests everywhere so its parsing is checked on every platform.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WifiError {
    #[error("Wi-Fi control is not available on this system")]
    Unsupported,
    #[error("Location permission is needed to see Wi-Fi network names")]
    PermissionDenied,
    #[error("no Wi-Fi interface named {0}")]
    NoInterface(String),
    #[error("{0} is not in range")]
    NotFound(String),
    #[error("Wi-Fi: {0}")]
    Failed(String),
}

/// Whether the system lets this process read network names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// The platform does not gate network names.
    NotNeeded,
    Granted,
    /// Not asked yet; [`WifiControl::request_permission`] prompts.
    NotDetermined,
    Denied,
}

impl Permission {
    pub fn allows_scanning(self) -> bool {
        matches!(self, Self::NotNeeded | Self::Granted)
    }
}

pub trait WifiControl: Send {
    /// Wi-Fi interface names, e.g. `wlan1`, `en0`.
    fn interfaces(&self) -> Vec<String>;
    fn permission(&self) -> Permission;
    /// Ask the system for permission. On macOS this must run on the main
    /// thread; the answer arrives asynchronously.
    fn request_permission(&self) {}
    /// Logger hotspots currently visible on `interface`.
    fn scan_loggers(&self, interface: &str) -> Result<Vec<String>, WifiError>;
    /// Network `interface` is associated with.
    fn current(&self, interface: &str) -> Result<Option<String>, WifiError>;
    fn join(&self, interface: &str, ssid: &str) -> Result<(), WifiError>;
    /// Return `interface` to `previous`, or disconnect it when there was none.
    fn restore(&self, interface: &str, previous: Option<&str>) -> Result<(), WifiError>;
}

/// The platform's Wi-Fi control, `None` where there is none.
pub fn system() -> Option<Box<dyn WifiControl>> {
    #[cfg(target_os = "linux")]
    {
        linux::Nmcli::detect().map(|control| Box::new(control) as Box<dyn WifiControl>)
    }
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(macos::CoreWlan))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// AiM logger hotspots are named `AiM-<model>-<serial>`, e.g. `AiM-MYC6-1234`.
pub fn is_logger_ssid(ssid: &str) -> bool {
    ssid.strip_prefix("AiM-")
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(model, serial)| model.starts_with("MYC") && !serial.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_logger_hotspots() {
        assert!(is_logger_ssid("AiM-MYC6-12345"));
        assert!(!is_logger_ssid("AiM-MYC6-"));
        assert!(!is_logger_ssid("Track Wi-Fi"));
        assert!(!is_logger_ssid("MYC6-12345"));
    }
}
