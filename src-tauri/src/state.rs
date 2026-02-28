use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub sounds: Vec<SoundItem>,
    pub bg_image_path: Option<String>,
    pub default_input_id: Option<String>,
    pub default_output_id: Option<String>,
    pub global_stop_shortcut: Option<String>,
    pub language: Option<String>,
}

pub struct SoundManager {
    pub config: Arc<Mutex<AppConfig>>,
    pub config_path: Arc<Mutex<Option<PathBuf>>>,
}

impl SoundManager {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(AppConfig::default())),
            config_path: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_path_and_load(&self, path: PathBuf) {
        *self.config_path.lock().unwrap() = Some(path.clone());
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<AppConfig>(&data) {
                *self.config.lock().unwrap() = parsed;
            }
        }
    }

    fn save(&self) {
        if let Some(path) = self.config_path.lock().unwrap().as_ref() {
            if let Ok(config) = self.config.lock() {
                if let Ok(data) = serde_json::to_string_pretty(&*config) {
                    let _ = fs::write(path, data);
                }
            }
        }
    }

    pub fn add_sound(&self, item: SoundItem) {
        if let Ok(mut config) = self.config.lock() {
            config.sounds.push(item);
        }
        self.save();
    }

    pub fn remove_sound(&self, id: &str) {
        if let Ok(mut config) = self.config.lock() {
            config.sounds.retain(|s| s.id != id);
        }
        self.save();
    }

    pub fn get_sounds(&self) -> Vec<SoundItem> {
        if let Ok(config) = self.config.lock() {
            config.sounds.clone()
        } else {
            Vec::new()
        }
    }

    pub fn update_shortcut(&self, id: &str, shortcut: Option<String>) {
        if let Ok(mut config) = self.config.lock() {
            if let Some(item) = config.sounds.iter_mut().find(|s| s.id == id) {
                item.shortcut = shortcut;
            }
        }
        self.save();
    }

    pub fn get_sound_by_shortcut(&self, shortcut: &str) -> Option<SoundItem> {
        if let Ok(config) = self.config.lock() {
            config
                .sounds
                .iter()
                .find(|s| s.shortcut.as_deref() == Some(shortcut))
                .cloned()
        } else {
            None
        }
    }

    pub fn set_bg_image(&self, path: Option<String>) {
        if let Ok(mut config) = self.config.lock() {
            config.bg_image_path = path;
        }
        self.save();
    }

    pub fn get_app_config(&self) -> AppConfig {
        if let Ok(config) = self.config.lock() {
            config.clone()
        } else {
            AppConfig::default()
        }
    }

    pub fn set_default_devices(&self, input_id: Option<String>, output_id: Option<String>) {
        if let Ok(mut config) = self.config.lock() {
            config.default_input_id = input_id;
            config.default_output_id = output_id;
        }
        self.save();
    }

    pub fn update_global_stop_shortcut(&self, shortcut: Option<String>) {
        if let Ok(mut config) = self.config.lock() {
            config.global_stop_shortcut = shortcut;
        }
        self.save();
    }

    pub fn set_language(&self, lang: Option<String>) {
        if let Ok(mut config) = self.config.lock() {
            config.language = lang;
        }
        self.save();
    }
}
