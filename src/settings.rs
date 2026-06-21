//! 设置窗口：负责加载/保存设置并与各模块联动

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString};

use crate::hotkey::{HotkeyRegistrar, HotkeySpec};
use crate::storage::Storage;
use crate::ui::SettingsWindow;
use crate::updater::{CheckResult, UpdateInfo};
use crate::{SETTING_HOTKEY, SETTING_MAX_AGE_DAYS, SETTING_MAX_HISTORY};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 打开设置窗口。已打开则聚焦。
/// - `current_hotkey_id`: 共享的当前热键 id，注册成功后更新
/// - `refresh_history`: 设置变更后回调，让主面板刷新历史
/// - `update_info`: 共享的更新检查结果，设置页据此初始化更新状态
pub fn open(
    storage: Rc<RefCell<Storage>>,
    registrar: Rc<RefCell<HotkeyRegistrar>>,
    current_hotkey_id: Rc<Cell<Option<u32>>>,
    window_ref: Rc<RefCell<Option<SettingsWindow>>>,
    refresh_history: impl Fn() + 'static,
    update_info: Arc<Mutex<Option<UpdateInfo>>>,
) {
    // 已打开则聚焦，不重复创建
    if window_ref.borrow().is_some() {
        if let Some(w) = window_ref.borrow().as_ref() {
            let _ = w.window().show();
        }
        return;
    }

    let window = match SettingsWindow::new() {
        Ok(w) => w,
        Err(_) => {
            return;
        }
    };

    // 初始值
    let spec = load_hotkey_spec(&storage);
    window.set_hotkey_display(SharedString::from(spec.display()));
    window.set_hotkey_recording(false);
    window.set_autostart_enabled(crate::autostart::is_enabled());
    window.set_max_history_input(SharedString::from(load_max_history_display(&storage)));
    window.set_max_age_input(SharedString::from(load_max_age_display(&storage)));
    window.set_confirm_kind(SharedString::from(""));
    window.set_confirm_countdown(0);
    window.set_about_version(SharedString::from(APP_VERSION));

    // 更新状态初始化：读取启动时后台检查的结果
    window.set_update_busy(false);
    if let Some(info) = update_info.lock().unwrap().as_ref() {
        window.set_update_status(SharedString::from(format!("新版本 v{}", info.version)));
        window.set_update_available(true);
        window.set_update_url(SharedString::from(info.url.clone()));
    } else {
        window.set_update_status(SharedString::from(""));
        window.set_update_available(false);
        window.set_update_url(SharedString::from(""));
    }

    // 拖拽
    let drag_state = Rc::new(RefCell::new(None::<crate::drag::DragState>));
    let drag_state_for_start = drag_state.clone();
    let window_weak = window.as_weak();
    window.on_drag_started(move || {
        if let Some(w) = window_weak.upgrade() {
            if let Some((cx, cy)) = crate::drag::cursor_position() {
                let pos = w.window().position();
                *drag_state_for_start.borrow_mut() = Some(crate::drag::DragState {
                    window_x: pos.x,
                    window_y: pos.y,
                    cursor_x: cx,
                    cursor_y: cy,
                });
            }
        }
    });

    let drag_state_for_move = drag_state.clone();
    let window_weak = window.as_weak();
    window.on_dragged(move || {
        if let Some(state) = *drag_state_for_move.borrow() {
            if let Some((cx, cy)) = crate::drag::cursor_position() {
                let dx = cx - state.cursor_x;
                let dy = cy - state.cursor_y;
                if let Some(w) = window_weak.upgrade() {
                    w.window().set_position(slint::PhysicalPosition::new(
                        state.window_x + dx,
                        state.window_y + dy,
                    ));
                }
            }
        }
    });

    // 关闭窗口：取出强引用 drop，释放窗口
    let window_ref_close = window_ref.clone();
    window.on_close_requested(move || {
        if let Some(w) = window_ref_close.borrow_mut().take() {
            let _ = w.hide();
        }
    });

    // 开始录制
    let window_weak = window.as_weak();
    window.on_start_record(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_hotkey_recording(true);
        }
    });

    // 录制完成
    let storage_hk = storage.clone();
    let registrar_hk = registrar.clone();
    let hotkey_id_hk = current_hotkey_id.clone();
    let window_weak = window.as_weak();
    window.on_hotkey_recorded(move |key_text, ctrl, alt, shift| {
        let key = normalize_key(key_text.as_str());
        if key.is_empty() {
            return;
        }
        let spec = HotkeySpec { key, ctrl, alt, shift };
        if storage_hk
            .borrow()
            .set_setting(SETTING_HOTKEY, &spec.to_storage_string())
            .is_ok()
        {
            if let Some(id) = registrar_hk.borrow_mut().register(&spec) {
                hotkey_id_hk.set(Some(id));
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_hotkey_display(SharedString::from(spec.display()));
                w.set_hotkey_recording(false);
            }
        }
    });

    // 切换自启
    let window_weak = window.as_weak();
    window.on_toggle_autostart(move || {
        if let Some(w) = window_weak.upgrade() {
            let next = !w.get_autostart_enabled();
            let ok = if next {
                crate::autostart::enable()
            } else {
                crate::autostart::disable()
            };
            if ok {
                w.set_autostart_enabled(next);
            }
        }
    });

    // 应用自动清理规则（最大记录数 + 最大保留天数，共用一个应用按钮）
    let storage_mh = storage.clone();
    let refresh_mh: Rc<dyn Fn()> = Rc::new(refresh_history);
    let refresh_for_apply = refresh_mh.clone();
    let window_weak = window.as_weak();
    window.on_apply_cleanup_rules(move || {
        let (history_input, age_input) = if let Some(w) = window_weak.upgrade() {
            (
                w.get_max_history_input().to_string(),
                w.get_max_age_input().to_string(),
            )
        } else {
            return;
        };
        let history_value = if history_input.trim().is_empty() {
            "0".to_string()
        } else {
            history_input.trim().to_string()
        };
        let age_value = if age_input.trim().is_empty() {
            "0".to_string()
        } else {
            age_input.trim().to_string()
        };
        let max_count: usize = history_value.parse().unwrap_or(0);
        let max_age_days: usize = age_value.parse().unwrap_or(0);
        let _ = storage_mh.borrow().set_setting(SETTING_MAX_HISTORY, &history_value);
        let _ = storage_mh.borrow().set_setting(SETTING_MAX_AGE_DAYS, &age_value);
        // 立即按规则清理一次
        let _ = storage_mh.borrow().purge_by_rule(max_count, max_age_days);
        refresh_for_apply();
    });

    // 二次确认弹层：取消
    let window_weak = window.as_weak();
    window.on_confirm_cancel(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_confirm_kind(SharedString::from(""));
            w.set_confirm_countdown(0);
        }
    });

    // 二次确认弹层：确认（根据 confirm_kind 调用对应清理）
    let storage_confirm = storage.clone();
    let refresh_confirm = refresh_mh.clone();
    let window_weak = window.as_weak();
    window.on_confirm_accept(move || {
        let kind = if let Some(w) = window_weak.upgrade() {
            w.get_confirm_kind().to_string()
        } else {
            return;
        };
        match kind.as_str() {
            "all" => {
                let _ = storage_confirm.borrow().clear_all();
            }
            "unnoted" => {
                let _ = storage_confirm.borrow().clear_unnoted();
            }
            _ => {}
        }
        if let Some(w) = window_weak.upgrade() {
            w.set_confirm_kind(SharedString::from(""));
            w.set_confirm_countdown(0);
        }
        refresh_confirm();
    });

    // 手动检查更新：后台执行，结果回传主线程更新 UI 并写入共享状态
    let update_info_check = update_info.clone();
    let window_weak = window.as_weak();
    window.on_check_update(move || {
        let w = match window_weak.upgrade() {
            Some(w) => w,
            None => return,
        };
        w.set_update_busy(true);
        w.set_update_status(SharedString::from("正在检查…"));
        let window_weak = window_weak.clone();
        let update_info = update_info_check.clone();
        std::thread::spawn(move || {
            let result = crate::updater::check();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = window_weak.upgrade() else { return };
                w.set_update_busy(false);
                match result {
                    Ok(CheckResult::Available(info)) => {
                        w.set_update_status(SharedString::from(format!("新版本 v{}", info.version)));
                        w.set_update_url(SharedString::from(info.url.clone()));
                        w.set_update_available(true);
                        *update_info.lock().unwrap() = Some(info);
                    }
                    Ok(CheckResult::UpToDate) => {
                        w.set_update_status(SharedString::from("已是最新版本"));
                        w.set_update_available(false);
                    }
                    Err(e) => {
                        w.set_update_status(SharedString::from(format!("检查失败: {e}")));
                        w.set_update_available(false);
                    }
                }
            });
        });
    });

    // 立即更新：后台下载安装包并静默安装，完成后退出由安装器重启
    let window_weak = window.as_weak();
    window.on_do_update(move |url| {
        let url = url.to_string();
        let window_weak = window_weak.clone();
        if let Some(w) = window_weak.upgrade() {
            w.set_update_busy(true);
            w.set_update_status(SharedString::from("正在下载更新…"));
        }
        std::thread::spawn(move || {
            match crate::updater::download_and_install(&url) {
                Ok(()) => {
                    // 安装器已接管，进程即将退出
                }
                Err(e) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() {
                            w.set_update_busy(false);
                            w.set_update_status(SharedString::from(format!("更新失败: {e}")));
                        }
                    });
                }
            }
        });
    });

    // 关键：把强引用存入 window_ref，保持窗口存活
    *window_ref.borrow_mut() = Some(window.clone_strong());
    let _ = window.show();
}

/// 从存储加载当前快捷键规格
pub fn load_hotkey_spec(storage: &Rc<RefCell<Storage>>) -> HotkeySpec {
    storage
        .borrow()
        .get_setting(SETTING_HOTKEY)
        .ok()
        .flatten()
        .map(|s| HotkeySpec::parse(&s))
        .unwrap_or_else(HotkeySpec::default_spec)
}

/// 从存储加载最大历史数显示文本（0 显示为空）
pub fn load_max_history_display(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_MAX_HISTORY)
        .ok()
        .flatten()
        .and_then(|s| {
            let v: usize = s.parse().ok()?;
            if v == 0 { None } else { Some(s) }
        })
        .unwrap_or_default()
}

/// 从存储加载最大历史数（0=不限制），解析失败返回 0
pub fn load_max_history_value(storage: &Rc<RefCell<Storage>>) -> usize {
    storage
        .borrow()
        .get_setting(SETTING_MAX_HISTORY)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// 从存储加载最大保留天数显示文本（0 显示为空）
pub fn load_max_age_display(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_MAX_AGE_DAYS)
        .ok()
        .flatten()
        .and_then(|s| {
            let v: usize = s.parse().ok()?;
            if v == 0 { None } else { Some(s) }
        })
        .unwrap_or_default()
}

/// 从存储加载最大保留天数（0=不限制），解析失败返回 0
pub fn load_max_age_value(storage: &Rc<RefCell<Storage>>) -> usize {
    storage
        .borrow()
        .get_setting(SETTING_MAX_AGE_DAYS)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// 把 Slint Key 文本归一化为小写主键名
fn normalize_key(text: &str) -> String {
    let k = text.trim().to_lowercase();
    match k.as_str() {
        "return" => "enter".into(),
        _ => k,
    }
}



