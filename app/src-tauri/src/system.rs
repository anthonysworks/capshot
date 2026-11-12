//! Small pieces of Windows integration used by the setup: starting with
//! Windows, and the Print Screen key, which Windows keeps for its own
//! screen capture unless told otherwise.

// Registry sizes are 32-bit and these values are a few bytes long.
#![allow(clippy::cast_possible_truncation)]

use std::path::Path;

use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE: PCWSTR = w!("Capshot");
const KEYBOARD_KEY: PCWSTR = w!(r"Control Panel\Keyboard");
const PRINT_SCREEN: PCWSTR = w!("PrintScreenKeyForSnippingEnabled");

/// Starts Capshot (in the tray) when the user signs in, or stops doing so.
pub fn set_startup(enabled: bool, exe: &Path) -> Result<(), String> {
    // SAFETY: plain registry calls with valid, NUL-terminated strings.
    unsafe {
        if enabled {
            // Quoted: the path may contain spaces.
            let command = HSTRING::from(format!("\"{}\"", exe.display()));
            let bytes =
                std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), (command.len() + 1) * 2);
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ.0,
                Some(bytes.as_ptr().cast()),
                bytes.len() as u32,
            )
            .ok()
            .map_err(|e| e.to_string())
        } else {
            // Not there is as good as removed.
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE);
            Ok(())
        }
    }
}

/// Whether Windows opens its own screen capture on Print Screen. The
/// setting is on unless the registry says otherwise.
pub fn print_screen_taken() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the out-pointers point at `value` and `size`.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEYBOARD_KEY,
            PRINT_SCREEN,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    result.is_err() || value != 0
}

/// Turns Windows' own Print Screen capture off (for this user only; it can
/// be turned back on in Settings › Accessibility › Keyboard). Windows may
/// need the user to sign out and in again to notice.
pub fn release_print_screen() -> Result<(), String> {
    let value = 0u32.to_le_bytes();
    // SAFETY: plain registry call with valid strings and data.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            KEYBOARD_KEY,
            PRINT_SCREEN,
            REG_DWORD.0,
            Some(value.as_ptr().cast()),
            value.len() as u32,
        )
        .ok()
        .map_err(|e| e.to_string())
    }
}
