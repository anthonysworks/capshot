//! Print Screen through a low-level keyboard hook.
//!
//! Windows keeps Print Screen for its own screen capture: turning the option
//! off in the registry is only noticed after signing out, and until then
//! `RegisterHotKey` either fails or fires next to the Snipping Tool, which
//! covers the screen. A low-level hook sees the key before Windows does and
//! swallows it, so Capshot gets Print Screen whatever the Windows option says.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_global_shortcut::Modifiers;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use crate::actions::{self, Action};

/// What Print Screen does with each combination of modifiers.
static BINDINGS: Mutex<Vec<(Modifiers, Action)>> = Mutex::new(Vec::new());
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Whether the hook could be installed (decided once).
static ACTIVE: OnceLock<bool> = OnceLock::new();
/// The key went down and was swallowed; its release is swallowed too.
static DOWN: AtomicBool = AtomicBool::new(false);
/// When the last Print Screen event came, in milliseconds since `EPOCH`.
static LAST_EVENT: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// A key held this long without a repeat is not held: its release was lost.
const STALE_DOWN: Duration = Duration::from_millis(1500);

fn now_ms() -> u64 {
    u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Sets what Print Screen triggers. Returns `false` when the hook cannot run,
/// in which case the caller registers the shortcuts the usual way.
pub fn set(app: &AppHandle, bindings: Vec<(Modifiers, Action)>) -> bool {
    let active = *ACTIVE.get_or_init(|| {
        let _ = APP.set(app.clone());
        start()
    });
    if let Ok(mut current) = BINDINGS.lock() {
        *current = bindings;
    }
    active
}

fn start() -> bool {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("print-screen-hook".into())
        .spawn(move || {
            // SAFETY: `hook` is a valid callback, a low-level hook needs no module.
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) };
            if let Err(err) = &hook {
                tracing::warn!("cannot hook the keyboard for Print Screen: {err}");
            }
            let _ = sender.send(hook.is_ok());
            if hook.is_err() {
                return;
            }
            // The hook is called from this thread's message loop.
            let mut message = MSG::default();
            // SAFETY: `message` outlives the calls.
            unsafe {
                while GetMessageW(&mut message, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        });
    spawned.is_ok() && receiver.recv().unwrap_or(false)
}

/// The modifiers held right now.
fn modifiers() -> Modifiers {
    let held = |key: VIRTUAL_KEY| {
        // SAFETY: plain call. The high bit tells the key is down.
        let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
        state < 0
    };
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, held(VK_SHIFT));
    mods.set(Modifiers::CONTROL, held(VK_CONTROL));
    mods.set(Modifiers::ALT, held(VK_MENU));
    mods.set(Modifiers::SUPER, held(VK_LWIN) || held(VK_RWIN));
    mods
}

fn action_now() -> Option<Action> {
    let mods = modifiers();
    BINDINGS
        .lock()
        .ok()?
        .iter()
        .find(|(wanted, _)| *wanted == mods)
        .map(|(_, action)| action.clone())
}

fn trigger(action: Action) {
    tracing::info!(?action, "Print Screen pressed");
    // The hook must answer within a fraction of a second, or Windows drops it.
    if let Some(app) = APP.get().cloned() {
        std::thread::spawn(move || actions::trigger(&app, action));
    }
}

/// The key went down. Returns whether Capshot takes it (and Windows must
/// not see it).
fn on_down() -> bool {
    let Some(action) = action_now() else {
        tracing::info!(mods = ?modifiers(), "Print Screen passed on: no shortcut for these modifiers");
        return false;
    };
    // Holding the key repeats it: act once. A "down" that nothing has
    // repeated for a while is a lost release, not a hold.
    let since = now_ms().saturating_sub(LAST_EVENT.load(Ordering::SeqCst));
    let stale = since > u64::try_from(STALE_DOWN.as_millis()).unwrap_or(u64::MAX);
    let was_down = DOWN.swap(true, Ordering::SeqCst);
    if !was_down || stale {
        if was_down {
            tracing::warn!(
                since,
                "Print Screen was still counted as held: pressed again"
            );
        }
        trigger(action);
    } else {
        tracing::info!("Print Screen repeat ignored");
    }
    true
}

/// The key came up.
fn on_up() -> bool {
    if DOWN.swap(false, Ordering::SeqCst) {
        tracing::info!("Print Screen released");
        return true;
    }
    // Some keyboards only report the release of this key.
    let Some(action) = action_now() else {
        return false;
    };
    trigger(action);
    true
}

fn handle(message: u32) -> bool {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => on_down(),
        WM_KEYUP | WM_SYSKEYUP => on_up(),
        _ => false,
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION.cast_signed() {
        // SAFETY: for `HC_ACTION`, `lparam` points at a `KBDLLHOOKSTRUCT`.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = event.flags.0 & LLKHF_INJECTED.0 != 0;
        // `VK_SNAPSHOT` is 0x2C; our own and other programs' synthetic
        // key presses are left alone.
        if event.vkCode == u32::from(VK_SNAPSHOT.0) && !injected {
            let message = u32::try_from(wparam.0).unwrap_or_default();
            let taken = handle(message);
            LAST_EVENT.store(now_ms(), Ordering::SeqCst);
            if taken {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: passes the event on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
