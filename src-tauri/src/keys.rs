//! Global hotkeys. Defaults below; your changes are saved in %APPDATA%\Kakkoii\keys.json.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::engine::Engine;
use crate::{recorder, shot};

/// What each key does, and its default.
pub const DEFAULTS: [(&str, &str); 4] = [
    ("orb", "Alt+K"),           // the orb and its menu
    ("record", "Alt+Shift+R"),  // open the bar, or stop if recording
    ("pause", "Alt+Shift+P"),   // pause or carry on
    ("shot", "Alt+Shift+S"),    // a screenshot
];

pub struct Keys {
    map: Mutex<BTreeMap<String, String>>,
    path: PathBuf,
}

impl Keys {
    pub fn load(dir: PathBuf) -> Self {
        let path = dir.join("keys.json");
        let saved: BTreeMap<String, String> =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let map = DEFAULTS
            .iter()
            .map(|(a, d)| {
                let k = saved.get(*a).filter(|k| k.parse::<Shortcut>().is_ok()).cloned().unwrap_or_else(|| d.to_string());
                (a.to_string(), k)
            })
            .collect();
        Self { map: Mutex::new(map), path }
    }

    pub fn all(&self) -> BTreeMap<String, String> {
        self.map.lock().unwrap().clone()
    }

    pub fn register_all(&self, app: &AppHandle) {
        for (a, k) in self.all() {
            if let Ok(sc) = k.parse::<Shortcut>() {
                if app.global_shortcut().register(sc).is_err() {
                    recorder::log(&format!("hotkey {k} for {a} is taken by another app"));
                }
            }
        }
    }

    /// Change one key. Fails (and keeps the old one) if it's malformed, used by
    /// another action, or taken by another app.
    pub fn set(&self, app: &AppHandle, action: &str, accel: &str) -> Result<BTreeMap<String, String>, String> {
        let new: Shortcut = accel.parse().map_err(|_| format!("{accel} isn't a key Kakkoii can use"))?;
        let mut map = self.map.lock().unwrap();
        let old = map.get(action).cloned().ok_or("no such action")?;
        if let Some((other, _)) = map.iter().find(|(a, k)| a.as_str() != action && k.parse::<Shortcut>().ok() == Some(new)) {
            return Err(format!("{accel} already does {other}"));
        }
        let gs = app.global_shortcut();
        if let Ok(o) = old.parse::<Shortcut>() {
            let _ = gs.unregister(o);
        }
        if gs.register(new).is_err() {
            if let Ok(o) = old.parse::<Shortcut>() {
                let _ = gs.register(o);
            }
            return Err(format!("{accel} is taken by another app"));
        }
        map.insert(action.to_string(), accel.to_string());
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, serde_json::to_string_pretty(&*map).unwrap_or_default());
        Ok(map.clone())
    }

    fn action_of(&self, sc: &Shortcut) -> Option<String> {
        self.map.lock().unwrap().iter().find(|(_, k)| k.parse::<Shortcut>().ok().as_ref() == Some(sc)).map(|(a, _)| a.clone())
    }
}

/// The global-shortcut plugin's handler.
pub fn handle(app: &AppHandle, sc: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }
    let (Some(keys), Some(e)) = (app.try_state::<Keys>(), app.try_state::<Arc<Engine>>()) else { return };
    match keys.action_of(sc).as_deref() {
        Some("orb") => e.orb_menu(),
        Some("shot") => {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = shot::capture(&app, "region");
            });
        }
        Some("pause") => {
            let paused = e.status()["paused"].as_bool().unwrap_or(false);
            let _ = e.pause(!paused);
        }
        Some("record") => {
            let e = e.inner().clone();
            std::thread::spawn(move || {
                if e.recording() {
                    recorder::log("stop via hotkey");
                    let _ = e.stop();
                } else {
                    e.show_bar();
                }
            });
        }
        _ => {}
    }
}
