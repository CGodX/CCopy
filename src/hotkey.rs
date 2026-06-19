//! 快捷键的解析、格式化与动态注册

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};

/// 快捷键规格：主键 + 修饰键
#[derive(Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl HotkeySpec {
    /// 默认快捷键 Alt+V
    pub fn default_spec() -> Self {
        Self {
            key: "v".into(),
            ctrl: false,
            alt: true,
            shift: false,
        }
    }

    /// 从存储字符串解析，格式如 "alt+shift+v"；失败回退默认值
    pub fn parse(stored: &str) -> Self {
        let parts: Vec<String> = stored
            .split('+')
            .map(|s| s.trim().to_lowercase())
            .collect();
        if parts.is_empty() {
            return Self::default_spec();
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key = String::new();
        for part in &parts {
            match part.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => key = part.clone(),
            }
        }
        if key.is_empty() {
            return Self::default_spec();
        }
        Self { key, ctrl, alt, shift }
    }

    /// 序列化为存储字符串
    pub fn to_storage_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("ctrl".into());
        }
        if self.alt {
            parts.push("alt".into());
        }
        if self.shift {
            parts.push("shift".into());
        }
        parts.push(self.key.clone());
        parts.join("+")
    }

    /// 人类可读显示文本，如 "Alt + V"
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Shift".into());
        }
        parts.push(display_key(&self.key));
        parts.join(" + ")
    }

    /// 转为 global_hotkey 的 HotKey
    pub fn to_hotkey(&self) -> Option<HotKey> {
        let code = key_to_code(&self.key)?;
        let mut mods = Modifiers::empty();
        if self.ctrl {
            mods |= Modifiers::CONTROL;
        }
        if self.alt {
            mods |= Modifiers::ALT;
        }
        if self.shift {
            mods |= Modifiers::SHIFT;
        }
        Some(HotKey::new(Some(mods), code))
    }
}

/// 主键显示名
fn display_key(key: &str) -> String {
    let k = key.to_lowercase();
    match k.as_str() {
        "space" => "Space".into(),
        "return" | "enter" => "Enter".into(),
        "escape" => "Esc".into(),
        "tab" => "Tab".into(),
        "backspace" => "Backspace".into(),
        _ => k.to_uppercase(),
    }
}

/// Slint Key 文本 -> global_hotkey Code
fn key_to_code(key: &str) -> Option<Code> {
    let k = key.to_lowercase();
    Some(match k.as_str() {
        "a" => Code::KeyA,
        "b" => Code::KeyB,
        "c" => Code::KeyC,
        "d" => Code::KeyD,
        "e" => Code::KeyE,
        "f" => Code::KeyF,
        "g" => Code::KeyG,
        "h" => Code::KeyH,
        "i" => Code::KeyI,
        "j" => Code::KeyJ,
        "k" => Code::KeyK,
        "l" => Code::KeyL,
        "m" => Code::KeyM,
        "n" => Code::KeyN,
        "o" => Code::KeyO,
        "p" => Code::KeyP,
        "q" => Code::KeyQ,
        "r" => Code::KeyR,
        "s" => Code::KeyS,
        "t" => Code::KeyT,
        "u" => Code::KeyU,
        "v" => Code::KeyV,
        "w" => Code::KeyW,
        "x" => Code::KeyX,
        "y" => Code::KeyY,
        "z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "f1" => Code::F1,
        "f2" => Code::F2,
        "f3" => Code::F3,
        "f4" => Code::F4,
        "f5" => Code::F5,
        "f6" => Code::F6,
        "f7" => Code::F7,
        "f8" => Code::F8,
        "f9" => Code::F9,
        "f10" => Code::F10,
        "f11" => Code::F11,
        "f12" => Code::F12,
        "space" => Code::Space,
        "return" | "enter" => Code::Enter,
        "escape" => Code::Escape,
        "tab" => Code::Tab,
        "backspace" => Code::Backspace,
        "insert" => Code::Insert,
        "delete" => Code::Delete,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" => Code::PageUp,
        "pagedown" => Code::PageDown,
        "left" => Code::ArrowLeft,
        "right" => Code::ArrowRight,
        "up" => Code::ArrowUp,
        "down" => Code::ArrowDown,
        _ => return None,
    })
}

/// 管理已注册的全局热键，支持动态替换
pub struct HotkeyRegistrar {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl HotkeyRegistrar {
    pub fn new() -> Result<Self, global_hotkey::Error> {
        let manager = GlobalHotKeyManager::new()?;
        Ok(Self {
            manager,
            current: None,
        })
    }

    /// 注册（或替换）当前热键，返回热键 id
    pub fn register(&mut self, spec: &HotkeySpec) -> Option<u32> {
        // 先注销旧热键
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
        let hotkey = spec.to_hotkey()?;
        match self.manager.register(hotkey) {
            Ok(()) => {
                let id = hotkey.id();
                self.current = Some(hotkey);
                Some(id)
            }
            Err(e) => {
                eprintln!("注册热键 {} 失败: {e}", spec.display());
                None
            }
        }
    }
}
