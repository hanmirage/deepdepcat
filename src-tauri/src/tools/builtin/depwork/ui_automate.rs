//! ui_automate — desktop UI automation for Depwork mode (Windows).
//!
//! Drives any desktop application through synthetic input: mouse clicks,
//! keyboard typing, hotkeys, window enumeration/activation and delays.
//! Code mode never sees this tool (see `ToolScope::Depwork`).
//!
//! Actions:
//! - `mouse_click` — move cursor to (x, y) and click (button: left|right|middle)
//! - `mouse_move` — move cursor to (x, y)
//! - `scroll` — vertical (dy) or horizontal (dx) wheel steps
//! - `type` — type a string (full Unicode, no hotkeys)
//! - `hotkey` — press a combination like "ctrl+shift+p"
//! - `key` — press a single special key (enter/esc/tab/arrows/F-keys…)
//! - `window_activate` — bring a window whose title contains `title` to front
//! - `window_list` — list visible window titles
//! - `wait` — pause for `ms` milliseconds
//!
//! Examples:
//! - mouse_click x=640 y=400 button=left
//! - hotkey keys="ctrl+s"
//! - window_activate title="记事本"

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Desktop UI automation (Windows only).
pub struct UiAutomateTool;

impl UiAutomateTool {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// Move cursor to (x, y) and click the given button.
    pub fn mouse_click(x: i32, y: i32, button: &str) -> AppResult<String> {
        unsafe {
            SetCursorPos(x, y)?;
        }
        let (down, up) = match button {
            "left" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            other => return Err(format!("Unknown button: {other}. Use left/right/middle").into()),
        };
        send_mouse(down, 0);
        send_mouse(up, 0);
        Ok(format!("Clicked {button} at ({x}, {y})"))
    }

    pub fn mouse_move(x: i32, y: i32) -> AppResult<String> {
        unsafe {
            SetCursorPos(x, y)?;
        }
        Ok(format!("Cursor moved to ({x}, {y})"))
    }

    pub fn scroll(dx: i32, dy: i32) -> AppResult<String> {
        // One wheel "step" is 120 units; each action sends one step. The
        // direction sign is encoded in the high bit of the u32 wheel data —
        // cast the signed delta AFTER multiplying, so a negative direction
        // stays negative (two's-complement) instead of overflowing.
        if dy != 0 {
            send_mouse(MOUSEEVENTF_WHEEL, (dy.clamp(-1, 1) * 120) as u32);
        }
        if dx != 0 {
            send_mouse(MOUSEEVENTF_HWHEEL, (dx.clamp(-1, 1) * 120) as u32);
        }
        Ok(format!("Scrolled dx={dx} dy={dy}"))
    }

    pub fn type_text(text: &str) -> AppResult<String> {
        let count = text.encode_utf16().count() as u32;
        for unit in text.encode_utf16() {
            // Unicode input: wScan carries the UTF-16 code unit, wVk = 0.
            let mut ki = KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: 0,
            };
            send_keyboard(&mut ki);
            ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
            send_keyboard(&mut ki);
        }
        Ok(format!("Typed {count} characters"))
    }

    /// Press a hotkey combination like "ctrl+shift+p".
    pub fn hotkey(keys: &str) -> AppResult<String> {
        let vks = parse_hotkey(keys)?;
        if vks.is_empty() {
            return Err("Empty hotkey".into());
        }
        for vk in &vks {
            let mut ki = KEYBDINPUT {
                wVk: *vk,
                wScan: 0,
                dwFlags: KEYBD_EVENT_FLAGS(0),
                time: 0,
                dwExtraInfo: 0,
            };
            send_keyboard(&mut ki);
        }
        for vk in vks.iter().rev() {
            let mut ki = KEYBDINPUT {
                wVk: *vk,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };
            send_keyboard(&mut ki);
        }
        Ok(format!("Pressed hotkey {keys}"))
    }

    /// Press a single key by name.
    pub fn press_key(name: &str) -> AppResult<String> {
        let vk = key_vk(name)?;
        let mut ki = KEYBDINPUT {
            wVk: vk,
            wScan: 0,
            dwFlags: KEYBD_EVENT_FLAGS(0),
            time: 0,
            dwExtraInfo: 0,
        };
        send_keyboard(&mut ki);
        ki.dwFlags = KEYEVENTF_KEYUP;
        send_keyboard(&mut ki);
        Ok(format!("Pressed key {name}"))
    }

    pub fn list_windows() -> AppResult<String> {
        let mut titles: Vec<String> = Vec::new();
        let raw = &mut titles as *mut Vec<String> as isize;
        unsafe {
            EnumWindows(Some(enum_proc), LPARAM(raw))?;
        }
        if titles.is_empty() {
            return Ok("No visible windows found".to_string());
        }
        Ok(format!(
            "Visible windows ({}):\n{}",
            titles.len(),
            titles.join("\n")
        ))
    }

    /// Bring the first visible window whose title contains `needle` to front.
    pub fn activate_window(needle: &str) -> AppResult<String> {
        let mut titles: Vec<(HWND, String)> = Vec::new();
        let raw = &mut titles as *mut Vec<(HWND, String)> as isize;
        unsafe {
            EnumWindows(Some(enum_proc_hwnd), LPARAM(raw))?;
        }
        let needle_lower = needle.to_lowercase();
        let hit = titles
            .iter()
            .find(|(_, t)| t.to_lowercase().contains(&needle_lower));
        match hit {
            Some((hwnd, title)) => {
                let ok = unsafe { SetForegroundWindow(*hwnd) }.as_bool();
                if !ok {
                    return Err(format!(
                        "Failed to bring \"{title}\" to the foreground (foreground \
                         lock? retry or focus it manually)"
                    )
                    .into());
                }
                Ok(format!("Activated window \"{title}\""))
            }
            None => Err(format!(
                "No visible window whose title contains \"{needle}\" ({} windows checked)",
                titles.len()
            )
            .into()),
        }
    }

    fn send_mouse(flags: MOUSE_EVENT_FLAGS, mouse_data: u32) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: mouse_data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn send_keyboard(ki: &mut KEYBDINPUT) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: *ki },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let titles = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            if let Some(title) = window_title(hwnd) {
                if !title.is_empty() {
                    titles.push(title);
                }
            }
        }
        BOOL(1)
    }

    unsafe extern "system" fn enum_proc_hwnd(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let titles = unsafe { &mut *(lparam.0 as *mut Vec<(HWND, String)>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            if let Some(title) = window_title(hwnd) {
                if !title.is_empty() {
                    titles.push((hwnd, title));
                }
            }
        }
        BOOL(1)
    }

    fn window_title(hwnd: HWND) -> Option<String> {
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let written = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if written <= 0 {
            return None;
        }
        String::from_utf16_lossy(&buf[..written as usize]).into()
    }

    /// Parse "ctrl+shift+p" / "alt+f4" into a list of virtual keys.
    pub fn parse_hotkey(keys: &str) -> AppResult<Vec<VIRTUAL_KEY>> {
        let mut out = Vec::new();
        for part in keys.split('+') {
            let part = part.trim().to_ascii_lowercase();
            if part.is_empty() {
                continue;
            }
            let vk = match part.as_str() {
                "ctrl" | "control" => VK_CONTROL,
                "shift" => VK_SHIFT,
                "alt" => VK_MENU,
                "win" | "meta" => VK_LWIN,
                "enter" | "return" => VK_RETURN,
                "esc" | "escape" => VK_ESCAPE,
                "tab" => VK_TAB,
                "backspace" | "bs" => VK_BACK,
                "delete" | "del" => VK_DELETE,
                "space" => VK_SPACE,
                "up" => VK_UP,
                "down" => VK_DOWN,
                "left" => VK_LEFT,
                "right" => VK_RIGHT,
                "home" => VK_HOME,
                "end" => VK_END,
                "pageup" | "pgup" => VK_PRIOR,
                "pagedown" | "pgdn" => VK_NEXT,
                other if is_f_key(other) => {
                    let n: u16 = other[1..].parse().unwrap_or(1);
                    VIRTUAL_KEY(VK_F1.0 + n - 1)
                }
                other if is_single_key_char(other) => {
                    let c = other.as_bytes()[0];
                    // Hotkey parts arrive lowercased: a-z must be upper-cased
                    // to match virtual-key codes (VK_A..VK_Z == 'A'..'Z').
                    let code = if c.is_ascii_lowercase() {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    };
                    VIRTUAL_KEY(code as u16)
                }
                other => {
                    return Err(format!(
                        "Unsupported key in hotkey: \"{other}\". Use ctrl/shift/alt/win, \
                         f1-f12, a-z, 0-9, enter/esc/tab/space/arrows/home/end/pageup/pagedown"
                    )
                    .into());
                }
            };
            if !out.contains(&vk) {
                out.push(vk);
            }
        }
        if out.is_empty() {
            return Err("Empty hotkey".into());
        }
        Ok(out)
    }

    pub fn key_vk(name: &str) -> AppResult<VIRTUAL_KEY> {
        match name.to_ascii_lowercase().as_str() {
            "enter" | "return" => Ok(VK_RETURN),
            "esc" | "escape" => Ok(VK_ESCAPE),
            "tab" => Ok(VK_TAB),
            "backspace" | "bs" => Ok(VK_BACK),
            "delete" | "del" => Ok(VK_DELETE),
            "space" => Ok(VK_SPACE),
            "up" => Ok(VK_UP),
            "down" => Ok(VK_DOWN),
            "left" => Ok(VK_LEFT),
            "right" => Ok(VK_RIGHT),
            "home" => Ok(VK_HOME),
            "end" => Ok(VK_END),
            "pageup" | "pgup" => Ok(VK_PRIOR),
            "pagedown" | "pgdn" => Ok(VK_NEXT),
            other if is_f_key(other) => {
                let n: u16 = other[1..]
                    .parse()
                    .map_err(|_| format!("Unknown key: {name}"))?;
                Ok(VIRTUAL_KEY(VK_F1.0 + n - 1))
            }
            other if is_single_key_char(other) => {
                let c = other.as_bytes()[0];
                let code = if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                };
                Ok(VIRTUAL_KEY(code as u16))
            }
            _ => Err(format!("Unknown key: {name}").into()),
        }
    }

    /// "f1".."f12" (lowercased input).
    fn is_f_key(part: &str) -> bool {
        part.starts_with('f')
            && part.len() >= 2
            && part.len() <= 3
            && part[1..]
                .parse::<u8>()
                .map(|n| (1..=12).contains(&n))
                .unwrap_or(false)
    }

    /// Single ASCII letter/digit (lowercased input).
    fn is_single_key_char(part: &str) -> bool {
        part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric()
    }
}

#[cfg(not(windows))]
mod win {
    use super::*;
    pub fn mouse_click(_x: i32, _y: i32, _b: &str) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn mouse_move(_x: i32, _y: i32) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn scroll(_dx: i32, _dy: i32) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn type_text(_t: &str) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn hotkey(_k: &str) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn press_key(_k: &str) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn list_windows() -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
    pub fn activate_window(_n: &str) -> AppResult<String> {
        Err("ui_automate requires Windows".into())
    }
}

#[async_trait]
impl Tool for UiAutomateTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "ui_automate"
    }

    fn description(&self) -> &str {
        "Desktop UI automation (Windows): control any application via synthetic \
         input. Actions: mouse_click(x,y,button=left|right|middle), \
         mouse_move(x,y), scroll(dx,dy wheel steps), type(text, full Unicode), \
         hotkey(keys like \"ctrl+shift+p\" or \"alt+f4\"), key(enter|esc|tab|f1-f12|arrows|...), \
         window_activate(title substring), window_list, wait(ms). Coordinates are \
         in screen pixels."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["mouse_click", "mouse_move", "scroll", "type", "hotkey", "key", "window_activate", "window_list", "wait"],
                    "description": "Operation to perform."
                },
                "x": { "type": "number", "description": "Screen X coordinate (pixels)." },
                "y": { "type": "number", "description": "Screen Y coordinate (pixels)." },
                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button (mouse_click)." },
                "dx": { "type": "number", "description": "Horizontal wheel steps (scroll)." },
                "dy": { "type": "number", "description": "Vertical wheel steps (scroll)." },
                "text": { "type": "string", "description": "Text to type (type)." },
                "keys": { "type": "string", "description": "Hotkey combination, +-separated (hotkey)." },
                "key": { "type": "string", "description": "Single key name (key)." },
                "title": { "type": "string", "description": "Window title substring (window_activate)." },
                "ms": { "type": "number", "description": "Milliseconds to wait (wait)." }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?
            .to_string();
        let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let dx = args.get("dx").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let dy = args.get("dy").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let button = args
            .get("button")
            .and_then(|b| b.as_str())
            .unwrap_or("left")
            .to_string();
        let text = args
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let keys = args
            .get("keys")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        let key = args
            .get("key")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        let title = args
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let ms = args.get("ms").and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;

        let out = tokio::task::spawn_blocking(move || {
            dispatch(
                &action, x, y, dx, dy, &button, &text, &keys, &key, &title, ms,
            )
        })
        .await
        .map_err(|e| format!("UI task panicked: {e}"))??;
        Ok(ToolResult::success(out))
    }
}

/// Pure action dispatch — separated from `execute` so it is testable without
/// constructing a `ToolContext` (which requires an `AppHandle`).
#[allow(clippy::too_many_arguments)]
fn dispatch(
    action: &str,
    x: i32,
    y: i32,
    dx: i32,
    dy: i32,
    button: &str,
    text: &str,
    keys: &str,
    key: &str,
    title: &str,
    ms: u64,
) -> AppResult<String> {
    match action {
        "mouse_click" => win::mouse_click(x, y, button),
        "mouse_move" => win::mouse_move(x, y),
        "scroll" => win::scroll(dx, dy),
        "type" => win::type_text(text),
        "hotkey" => win::hotkey(keys),
        "key" => win::press_key(key),
        "window_activate" => win::activate_window(title),
        "window_list" => win::list_windows(),
        "wait" => {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Ok(format!("Waited {ms} ms"))
        }
        other => Err(format!(
            "Unknown action: {other}. Use mouse_click/mouse_move/scroll/type/hotkey/key/window_activate/window_list/wait"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn hotkey_parses_combinations() {
        let vks = win::parse_hotkey("ctrl+shift+p").expect("parse");
        assert_eq!(vks.len(), 3);
        let vks = win::parse_hotkey("alt+f4").expect("parse");
        assert_eq!(vks.len(), 2);
        assert_eq!(
            vks[1].0,
            windows::Win32::UI::Input::KeyboardAndMouse::VK_F4.0
        );
    }

    #[cfg(windows)]
    #[test]
    fn hotkey_rejects_unknown_keys() {
        assert!(win::parse_hotkey("ctrl+xyz").is_err());
        assert!(win::parse_hotkey("").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn hotkey_dedupes_and_maps_special_keys() {
        let vks = win::parse_hotkey("ctrl+ctrl+enter").expect("parse");
        assert_eq!(vks.len(), 2);
        let vks = win::parse_hotkey("f12").expect("parse");
        assert_eq!(
            vks[0].0,
            windows::Win32::UI::Input::KeyboardAndMouse::VK_F12.0
        );
    }

    #[cfg(windows)]
    #[test]
    fn key_names_resolve() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN, VK_TAB};
        let vks = [
            win::key_vk("Enter").unwrap(),
            win::key_vk("ESC").unwrap(),
            win::key_vk("tab").unwrap(),
        ];
        assert_eq!(vks[0].0, VK_RETURN.0);
        assert_eq!(vks[1].0, VK_ESCAPE.0);
        assert_eq!(vks[2].0, VK_TAB.0);
        assert!(win::key_vk("notakey").is_err());
    }

    #[test]
    fn unknown_action_fails_cleanly() {
        let err = dispatch("explode", 0, 0, 0, 0, "left", "", "", "", "", 0)
            .expect_err("should fail")
            .to_string();
        assert!(err.contains("Unknown action"));
    }
}
