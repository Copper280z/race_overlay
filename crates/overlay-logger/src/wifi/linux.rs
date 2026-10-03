//! NetworkManager through `nmcli`. Network names need no permission, and a
//! machine with two adapters can keep one on the logger and the other on the
//! paddock network.

use super::{Permission, WifiControl, WifiError, is_logger_ssid};
use std::process::Command;

pub struct Nmcli;

impl Nmcli {
    pub fn detect() -> Option<Self> {
        Command::new("nmcli")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|_| Self)
    }
}

fn nmcli(args: &[&str]) -> Result<String, WifiError> {
    let output = Command::new("nmcli")
        .args(args)
        .output()
        .map_err(|error| WifiError::Failed(format!("nmcli: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(WifiError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// Split one `nmcli -t` line into fields; `\:` and `\\` are escapes.
pub(super) fn terse_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    fields.last_mut().unwrap().push(next);
                }
            }
            ':' => fields.push(String::new()),
            c => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

impl WifiControl for Nmcli {
    fn interfaces(&self) -> Vec<String> {
        nmcli(&["-t", "-f", "DEVICE,TYPE", "device"])
            .map(|out| {
                out.lines()
                    .map(terse_fields)
                    .filter(|f| f.get(1).is_some_and(|t| t == "wifi"))
                    .map(|f| f[0].clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn permission(&self) -> Permission {
        Permission::NotNeeded
    }

    fn scan_loggers(&self, interface: &str) -> Result<Vec<String>, WifiError> {
        let out = nmcli(&[
            "-t", "-f", "SSID", "device", "wifi", "list", "ifname", interface, "--rescan", "yes",
        ])?;
        let mut ssids = out
            .lines()
            .map(|line| terse_fields(line).remove(0))
            .filter(|ssid| is_logger_ssid(ssid))
            .collect::<Vec<_>>();
        ssids.sort();
        ssids.dedup();
        Ok(ssids)
    }

    fn current(&self, interface: &str) -> Result<Option<String>, WifiError> {
        let out = nmcli(&[
            "-t",
            "-f",
            "ACTIVE,SSID",
            "device",
            "wifi",
            "list",
            "ifname",
            interface,
            "--rescan",
            "no",
        ])?;
        Ok(out
            .lines()
            .map(terse_fields)
            .find(|f| f.first().is_some_and(|active| active == "yes"))
            .and_then(|f| f.get(1).cloned())
            .filter(|ssid| !ssid.is_empty()))
    }

    fn join(&self, interface: &str, ssid: &str) -> Result<(), WifiError> {
        nmcli(&[
            "--wait", "30", "device", "wifi", "connect", ssid, "ifname", interface,
        ])
        .map(|_| ())
        .map_err(|error| match error {
            WifiError::Failed(text) if text.contains("No network with SSID") => {
                WifiError::NotFound(ssid.to_owned())
            }
            other => other,
        })
    }

    fn restore(&self, interface: &str, previous: Option<&str>) -> Result<(), WifiError> {
        match previous {
            // Reuses the saved profile and its credentials.
            Some(ssid) => self.join(interface, ssid),
            None => nmcli(&["device", "disconnect", interface]).map(|_| ()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_escaped_terse_output() {
        assert_eq!(terse_fields(r"yes:AiM-MYC6-1\:2"), ["yes", "AiM-MYC6-1:2"]);
        assert_eq!(terse_fields("wlan0:wifi"), ["wlan0", "wifi"]);
        assert_eq!(terse_fields(""), [""]);
    }
}
