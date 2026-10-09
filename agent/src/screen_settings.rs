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
/// Present once settings have been checked for the pre-proxy menu, so an owner who picks
/// the old four features again keeps them.
const MIGRATED: &str = "/data/local/tmp/openui-devui/menu-v2";
/// Features the screen menu can show, in menu order; at most four at a time.
const MENU_ITEMS: [&str; 6] = [
    "overview",
    "proxy",
    "wireguard",
    "clash",
    "profiles",
    "devices",
];
const DEFAULT_MENU: [&str; 4] = ["overview", "proxy", "wireguard", "clash"];
/// The default before Proxy and Clash existed: every feature there was.
const LEGACY_MENU: [&str; 4] = ["overview", "wireguard", "profiles", "devices"];
/// Pages a screen built before Proxy and Clash knows (bits of its `C` record mask).
const LEGACY_PAGES: u32 = 0b1_1011;
/// WireGuard (bit 0) with device routing (1) and saved profiles (3): such a screen opens
/// those two only while their own bits are set, not through WireGuard.
const WIREGUARD_PAGES: u32 = 0b1011;
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
            menu_items: DEFAULT_MENU.map(String::from).to_vec(),
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
        if self.menu_items.is_empty() {
            return Err("Choose at least one menu item");
        }
        // The screen has room for four menu rows.
        if self.menu_items.len() > 4 {
            return Err("Choose at most four menu items");
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.menu_items {
            if !MENU_ITEMS.contains(&item.as_str()) || !seen.insert(item) {
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
    /// `C` for screens built before Proxy and Clash, which drop a record naming a page
    /// they lack: only their pages, all of them when none is chosen, and the menu as the
    /// startup page in place of a new one. WireGuard, Proxy or Clash brings all of
    /// WireGuard's pages, so such a screen keeps its profiles and device routing (the
    /// new screen reaches those through WireGuard). `V` carries the full choice; page
    /// numbers: wireguard 0, devices 1, menu 2, profiles 3, overview 4, proxy 5, clash 6.
    pub fn projection(&self) -> String {
        let page = |s: &str| match s {
            "wireguard" => 0,
            "devices" => 1,
            "profiles" => 3,
            "overview" => 4,
            "proxy" => 5,
            "clash" => 6,
            _ => 2,
        };
        let mask = self.menu_items.iter().fold(0u32, |m, s| m | (1 << page(s)));
        let home = page(&self.home_page);
        let mut legacy = match mask & LEGACY_PAGES {
            0 => LEGACY_PAGES,
            m => m,
        };
        let tunnel = |s: &String| matches!(s.as_str(), "wireguard" | "proxy" | "clash");
        if self.menu_items.iter().any(tunnel) {
            legacy |= WIREGUARD_PAGES;
        }
        let legacy_home = if legacy & (1 << home) != 0 { home } else { 2 };
        let line = |kind: char, home: u32, mask: u32| {
            format!(
                "{kind} {} {} {} {home} {mask} {}\n",
                self.revision, self.idle_seconds, self.brightness, self.language
            )
        };
        line('C', legacy_home, legacy) + &line('V', home, mask)
    }
}
/// Settings saved with the old default menu showed every feature there was; they get the
/// new default once. Any other list is the owner's choice and stays.
fn migrate(s: &Settings) -> Option<Settings> {
    let legacy = s.menu_items.len() == LEGACY_MENU.len()
        && LEGACY_MENU
            .iter()
            .all(|i| s.menu_items.iter().any(|m| m == i));
    if !legacy {
        return None;
    }
    let menu_items: Vec<String> = DEFAULT_MENU.map(String::from).to_vec();
    let home_page = if menu_items.contains(&s.home_page) {
        s.home_page.clone()
    } else {
        "menu".into()
    };
    Some(Settings {
        // A running screen applies settings when the revision changes.
        revision: s.revision.saturating_add(1),
        menu_items,
        home_page,
        ..s.clone()
    })
}
fn persist(s: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(DIR)?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))?;
    storage::atomic_write(Path::new(FILE), &serde_json::to_vec_pretty(s)?)?;
    // Best effort: without it the next start checks the menu again.
    let _ = storage::atomic_write(Path::new(MIGRATED), b"2\n");
    Ok(())
}
fn state() -> &'static Mutex<Settings> {
    static STATE: OnceLock<Mutex<Settings>> = OnceLock::new();
    STATE.get_or_init(|| {
        let saved = fs::read(FILE)
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .filter(|s| s.validate().is_ok());
        let settings = match saved {
            Some(s) if !Path::new(MIGRATED).exists() => match migrate(&s) {
                // Kept in memory even if it cannot be saved; the check repeats next start.
                Some(next) => {
                    let _ = persist(&next);
                    next
                }
                None => {
                    let _ = storage::atomic_write(Path::new(MIGRATED), b"2\n");
                    s
                }
            },
            Some(s) => s,
            None => Settings::default(),
        };
        Mutex::new(settings)
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
    if persist(&next).is_err() {
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
    /// The page mask of the `C` record, the one screens before Proxy and Clash read.
    fn old_screen_mask(s: &Settings) -> u32 {
        let projection = s.projection();
        let c = projection.lines().next().unwrap();
        assert!(c.starts_with("C "), "{c}");
        c.split(' ').nth(5).unwrap().parse().unwrap()
    }
    #[test]
    fn validation_and_conflicts() {
        let old = Settings::default();
        let b = body();
        let next = prepare(&old, &serde_json::to_vec(&b).unwrap()).unwrap();
        assert_eq!(next.revision, 2);
        assert!(next.shortcut_enabled);
        // An old screen opens profiles and devices only from its menu: WireGuard brings them.
        assert_eq!(
            next.projection(),
            "C 2 90 160 4 27 zh\nV 2 90 160 4 17 zh\n"
        );
        for (key, value) in [
            ("brightness", json!(0)),
            ("brightness", json!(256)),
            ("idle_seconds", json!(29)),
            ("idle_seconds", json!(601)),
            ("menu_items", json!([])),
            ("menu_items", json!(["overview", "overview"])),
            ("menu_items", json!(["overview", "nodes"])),
            (
                "menu_items",
                json!(["overview", "proxy", "wireguard", "clash", "devices"]),
            ),
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
        assert_eq!(
            english.projection(),
            "C 2 90 160 4 27 en\nV 2 90 160 4 17 en\n"
        );
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
    #[test]
    fn proxy_and_clash_pages_keep_old_screens_working() {
        // The new default: old screens get their four pages (Proxy and Clash bring
        // WireGuard's), the new screen gets the choice.
        assert_eq!(
            Settings::default().projection(),
            "C 1 120 128 2 27 zh\nV 1 120 128 2 113 zh\n"
        );
        assert_eq!(old_screen_mask(&Settings::default()) & 0b1_1011, 0b1_1011);
        let choose = |items: Value, home: &str| {
            let mut b = body();
            b["menu_items"] = items;
            b["home_page"] = json!(home);
            prepare(&Settings::default(), &serde_json::to_vec(&b).unwrap()).unwrap()
        };
        // No page an old screen knows: it shows all of them and starts at the menu.
        assert_eq!(
            choose(json!(["proxy", "clash"]), "clash").projection(),
            "C 2 90 160 2 27 zh\nV 2 90 160 6 96 zh\n"
        );
        assert_eq!(
            choose(
                json!(["devices", "proxy", "profiles", "overview"]),
                "devices"
            )
            .projection(),
            "C 2 90 160 1 27 zh\nV 2 90 160 1 58 zh\n"
        );
        // Without WireGuard, profiles and devices still reach an old screen through Clash.
        let clash = choose(json!(["overview", "proxy", "clash"]), "overview");
        assert_eq!(
            clash.projection(),
            "C 2 90 160 4 27 zh\nV 2 90 160 4 112 zh\n"
        );
        assert_eq!(old_screen_mask(&clash) & 0b1011, 0b1011);
        for (items, mask) in [
            (json!(["clash"]), 0b1_1011),
            (json!(["wireguard"]), 0b1011),
            (json!(["proxy", "overview"]), 0b1_1011),
            // No tunnel chosen: only the pages chosen.
            (json!(["overview"]), 0b1_0000),
            (json!(["profiles", "overview"]), 0b1_1000),
            (json!(["devices"]), 0b10),
        ] {
            assert_eq!(
                old_screen_mask(&choose(items.clone(), "menu")),
                mask,
                "{items}"
            );
        }
    }
    #[test]
    fn the_old_default_menu_moves_to_the_new_one_once() {
        let saved = |items: &[&str], home: &str| Settings {
            revision: 7,
            menu_items: items.iter().map(|s| s.to_string()).collect(),
            home_page: home.into(),
            shortcut_enabled: true,
            ..Settings::default()
        };
        let old = saved(
            &["devices", "profiles", "wireguard", "overview"],
            "profiles",
        );
        let next = migrate(&old).unwrap();
        assert_eq!(next.menu_items, DEFAULT_MENU.to_vec());
        assert_eq!((next.revision, next.home_page.as_str()), (8, "menu"));
        assert!(next.shortcut_enabled && next.validate().is_ok());
        // An old screen still shows all four pages it had before the move.
        assert_eq!(old_screen_mask(&old), LEGACY_PAGES);
        assert_eq!(old_screen_mask(&next), LEGACY_PAGES);
        let kept = migrate(&saved(&LEGACY_MENU, "wireguard")).unwrap();
        assert_eq!(kept.home_page, "wireguard");
        for custom in [
            &["overview", "wireguard"][..],
            &["overview", "wireguard", "profiles", "clash"],
            &DEFAULT_MENU,
        ] {
            assert!(migrate(&saved(custom, "menu")).is_none(), "{custom:?}");
        }
    }
}
