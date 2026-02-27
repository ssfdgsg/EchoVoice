use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub shortcut: Option<String>,
}

pub struct SoundManager {
    pub sounds: Arc<Mutex<Vec<SoundItem>>>,
}

impl SoundManager {
    pub fn new() -> Self {
        Self {
            sounds: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_sound(&self, item: SoundItem) {
        if let Ok(mut list) = self.sounds.lock() {
            list.push(item);
        }
    }

    pub fn remove_sound(&self, id: &str) {
        if let Ok(mut list) = self.sounds.lock() {
            list.retain(|s| s.id != id);
        }
    }

    pub fn get_sounds(&self) -> Vec<SoundItem> {
        if let Ok(list) = self.sounds.lock() {
            list.clone()
        } else {
            Vec::new()
        }
    }

    pub fn update_shortcut(&self, id: &str, shortcut: Option<String>) {
        if let Ok(mut list) = self.sounds.lock() {
            if let Some(item) = list.iter_mut().find(|s| s.id == id) {
                item.shortcut = shortcut;
            }
        }
    }

    pub fn get_sound_by_shortcut(&self, shortcut: &str) -> Option<SoundItem> {
        if let Ok(list) = self.sounds.lock() {
            list.iter()
                .find(|s| s.shortcut.as_deref() == Some(shortcut))
                .cloned()
        } else {
            None
        }
    }
}
