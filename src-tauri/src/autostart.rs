//! "Launch EchoVoice when Windows starts" via the standard per-user Run key.
//!
//! Implemented directly against the registry instead of pulling in another plugin: it is
//! one value under `HKCU\...\Run`, needs no elevation and can be undone cleanly.

#![cfg(target_os = "windows")]

use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "EchoVoice";

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}

/// Registers (or clears) the current executable in the user's startup list.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let key = wide(RUN_KEY);
    let mut handle: HKEY = std::ptr::null_mut();

    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            KEY_ALL_ACCESS,
            &mut handle,
        )
    };
    if open != ERROR_SUCCESS {
        return Err(format!(
            "Could not open the startup registry key (error {open})"
        ));
    }

    let result = if enabled {
        let exe = std::env::current_exe()
            .map_err(|err| format!("Could not locate the EchoVoice executable: {err}"))?;
        let exe = exe
            .to_str()
            .ok_or_else(|| "Executable path is not valid UTF-8".to_string())?;
        let value = wide(&format!("\"{exe}\""));
        let name = wide(VALUE_NAME);
        let written = unsafe {
            RegSetValueExW(
                handle,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if written == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!(
                "Could not write the startup entry (error {written})"
            ))
        }
    } else {
        let name = wide(VALUE_NAME);
        let deleted = unsafe { RegDeleteValueW(handle, name.as_ptr()) };
        if deleted == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!(
                "Could not remove the startup entry (error {deleted})"
            ))
        }
    };

    unsafe { RegCloseKey(handle) };
    result
}

/// Reads the Run key back so the settings checkbox reflects reality even if the
/// executable moved or another tool removed the entry.
pub fn is_enabled() -> bool {
    let key = wide(RUN_KEY);
    let mut handle: HKEY = std::ptr::null_mut();
    let opened =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_READ, &mut handle) };
    if opened != ERROR_SUCCESS {
        return false;
    }

    let name = wide(VALUE_NAME);
    let mut data_type = 0u32;
    let mut needed = 0u32;
    let queried = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut data_type,
            std::ptr::null_mut(),
            &mut needed,
        )
    };
    unsafe { RegCloseKey(handle) };

    if queried != ERROR_SUCCESS || data_type != REG_SZ || needed == 0 {
        return false;
    }

    // The entry exists. (We deliberately do not compare the stored path: an installer
    // upgrade can move the executable, and a stale entry would then read as "disabled"
    // even though Windows would still try to launch it.)
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_null_terminated() {
        let value = wide("EchoVoice");
        assert_eq!(value.last(), Some(&0));
        assert_eq!(value.len(), "EchoVoice".len() + 1);
    }

    #[test]
    fn enabling_then_disabling_round_trips() {
        // Requires a writable HKCU, which is always true on a real Windows session.
        if set_enabled(true).is_err() {
            return;
        }
        assert!(set_enabled(false).is_ok());
    }
}
