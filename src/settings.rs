//! The user's choices, in data/settings.json.

use crate::engine::{FarMode, Policy};
use crate::paths;
use crate::places::Places;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// "my places": a siren here takes the whole screen
    pub homes: Vec<String>,
    /// a siren this close (km) is announced louder
    pub near_km: f32,
    pub far_mode: FarMode,
    pub far_sound: bool,
    pub quiet_from: u8,
    pub quiet_to: u8,
    pub vol_home: u8,
    pub vol_far: u8,
    /// the full screen goes on every monitor
    pub all_monitors: bool,
    pub autostart: bool,
    pub snooze_until_ms: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // (none until the user picks: the settings tab asks for it)
            homes: vec![],
            near_km: 15.0,
            far_mode: FarMode::Panel,
            far_sound: true,
            quiet_from: 23,
            quiet_to: 7,
            vol_home: 100,
            vol_far: 35,
            all_monitors: true,
            autostart: true,
            snooze_until_ms: 0,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        let path = paths::data_dir().join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => s,
                Err(e) => {
                    paths::log(&format!("settings.json unreadable, defaults used: {e}"));
                    let _ = std::fs::rename(&path, path.with_extension("bad.json"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) {
        let path = paths::data_dir().join("settings.json");
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = paths::write_atomic(&path, &bytes) {
                    paths::log(&format!("settings not saved: {e}"));
                }
            }
            Err(e) => paths::log(&format!("settings not saved: {e}")),
        }
    }

    pub fn policy(&self, places: &Places, mute_all: bool) -> Policy {
        Policy {
            homes: self.homes.iter().filter_map(|n| places.find(n)).collect(),
            near_km: self.near_km,
            far_mode: self.far_mode,
            far_sound: self.far_sound,
            quiet_from: self.quiet_from,
            quiet_to: self.quiet_to,
            snooze_until_ms: self.snooze_until_ms,
            mute_all,
        }
    }
}
