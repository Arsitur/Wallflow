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
