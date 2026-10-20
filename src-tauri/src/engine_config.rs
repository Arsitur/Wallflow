use std::{collections::HashMap, fs, path::Path};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct Playlist { pub name: String, pub items: Vec<String> }

#[derive(Default)]
pub struct EngineConfig {
    pub playlists: Vec<Playlist>,
    pub current: HashMap<u32, String>,
    pub monitors: HashMap<String, u32>,
    pub monitor_playlists: HashMap<u32, Playlist>,
    pub properties: HashMap<String, Value>,
    pub layout: u32,
}

pub fn normalize_path(value: &str) -> String {
    let path = value.trim().trim_matches('"').replace('\\', "/");
    path.strip_prefix("//?/").unwrap_or(&path).to_lowercase()
}

impl EngineConfig {
    pub fn read(executable: &Path) -> Result<Self, String> {
        let path = executable.parent().ok_or("Wallpaper Engine directory unavailable")?.join("config.json");
        if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
            return Err("Wallpaper Engine configuration is too large".into());
        }
        let json: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        Self::parse(&json, &std::env::var("USERNAME").unwrap_or_default())
    }

    fn parse(json: &Value, username: &str) -> Result<Self, String> {
        let profiles: Vec<_> = json.as_object().ok_or("Invalid Wallpaper Engine configuration")?.iter()
            .filter(|(_, value)| value.get("general").is_some()).collect();
        let profile = profiles.iter().find(|(name, _)| name.eq_ignore_ascii_case(username))
            .or_else(|| if profiles.len() == 1 { profiles.first() } else { None })
            .ok_or("Cannot determine the active Wallpaper Engine profile")?.1;
        let general = &profile["general"];
        let mut config = Self::default();
        config.layout = general.pointer("/wallpaperconfig/layout").and_then(Value::as_u64).unwrap_or(0) as u32;
        if let Some(properties) = profile["wproperties"].as_object() {
            config.properties = properties.iter().map(|(file, value)| (normalize_path(file), value.clone())).collect();
        }
        if let Some(playlists) = general["playlists"].as_array() {
            for playlist in playlists {
                let Some(name) = playlist["name"].as_str().filter(|name| !name.is_empty()) else { continue; };
                let items = playlist["items"].as_array().map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                config.playlists.push(Playlist { name: name.to_owned(), items });
            }
        }
        if let Some(selected) = general.pointer("/wallpaperconfig/selectedwallpapers").and_then(Value::as_object) {
            for (name, wallpaper) in selected {
                if let (Some(index), Some(file)) = (name.strip_prefix("Monitor").and_then(|index| index.parse().ok()), wallpaper["file"].as_str()) {
                    config.current.insert(index, file.to_owned());
                    if let Some(name) = wallpaper.pointer("/playlist/name").and_then(Value::as_str) {
                        let items = wallpaper.pointer("/playlist/items").and_then(Value::as_array)
                            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                        config.monitor_playlists.insert(index, Playlist { name: name.to_owned(), items });
                    }
                }
            }
        }
        if let Some(monitors) = general.pointer("/user/monitormap").and_then(Value::as_object) {
            for (name, value) in monitors {
                if let Some(index) = value["location"].as_u64().and_then(|index| u32::try_from(index).ok()) {
                    config.monitors.insert(normalize_path(name), index);
                }
            }
        }
        Ok(config)
    }

    pub fn monitor_index(&self, name: &str) -> Option<u32> { self.monitors.get(&normalize_path(name)).copied() }

    pub fn wallpaper_monitor(&self, physical: u32) -> u32 {
        // Clone and stretch have one shared renderer. Other selectedwallpapers
        // entries remain on disk but cannot receive CLI wallpaper commands.
        if matches!(self.layout, 1 | 2) { 0 } else { physical }
    }

    pub fn display_mode(&self) -> &'static str {
        match self.layout { 1 => "stretch", 2 => "clone", _ => "per-monitor" }
    }

    pub fn properties_for(&self, file: &Path, monitor: u32) -> Option<Value> {
        let file = normalize_path(&file.to_string_lossy());
        self.properties.get(&file)?.get(format!("Monitor{monitor}"))
            .filter(|value| value.is_object()).cloned()
    }

    pub fn playlist(&self, name: &str) -> Option<Playlist> {
        // One shared membership on every display, including unsaved additions to this named playlist.
        let mut loaded: Vec<_> = self.monitor_playlists.iter().collect();
        loaded.sort_by_key(|(monitor, _)| **monitor);
        let sources: Vec<_> = self.playlists.iter().chain(loaded.into_iter().map(|(_, playlist)| playlist))
            .filter(|playlist| playlist.name == name).collect();
        if sources.is_empty() { return None; }
        let mut seen = std::collections::HashSet::new();
        let items = sources.into_iter().flat_map(|playlist| &playlist.items)
            .filter(|file| seen.insert(normalize_path(file))).cloned().collect();
        Some(Playlist { name: name.to_owned(), items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reads_named_playlists_active_monitor_and_display_mapping_without_recent_history() {
        let value = json!({"Artur":{"general":{
            "playlists":[{"name":"Arthurian Wallpapers","items":["C:/Steam/a/scene.pkg"]},{"name":"Empty","items":[]}],
            "user":{"monitormap":{"//./DISPLAY2":{"location":1}}},
            "wallpaperconfig":{"selectedwallpapers":{"Monitor1":{"file":"C:/Steam/a/scene.pkg"}}},
            "wallpaperconfigrecent":[{"selectedwallpapers":{"Monitor1":{"file":"wrong"}}}]
        }}});
        let config = EngineConfig::parse(&value, "artur").unwrap();
        assert_eq!(config.playlists[0].name, "Arthurian Wallpapers");
        assert!(config.playlists[1].items.is_empty());
        assert_eq!(config.current[&1], "C:/Steam/a/scene.pkg");
        assert_eq!(config.monitor_index(r"\\.\DISPLAY2"), Some(1));
        assert_eq!(normalize_path(r"\\?\C:\Steam\a\scene.pkg"), normalize_path("C:/Steam/a/scene.pkg"));
    }
    #[test]
    fn does_not_mix_profiles_when_multiple_users_exist() {
        let value = json!({"Alice":{"general":{}},"Bob":{"general":{}}});
        assert!(EngineConfig::parse(&value, "unknown").is_err());
    }
    #[test]
    fn clone_and_stretch_ignore_dormant_secondary_wallpaper_entries() {
        for layout in [0, 1, 2] {
            let value = json!({"User":{"general":{"wallpaperconfig":{
                "layout":layout, "selectedwallpapers":{"Monitor0":{"file":"active"},"Monitor1":{"file":"dormant"}}
            }}, "wproperties":{"active":{"Monitor0":{"volume":0}},"dormant":{"Monitor1":{"volume":100}}}}});
            let config = EngineConfig::parse(&value, "User").unwrap();
            let renderer = config.wallpaper_monitor(1);
            assert_eq!(renderer, if layout == 0 { 1 } else { 0 });
            let active = &config.current[&renderer];
            assert_eq!(active, if layout == 0 { "dormant" } else { "active" });
            assert_eq!(config.properties_for(Path::new(active), renderer).unwrap()["volume"], if layout == 0 { 100 } else { 0 });
        }
    }
    #[test]
    fn saved_properties_keep_their_display_and_normalize_extended_paths() {
        let value = json!({"User":{"general":{}, "wproperties":{
            "C:/Steam/project/scene.pkg": {"Monitor0":{"alignment":1,"volume":0}, "Monitor1":{"alignment":2,"rate":42}}
        }}});
        let config = EngineConfig::parse(&value, "User").unwrap();
        assert_eq!(config.properties_for(Path::new(r"\\?\C:\Steam\project\scene.pkg"), 1), Some(json!({"alignment":2,"rate":42})));
        assert_eq!(config.properties_for(Path::new("c:/steam/project/scene.pkg"), 0), Some(json!({"alignment":1,"volume":0})));
        assert!(config.properties_for(Path::new("c:/steam/project/scene.pkg"), 2).is_none());
        assert!(config.properties_for(Path::new("c:/steam/other/scene.pkg"), 1).is_none());
    }
    #[test]
    #[ignore = "set WALLFLOW_ENGINE_TEST_DIR to an installed Wallpaper Engine directory; read-only"]
    fn installed_configuration_resolves_display_settings() {
        let folder = std::env::var_os("WALLFLOW_ENGINE_TEST_DIR").expect("WALLFLOW_ENGINE_TEST_DIR is required");
        let config = EngineConfig::read(&Path::new(&folder).join("wallpaper64.exe")).unwrap();
        assert!(!config.monitors.is_empty());
        for (display, index) in &config.monitors { assert_eq!(config.monitor_index(display), Some(*index)); }
        assert!(!config.playlists.is_empty());
        for playlist in &config.playlists {
            let combined = config.playlist(&playlist.name).unwrap();
            let mut seen = std::collections::HashSet::new();
            let saved: Vec<_> = playlist.items.iter().filter(|file| seen.insert(normalize_path(file))).cloned().collect();
            assert!(combined.items.starts_with(&saved));
        }
        for (file, displays) in &config.properties {
            for (display, properties) in displays.as_object().unwrap() {
                if let Some(index) = display.strip_prefix("Monitor").and_then(|index| index.parse::<u32>().ok()) {
                    assert_eq!(config.properties_for(Path::new(file), index), Some(properties.clone()));
                }
            }
        }
    }
    #[test]
    fn shares_named_membership_across_monitors_including_unsaved_additions() {
        let value = json!({"User":{"general":{
            "playlists":[{"name":"Arthurian Wallpapers","items":["saved"]}],
            "wallpaperconfig":{"selectedwallpapers":{
                "Monitor0":{"file":"active","playlist":{"name":"Arthurian Wallpapers","items":["SAVED","active"]}},
                "Monitor1":{"file":"unrelated","playlist":{"name":"Other","items":["unrelated"]}},
                "Monitor2":{"file":"another","playlist":{"name":"Arthurian Wallpapers","items":["another","active"]}}
            }}
        }}});
        let config = EngineConfig::parse(&value, "User").unwrap();
        assert_eq!(config.playlist("Arthurian Wallpapers").unwrap().items, vec!["saved", "active", "another"]);
        assert!(config.playlist("Missing").is_none());
    }
}
