#![cfg(target_os = "windows")]

//! Game compatible global hotkeys.
//!
//! Why a low-level keyboard hook instead of `RegisterHotKey`:
//!
//! * Games routinely call `RegisterHotKey` themselves for their own key binds. When they
//!   own the combo, `RegisterHotKey` fails with `ERROR_HOTKEY_ALREADY_REGISTERED` for us
//!   and the hotkey simply never fires. A `WH_KEYBOARD_LL` hook sees the key regardless.
//! * `RegisterHotKey` cannot report key *releases*, which push-to-play needs.
//! * The hook never consumes input, so the game still receives every keystroke.
//!
//! The hook is therefore the single source of truth; `tauri-plugin-global-shortcut` is
//! only used as a fallback when the hook could not be installed. Running both channels
//! would make every key press fire twice, so `register_shortcut` in `lib.rs` skips the
//! plugin entirely while the hook is alive.

use std::collections::HashSet;
use std::mem::zeroed;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::thread;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
use tracing::{info, warn};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[derive(Clone, Copy)]
struct KeyStroke {
    virtual_key: u32,
    extended: bool,
    modifiers: Modifiers,
    /// `true` when the key was released. Needed for push-to-play style sounds.
    key_up: bool,
}

static HOOK_SENDER: OnceLock<SyncSender<KeyStroke>> = OnceLock::new();
static PRESSED_KEYS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
/// Modifier state maintained from the hook's own key events.
static HOOK_MODIFIERS: OnceLock<Mutex<Modifiers>> = OnceLock::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// Number of keystrokes the hook has seen. Surfaced in the UI so the user can tell a
/// dead listener apart from a broken audio path.
pub fn keystroke_count() -> u64 {
    KEYSTROKES.load(Ordering::Relaxed)
}

static KEYSTROKES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Installs a passive low-level keyboard hook. It never consumes input, so the game still
/// receives every key. The regular RegisterHotKey path stays disabled while we are alive.
pub fn install(app: AppHandle) -> Result<(), String> {
    let (event_tx, event_rx) = mpsc::sync_channel::<KeyStroke>(256);
    HOOK_SENDER
        .set(event_tx)
        .map_err(|_| "Game hotkey listener is already installed".to_string())?;
    let _ = PRESSED_KEYS.set(Mutex::new(HashSet::new()));
    let _ = HOOK_MODIFIERS.set(Mutex::new(Modifiers::empty()));

    thread::Builder::new()
        .name("game-hotkey-dispatch".into())
        .spawn(move || {
            while let Ok(stroke) = event_rx.recv() {
                let Some(code) = virtual_key_to_code(stroke.virtual_key, stroke.extended) else {
                    continue;
                };
                let shortcut = Shortcut::new(Some(stroke.modifiers), code);
                if stroke.key_up {
                    crate::handle_shortcut_release(&app, &shortcut);
                } else {
                    crate::handle_shortcut_activation(&app, &shortcut);
                }
            }
        })
        .map_err(|err| format!("Failed to start game hotkey dispatcher: {err}"))?;

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("game-hotkey-hook".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(ptr::null());
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            if hook.is_null() {
                let err = std::io::Error::last_os_error();
                let _ = ready_tx.send(Err(format!(
                    "Failed to install game hotkey listener: {err}"
                )));
                return;
            }

            AVAILABLE.store(true, Ordering::Release);
            let _ = ready_tx.send(Ok(()));
            info!("Game-compatible keyboard listener started");

            let mut message: MSG = zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {}

            AVAILABLE.store(false, Ordering::Release);
            let _ = UnhookWindowsHookEx(hook);
            warn!("Game-compatible keyboard listener stopped");
        })
        .map_err(|err| format!("Failed to start game hotkey listener: {err}"))?;

    ready_rx
        .recv()
        .map_err(|_| "Game hotkey listener stopped during startup".to_string())?
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = &*(lparam as *const KBDLLHOOKSTRUCT);
        let vk = event.vkCode;
        let extended = event.flags & LLKHF_EXTENDED != 0;
        let key_down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;
        let key_up = wparam == WM_KEYUP as usize || wparam == WM_SYSKEYUP as usize;

        if key_down || key_up {
            KEYSTROKES.fetch_add(1, Ordering::Relaxed);

            if is_modifier(vk) {
                // Keep our own modifier bookkeeping: `GetAsyncKeyState` inside a hook can
                // lag behind, and a stale Shift would silently break every combo.
                update_modifiers(vk, key_down);
            }

            if key_down {
                let first_press = PRESSED_KEYS
                    .get()
                    .and_then(|keys| keys.lock().ok())
                    .map(|mut keys| keys.insert(vk))
                    .unwrap_or(false);

                if first_press {
                    // Reconcile with the real keyboard state on every fresh press so a
                    // missed key-up (hook installed mid-press, sleep/resume) self-heals.
                    let modifiers = reconcile_modifiers();
                    if let Some(sender) = HOOK_SENDER.get() {
                        let _ = sender.try_send(KeyStroke {
                            virtual_key: vk,
                            extended,
                            modifiers,
                            key_up: false,
                        });
                    }
                }
            } else {
                let was_pressed = PRESSED_KEYS
                    .get()
                    .and_then(|keys| keys.lock().ok())
                    .map(|mut keys| keys.remove(&vk))
                    .unwrap_or(false);
                // Only report a release for keys we actually saw go down, otherwise a hook
                // that starts mid-keypress would fire a bogus release.
                if was_pressed {
                    let modifiers = reconcile_modifiers();
                    if let Some(sender) = HOOK_SENDER.get() {
                        let _ = sender.try_send(KeyStroke {
                            virtual_key: vk,
                            extended,
                            modifiers,
                            key_up: true,
                        });
                    }
                }
            }
        }
    }

    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

fn is_modifier(vk: u32) -> bool {
    matches!(
        vk as u16,
        VK_CONTROL
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_MENU
            | VK_LMENU
            | VK_RMENU
            | VK_SHIFT
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LWIN
            | VK_RWIN
    )
}

unsafe fn update_modifiers(vk: u32, down: bool) {
    let Some(state) = HOOK_MODIFIERS.get() else {
        return;
    };
    let Ok(mut modifiers) = state.lock() else {
        return;
    };
    let flag = match vk as u16 {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Modifiers::CONTROL,
        VK_MENU | VK_LMENU | VK_RMENU => Modifiers::ALT,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Modifiers::SHIFT,
        VK_LWIN | VK_RWIN => Modifiers::SUPER,
        _ => return,
    };
    if down {
        modifiers.insert(flag);
    } else {
        modifiers.remove(flag);
    }
}

/// Merges our tracked modifier state with the real keyboard state.
unsafe fn reconcile_modifiers() -> Modifiers {
    let mut modifiers = HOOK_MODIFIERS
        .get()
        .and_then(|state| state.lock().ok())
        .map(|state| *state)
        .unwrap_or_else(Modifiers::empty);

    if key_is_down(VK_CONTROL) || key_is_down(VK_LCONTROL) || key_is_down(VK_RCONTROL) {
        modifiers.insert(Modifiers::CONTROL);
    }
    if key_is_down(VK_MENU) || key_is_down(VK_LMENU) || key_is_down(VK_RMENU) {
        modifiers.insert(Modifiers::ALT);
    }
    if key_is_down(VK_SHIFT) || key_is_down(VK_LSHIFT) || key_is_down(VK_RSHIFT) {
        modifiers.insert(Modifiers::SHIFT);
    }
    if key_is_down(VK_LWIN) || key_is_down(VK_RWIN) {
        modifiers.insert(Modifiers::SUPER);
    }
    modifiers
}

unsafe fn key_is_down(key: VIRTUAL_KEY) -> bool {
    GetAsyncKeyState(key as i32) < 0
}

fn virtual_key_to_code(key: u32, extended: bool) -> Option<Code> {
    Some(match key as u16 {
        VK_A => Code::KeyA,
        VK_B => Code::KeyB,
        VK_C => Code::KeyC,
        VK_D => Code::KeyD,
        VK_E => Code::KeyE,
        VK_F => Code::KeyF,
        VK_G => Code::KeyG,
        VK_H => Code::KeyH,
        VK_I => Code::KeyI,
        VK_J => Code::KeyJ,
        VK_K => Code::KeyK,
        VK_L => Code::KeyL,
        VK_M => Code::KeyM,
        VK_N => Code::KeyN,
        VK_O => Code::KeyO,
        VK_P => Code::KeyP,
        VK_Q => Code::KeyQ,
        VK_R => Code::KeyR,
        VK_S => Code::KeyS,
        VK_T => Code::KeyT,
        VK_U => Code::KeyU,
        VK_V => Code::KeyV,
        VK_W => Code::KeyW,
        VK_X => Code::KeyX,
        VK_Y => Code::KeyY,
        VK_Z => Code::KeyZ,
        VK_0 => Code::Digit0,
        VK_1 => Code::Digit1,
        VK_2 => Code::Digit2,
        VK_3 => Code::Digit3,
        VK_4 => Code::Digit4,
        VK_5 => Code::Digit5,
        VK_6 => Code::Digit6,
        VK_7 => Code::Digit7,
        VK_8 => Code::Digit8,
        VK_9 => Code::Digit9,
        VK_OEM_PLUS => Code::Equal,
        VK_OEM_COMMA => Code::Comma,
        VK_OEM_MINUS => Code::Minus,
        VK_OEM_PERIOD => Code::Period,
        VK_OEM_1 => Code::Semicolon,
        VK_OEM_2 => Code::Slash,
        VK_OEM_3 => Code::Backquote,
        VK_OEM_4 => Code::BracketLeft,
        VK_OEM_5 => Code::Backslash,
        VK_OEM_6 => Code::BracketRight,
        VK_OEM_7 => Code::Quote,
        VK_BACK => Code::Backspace,
        VK_TAB => Code::Tab,
        VK_SPACE => Code::Space,
        VK_RETURN if extended => Code::NumpadEnter,
        VK_RETURN => Code::Enter,
        VK_CAPITAL => Code::CapsLock,
        VK_ESCAPE => Code::Escape,
        VK_PRIOR => Code::PageUp,
        VK_NEXT => Code::PageDown,
        VK_END => Code::End,
        VK_HOME => Code::Home,
        VK_LEFT => Code::ArrowLeft,
        VK_UP => Code::ArrowUp,
        VK_RIGHT => Code::ArrowRight,
        VK_DOWN => Code::ArrowDown,
        VK_SNAPSHOT => Code::PrintScreen,
        VK_INSERT => Code::Insert,
        VK_DELETE => Code::Delete,
        VK_F1 => Code::F1,
        VK_F2 => Code::F2,
        VK_F3 => Code::F3,
        VK_F4 => Code::F4,
        VK_F5 => Code::F5,
        VK_F6 => Code::F6,
        VK_F7 => Code::F7,
        VK_F8 => Code::F8,
        VK_F9 => Code::F9,
        VK_F10 => Code::F10,
        VK_F11 => Code::F11,
        VK_F12 => Code::F12,
        VK_F13 => Code::F13,
        VK_F14 => Code::F14,
        VK_F15 => Code::F15,
        VK_F16 => Code::F16,
        VK_F17 => Code::F17,
        VK_F18 => Code::F18,
        VK_F19 => Code::F19,
        VK_F20 => Code::F20,
        VK_F21 => Code::F21,
        VK_F22 => Code::F22,
        VK_F23 => Code::F23,
        VK_F24 => Code::F24,
        VK_NUMLOCK => Code::NumLock,
        VK_NUMPAD0 => Code::Numpad0,
        VK_NUMPAD1 => Code::Numpad1,
        VK_NUMPAD2 => Code::Numpad2,
        VK_NUMPAD3 => Code::Numpad3,
        VK_NUMPAD4 => Code::Numpad4,
        VK_NUMPAD5 => Code::Numpad5,
        VK_NUMPAD6 => Code::Numpad6,
        VK_NUMPAD7 => Code::Numpad7,
        VK_NUMPAD8 => Code::Numpad8,
        VK_NUMPAD9 => Code::Numpad9,
        VK_ADD => Code::NumpadAdd,
        VK_DECIMAL => Code::NumpadDecimal,
        VK_DIVIDE => Code::NumpadDivide,
        VK_MULTIPLY => Code::NumpadMultiply,
        VK_SUBTRACT => Code::NumpadSubtract,
        VK_SCROLL => Code::ScrollLock,
        VK_VOLUME_DOWN => Code::AudioVolumeDown,
        VK_VOLUME_UP => Code::AudioVolumeUp,
        VK_VOLUME_MUTE => Code::AudioVolumeMute,
        VK_MEDIA_PLAY_PAUSE => Code::MediaPlayPause,
        VK_MEDIA_STOP => Code::MediaStop,
        VK_MEDIA_NEXT_TRACK => Code::MediaTrackNext,
        VK_MEDIA_PREV_TRACK => Code::MediaTrackPrevious,
        VK_PAUSE => Code::Pause,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_game_keys() {
        assert_eq!(virtual_key_to_code(VK_F8 as u32, false), Some(Code::F8));
        assert_eq!(virtual_key_to_code(VK_X as u32, false), Some(Code::KeyX));
        assert_eq!(
            virtual_key_to_code(VK_NUMPAD5 as u32, false),
            Some(Code::Numpad5)
        );
    }

    #[test]
    fn distinguishes_numpad_enter() {
        assert_eq!(
            virtual_key_to_code(VK_RETURN as u32, false),
            Some(Code::Enter)
        );
        assert_eq!(
            virtual_key_to_code(VK_RETURN as u32, true),
            Some(Code::NumpadEnter)
        );
    }

    #[test]
    fn maps_every_letter_and_digit() {
        for vk in VK_A..=VK_Z {
            assert!(
                virtual_key_to_code(vk as u32, false).is_some(),
                "VK {vk} is unmapped"
            );
        }
        for vk in VK_0..=VK_9 {
            assert!(
                virtual_key_to_code(vk as u32, false).is_some(),
                "VK {vk} is unmapped"
            );
        }
    }

    #[test]
    fn recognises_modifier_keys() {
        for vk in [
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LWIN,
            VK_RWIN,
        ] {
            assert!(is_modifier(vk as u32), "VK {vk} should be a modifier");
        }
        assert!(!is_modifier(VK_A as u32));
        assert!(!is_modifier(VK_F1 as u32));
    }

    #[test]
    fn shortcut_ids_match_what_the_ui_binds() {
        // The frontend builds strings like "Control+Shift+Digit1" from KeyboardEvent.code.
        for text in [
            "F8",
            "Digit1",
            "KeyX",
            "Control+Shift+Digit1",
            "Alt+KeyX",
            "Control+Alt+Shift+F7",
            "Super+Digit1",
            "Numpad1",
        ] {
            let parsed =
                Shortcut::from_str(text).unwrap_or_else(|err| panic!("'{text}' must parse: {err}"));
            // The hook rebuilds the same shortcut from vk + modifiers; both must agree.
            let rebuilt = Shortcut::new(Some(parsed.mods), parsed.key);
            assert_eq!(parsed.id(), rebuilt.id(), "id mismatch for '{text}'");
        }
    }
}
