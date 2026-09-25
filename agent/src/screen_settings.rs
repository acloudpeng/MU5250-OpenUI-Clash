//! Persisted DevUI preferences. No executable content or WireGuard configuration.
use crate::storage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Mutex, OnceLock},
};
const DIR: &str = "/data/local/tmp/openui-devui";
const FILE: &str = "/data/local/tmp/openui-devui/settings.json";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub revision: u64,
    pub shortcut_enabled: bool,
    pub brightness: u16,
    pub idle_seconds: u16,
    pub home_page: String,
    pub menu_items: Vec<String>,
    /// Screen language; follows the web panel's language switch.
    #[serde(default = "default_language")]
    pub language: String,
}
fn default_language() -> String {
    "zh".into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 1,
            shortcut_enabled: false,
            brightness: 128,
            idle_seconds: 120,
            home_page: "menu".into(),
            menu_items: ["overview", "wireguard", "profiles", "devices"]
                .map(String::from)
                .to_vec(),
            language: default_language(),
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<(), &'static str> {
        if !(20..=255).contains(&self.brightness) {
            return Err("Brightness must be between 20 and 255");
        }
        if !(30..=600).contains(&self.idle_seconds) {
            return Err("Idle return must be between 30 and 600 seconds");
        }
        if self.menu_items.is_empty() || self.menu_items.len() > 4 {
            return Err("Choose at least one menu item");
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.menu_items {
            if !["overview", "wireguard", "profiles", "devices"].contains(&item.as_str())
                || !seen.insert(item)
            {
                return Err("Invalid or duplicate menu item");
            }
        }
        if self.home_page != "menu" && !self.menu_items.contains(&self.home_page) {
            return Err("The startup page must be visible in the menu");
        }
        if !["zh", "en"].contains(&self.language.as_str()) {
            return Err("Language must be zh or en");
        }
        Ok(())
    }
    pub fn projection(&self) -> String {
        let page = |s: &str| match s {
            "wireguard" => 0,
            "devices" => 1,
            "profiles" => 3,
            "overview" => 4,
            _ => 2,
        };
        let mask = self.menu_items.iter().fold(0u32, |m, s| m | (1 << page(s)));
        format!(
            "C {} {} {} {} {} {}\n",
            self.revision,
            self.idle_seconds,
            self.brightness,
            page(&self.home_page),
            mask,
            self.language
        )
    }
}
fn state() -> &'static Mutex<Settings> {
    static STATE: OnceLock<Mutex<Settings>> = OnceLock::new();
    STATE.get_or_init(|| {
        let saved = fs::read(FILE)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .filter(|s| s.validate().is_ok());
        Mutex::new(saved.unwrap_or_default())
    })
}
pub fn current() -> Settings {
    state().lock().unwrap().clone()
}
pub fn get() -> (u16, Value) {
    (200, json!({"ok":true,"data":current()}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    expected_revision: u64,
    shortcut_enabled: bool,
    brightness: u16,
    idle_seconds: u16,
    home_page: String,
    menu_items: Vec<String>,
    /// Optional: the language normally changes through `PUT /api/screen/language`.
    language: Option<String>,
}
/// A validated settings change, or the HTTP status and message to reject it with.
type Prepared = Result<Settings, (u16, &'static str)>;
fn next_revision(old: &Settings) -> Result<u64, (u16, &'static str)> {
    old.revision
        .checked_add(1)
        .ok_or((400, "Revision limit reached"))
}
fn prepare(old: &Settings, body: &[u8]) -> Prepared {
    let u: Update = serde_json::from_slice(body).map_err(|_| (400, "Invalid DevUI settings"))?;
    if u.expected_revision != old.revision {
        return Err((409, "DevUI settings changed. Reload before saving."));
    }
    let s = Settings {
        revision: next_revision(old)?,
        shortcut_enabled: u.shortcut_enabled,
        brightness: u.brightness,
        idle_seconds: u.idle_seconds,
        home_page: u.home_page,
        menu_items: u.menu_items,
        language: u.language.unwrap_or_else(|| old.language.clone()),
    };
    s.validate().map_err(|e| (400, e))?;
    Ok(s)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Language {
    language: String,
}
/// Unversioned on purpose: the last language chosen in any browser wins, and
/// an unchanged language keeps the revision so the screen does not redraw.
fn prepare_language(old: &Settings, body: &[u8]) -> Prepared {
    let l: Language = serde_json::from_slice(body).map_err(|_| (400, "Invalid language"))?;
    if l.language == old.language {
        return Ok(old.clone());
    }
    let s = Settings {
        revision: next_revision(old)?,
        language: l.language,
        ..old.clone()
    };
    s.validate().map_err(|e| (400, e))?;
    Ok(s)
}
fn save(prepare: fn(&Settings, &[u8]) -> Prepared, body: &[u8]) -> (u16, Value) {
    let mut saved = state().lock().unwrap();
    let next = match prepare(&saved, body) {
        Ok(v) => v,
        Err((code, e)) => return (code, json!({"ok":false,"error":e})),
    };
    if next.revision == saved.revision {
        return (200, json!({"ok":true,"data":*saved}));
    }
    let persist = || -> std::io::Result<()> {
        fs::create_dir_all(DIR)?;
        fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))?;
        storage::atomic_write(Path::new(FILE), &serde_json::to_vec_pretty(&next)?)
    };
    if persist().is_err() {
        return (
            500,
            json!({"ok":false,"error":"Cannot save DevUI settings"}),
        );
    }
    *saved = next;
    (200, json!({"ok":true,"data":*saved}))
}
pub fn update(body: &[u8]) -> (u16, Value) {
    save(prepare, body)
}
pub fn set_language(body: &[u8]) -> (u16, Value) {
    save(prepare_language, body)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> Value {
        json!({"expected_revision":1,"shortcut_enabled":true,"brightness":160,"idle_seconds":90,"home_page":"overview","menu_items":["overview","wireguard"]})
    }
    #[test]
    fn validation_and_conflicts() {
        let old = Settings::default();
        let b = body();
        let next = prepare(&old, &serde_json::to_vec(&b).unwrap()).unwrap();
        assert_eq!(next.revision, 2);
        assert!(next.shortcut_enabled);
        assert_eq!(next.projection(), "C 2 90 160 4 17 zh\n");
        for (key, value) in [
            ("brightness", json!(0)),
            ("brightness", json!(256)),
            ("idle_seconds", json!(29)),
            ("idle_seconds", json!(601)),
            ("menu_items", json!([])),
            ("menu_items", json!(["overview", "overview"])),
            ("home_page", json!("devices")),
            ("language", json!("fr")),
            ("command", json!("reboot")),
        ] {
            let mut bad = b.clone();
            bad[key] = value;
            assert_eq!(
                prepare(&old, &serde_json::to_vec(&bad).unwrap())
                    .unwrap_err()
                    .0,
                400
            );
        }
        assert_eq!(
            prepare(&next, &serde_json::to_vec(&b).unwrap())
                .unwrap_err()
                .0,
            409
        );
        assert!(prepare(&old, b"{}").is_err());
        let mut english = b.clone();
        english["language"] = json!("en");
        let english = prepare(&old, &serde_json::to_vec(&english).unwrap()).unwrap();
        assert_eq!(english.projection(), "C 2 90 160 4 17 en\n");
        // Saving without a language keeps the current one.
        let mut later = b.clone();
        later["expected_revision"] = json!(2);
        assert_eq!(
            prepare(&english, &serde_json::to_vec(&later).unwrap())
                .unwrap()
                .language,
            "en"
        );
    }
    #[test]
    fn language_follows_the_web_panel() {
        let old = Settings::default();
        assert_eq!(old.language, "zh");
        let same = prepare_language(&old, br#"{"language":"zh"}"#).unwrap();
        assert_eq!(same.revision, old.revision);
        let en = prepare_language(&old, br#"{"language":"en"}"#).unwrap();
        assert_eq!((en.revision, en.language.as_str()), (2, "en"));
        assert!(en.projection().ends_with(" en\n"));
        for bad in [
            &br#"{"language":"fr"}"#[..],
            br#"{"language":"en","x":1}"#,
            b"{}",
        ] {
            assert_eq!(prepare_language(&old, bad).unwrap_err().0, 400);
        }
        // Settings saved before the language existed still load, as Chinese.
        let legacy: Settings = serde_json::from_value(json!({"revision":4,"shortcut_enabled":false,"brightness":128,"idle_seconds":120,"home_page":"menu","menu_items":["overview"]})).unwrap();
        assert_eq!(legacy.language, "zh");
    }
}
