use serde::{Deserialize, Serialize};
use wasapi::{DeviceEnumerator, Direction};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn get_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    let _ = wasapi::initialize_mta().ok();

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
    let _ = wasapi::initialize_mta().ok();

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

/// Everything that decides whether a voice app can actually hear the mix.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct RoutingDiagnostics {
    pub bridge_running: bool,
    /// The device the mix is written to.
    pub output_device_name: String,
    /// True when that device looks like a VB-Cable input endpoint.
    pub output_is_cable: bool,
    /// A capture endpoint named like "CABLE Output" exists.
    pub cable_output_present: bool,
    pub cable_output_name: Option<String>,
    /// The default *console* capture device.
    pub default_capture_name: Option<String>,
    /// The default *communications* capture device - this is what QQ/Discord pick up.
    pub default_comms_capture_name: Option<String>,
    /// True when CABLE Output is the default communications capture device.
    pub cable_is_comms_default: bool,
    pub mic_device_present: bool,
    pub mic_device_name: Option<String>,
    /// Human readable, actionable problems. Empty means the chain should work.
    pub problems: Vec<String>,
}

/// `true` when a device name looks like a VB-Cable endpoint.
fn looks_like_cable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable")
        || lower.contains("vb-audio")
        || lower.contains("vb audio")
        || lower.contains("virtual cable")
}

fn device_name(device: &wasapi::Device) -> Option<String> {
    device.get_friendlyname().ok()
}

/// Inspects the whole capture -> mix -> cable -> app chain.
pub fn get_routing_diagnostics(
    input_device_id: &str,
    output_device_id: &str,
    bridge_running: bool,
) -> RoutingDiagnostics {
    let _ = wasapi::initialize_mta().ok();
    let mut problems = Vec::new();

    let enumerator = DeviceEnumerator::new().ok();

    // --- what we are writing to -------------------------------------------------
    let output_device = enumerator
        .as_ref()
        .and_then(|enumerator| enumerator.get_device(output_device_id).ok());
    let output_device_name = output_device
        .as_ref()
        .and_then(device_name)
        .unwrap_or_else(|| output_device_id.to_string());
    let output_is_cable = looks_like_cable(&output_device_name);
    if !output_is_cable {
        problems.push(format!(
            "The soundboard is writing to '{output_device_name}', which is not a virtual cable. \
             Nothing will reach your voice app."
        ));
    }

    // --- the capture side the voice app needs ----------------------------------
    let mut cable_output_present = false;
    let mut cable_output_name = None;
    let mut default_capture_name = None;
    let mut default_comms_capture_name = None;

    if let Some(enumerator) = enumerator.as_ref() {
        if let Ok(collection) = enumerator.get_device_collection(&Direction::Capture) {
            for index in 0..collection.get_nbr_devices().unwrap_or(0) {
                if let Ok(device) = collection.get_device_at_index(index) {
                    if let Some(name) = device_name(&device) {
                        if looks_like_cable(&name) && !cable_output_present {
                            cable_output_present = true;
                            cable_output_name = Some(name.clone());
                        }
                    }
                }
            }
        }
        if let Ok(device) = enumerator.get_default_device(&Direction::Capture) {
            default_capture_name = device_name(&device);
        }
        if let Ok(device) = enumerator
            .get_default_device_for_role(&Direction::Capture, &wasapi::Role::Communications)
        {
            default_comms_capture_name = device_name(&device);
        }
    }

    if !cable_output_present {
        problems.push(
            "No VB-Cable capture endpoint was found. Install VB-Cable and reboot.".to_string(),
        );
    }

    let cable_is_comms_default = cable_output_name
        .as_deref()
        .zip(default_comms_capture_name.as_deref())
        .map(|(cable, comms)| looks_like_cable(comms) && comms == cable)
        .unwrap_or(false);

    if cable_output_present && !cable_is_comms_default {
        problems.push(format!(
            "CABLE Output is not the default *communications* microphone. QQ, WeChat and most \
             voice apps ignore the normal default and use this one. Current: '{}'. \
             Fix: Windows Sound settings -> Input -> set 'CABLE Output' as the default device \
             (and Default Communications Device), or pick it inside the app.",
            default_comms_capture_name.as_deref().unwrap_or("<none>")
        ));
    }

    // --- the microphone side ----------------------------------------------------
    let mic_device = enumerator
        .as_ref()
        .and_then(|enumerator| enumerator.get_device(input_device_id).ok());
    let mic_device_name = mic_device.as_ref().and_then(device_name);
    let mic_device_present = mic_device.is_some();
    if !mic_device_present {
        problems.push(format!(
            "The selected microphone '{input_device_id}' is gone. Pick another one."
        ));
    }

    if !bridge_running {
        problems.push("The audio bridge is not running.".to_string());
    }

    RoutingDiagnostics {
        bridge_running,
        output_device_name,
        output_is_cable,
        cable_output_present,
        cable_output_name,
        default_capture_name,
        default_comms_capture_name,
        cable_is_comms_default,
        mic_device_present,
        mic_device_name,
        problems,
    }
}
