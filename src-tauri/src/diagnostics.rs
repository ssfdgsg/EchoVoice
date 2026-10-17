//! Runtime diagnostics and elevation helpers.
//!
//! The single most common reason "hotkeys do nothing in game" is that the game runs as
//! administrator while EchoVoice does not: Windows then refuses to deliver keystrokes
//! destined for the elevated process to our hook, and no amount of user-mode code can
//! work around it. This module makes that state *visible* and offers a one-click fix.

#![cfg(target_os = "windows")]

use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::ShellExecuteExW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}

/// `true` when this process is running with an elevated (administrator) token.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            std::ptr::addr_of_mut!(elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Relaunches EchoVoice elevated. The current process exits afterwards.
pub fn restart_elevated() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Could not locate the EchoVoice executable: {err}"))?;
    let exe = exe
        .to_str()
        .ok_or_else(|| "Executable path is not valid UTF-8".to_string())?;

    let file = wide(exe);
    let parameters = wide("");
    let verb = wide("runas");
    let mut info: windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW>() as u32;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    info.fMask = windows_sys::Win32::UI::Shell::SEE_MASK_NOASYNC;

    let launched = unsafe { ShellExecuteExW(&mut info) };
    if launched == 0 {
        // The user probably dismissed the UAC prompt.
        return Err("Elevation was cancelled".to_string());
    }
    // A second instance would fight over the audio devices, so hand over and exit.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_null_terminated() {
        let value = wide("runas");
        assert_eq!(value.last(), Some(&0));
        assert_eq!(value.len(), "runas".len() + 1);
    }

    #[test]
    fn elevation_check_does_not_panic() {
        // Result depends on how the test runner was launched; just prove it is callable.
        let _ = is_elevated();
    }
}
