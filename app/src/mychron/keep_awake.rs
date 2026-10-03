//! Keeps the computer from idle-sleeping while Track mode waits at the track.
//! macOS uses `caffeinate`, which also exits by itself if the app dies.

use std::process::{Child, Command};

#[derive(Default)]
pub struct KeepAwake {
    child: Option<Child>,
}

impl KeepAwake {
    pub const SUPPORTED: bool = cfg!(target_os = "macos");

    pub fn set(&mut self, on: bool) {
        if on == self.child.is_some() || !Self::SUPPORTED {
            return;
        }
        if on {
            self.child = Command::new("caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .spawn()
                .ok();
        } else if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.set(false);
    }
}
