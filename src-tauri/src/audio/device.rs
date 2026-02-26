use serde::{Deserialize, Serialize};
use wasapi::{DeviceCollection, DeviceEnumerator, Direction};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn get_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    wasapi::initialize_mta().ok();

    let enumerator =
        DeviceEnumerator::new().map_err(|e| format!("Failed to get enumerator: {}", e))?;

    let default_device = enumerator.get_default_device(&Direction::Capture).ok();
    let default_id = default_device.and_then(|d| d.get_id().ok());

    let collection = enumerator
        .get_device_collection(&Direction::Capture)
        .map_err(|e| format!("Failed to get capture devices: {}", e))?;

    let mut devices = Vec::new();
    for i in 0..collection.get_nbr_devices().unwrap_or(0) {
        if let Ok(device) = collection.get_device_at_index(i) {
            if let (Ok(id), Ok(name)) = (device.get_id(), device.get_friendlyname()) {
                let is_default = default_id.as_ref() == Some(&id);
                devices.push(AudioDeviceInfo {
                    id,
                    name,
                    is_default,
                });
            }
        }
    }

    Ok(devices)
}

pub fn get_output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    wasapi::initialize_mta().ok();

    let enumerator =
        DeviceEnumerator::new().map_err(|e| format!("Failed to get enumerator: {}", e))?;

    let default_device = enumerator.get_default_device(&Direction::Render).ok();
    let default_id = default_device.and_then(|d| d.get_id().ok());

    let collection = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|e| format!("Failed to get render devices: {}", e))?;

    let mut devices = Vec::new();
    for i in 0..collection.get_nbr_devices().unwrap_or(0) {
        if let Ok(device) = collection.get_device_at_index(i) {
            if let (Ok(id), Ok(name)) = (device.get_id(), device.get_friendlyname()) {
                let is_default = default_id.as_ref() == Some(&id);
                devices.push(AudioDeviceInfo {
                    id,
                    name,
                    is_default,
                });
            }
        }
    }

    Ok(devices)
}
