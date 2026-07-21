//! 设置窗口：负责加载/保存设置并与各模块联动

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString};

use crate::hotkey::{HotkeyRegistrar, HotkeySpec};
use crate::storage::Storage;
use crate::sync::{self, SharedCoordinator};
use crate::ui::SettingsWindow;
use crate::updater::{CheckResult, UpdateInfo};
use crate::{
    SETTING_HOTKEY, SETTING_MAX_AGE_DAYS, SETTING_MAX_HISTORY, SETTING_SYNC_ENABLED,
    SETTING_SYNC_IMAGE_ENABLED, SETTING_SYNC_MARKED_ONLY, SETTING_SYNC_PASSWORD,
    SETTING_SYNC_RETAIN_MONTHS, SETTING_SYNC_URL, SETTING_SYNC_USERNAME,
};

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
    sync_coordinator: Rc<RefCell<Option<SharedCoordinator>>>,
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

    // 默认显示通用 tab
    window.set_active_tab(0);

    // 从存储加载同步配置（未配置时用默认值）
    window.set_sync_enabled(load_sync_enabled(&storage));
    window.set_sync_url(SharedString::from(load_sync_url(&storage)));
    window.set_sync_username(SharedString::from(load_sync_username(&storage)));
    window.set_sync_password(SharedString::from(load_sync_password(&storage)));
    window.set_sync_image_enabled(load_sync_image_enabled(&storage));
    window.set_sync_marked_only(load_sync_marked_only(&storage));
    window.set_sync_retain_months(SharedString::from(load_sync_retain_months_display(&storage)));
    window.set_sync_status(SharedString::from(""));
    window.set_sync_password_visible(false);

    // 加载统计到 UI
    let refresh_stats: Rc<dyn Fn()> = Rc::new({
        let storage = storage.clone();
        let window_weak = window.as_weak();
        move || {
            let Some(w) = window_weak.upgrade() else { return };
            if let Ok((total, marked, text, image, files)) = storage.borrow().stats() {
                w.set_stat_total(total as i32);
                w.set_stat_marked(marked as i32);
                w.set_stat_text(text as i32);
                w.set_stat_image(image as i32);
                w.set_stat_files(files as i32);
            }
        }
    });
    refresh_stats();

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
    let refresh_stats_for_apply = refresh_stats.clone();
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
        refresh_stats_for_apply();
    });

    // 同步配置应用：写入存储后重建协调器
    let storage_sync = storage.clone();
    let sync_coord_for_apply = sync_coordinator.clone();
    let window_weak = window.as_weak();
    window.on_apply_sync_config(move || {
        let Some(w) = window_weak.upgrade() else { return };
        let s = storage_sync.borrow();
        let _ = s.set_setting(
            SETTING_SYNC_ENABLED,
            if w.get_sync_enabled() { "1" } else { "0" },
        );
        let _ = s.set_setting(SETTING_SYNC_URL, w.get_sync_url().as_str());
        let _ = s.set_setting(SETTING_SYNC_USERNAME, w.get_sync_username().as_str());
        let _ = s.set_setting(SETTING_SYNC_PASSWORD, w.get_sync_password().as_str());
        let _ = s.set_setting(
            SETTING_SYNC_IMAGE_ENABLED,
            if w.get_sync_image_enabled() { "1" } else { "0" },
        );
        let _ = s.set_setting(
            SETTING_SYNC_MARKED_ONLY,
            if w.get_sync_marked_only() { "1" } else { "0" },
        );
        // 保留期：空值按默认 3 处理，避免存入空串
        let retain = w.get_sync_retain_months().to_string();
        let retain_val = if retain.trim().is_empty() {
            "3".to_string()
        } else {
            retain.trim().to_string()
        };
        let _ = s.set_setting(SETTING_SYNC_RETAIN_MONTHS, &retain_val);
        drop(s);
        // 重建协调器：配置变更后用新配置重新构建
        let new_coord = sync::build_coordinator(sync::config::SyncConfig::load(&storage_sync));
        *sync_coord_for_apply.borrow_mut() = new_coord;
        // 状态反馈改用 toast
        if let Some(w) = window_weak.upgrade() {
            if w.get_sync_enabled() {
                show_toast(&w, "配置已保存", "success");
            } else {
                show_toast(&w, "同步已关闭", "info");
            }
        }
    });

    // 测试连接：用当前界面输入的配置（先应用）临时构建协调器，调用 ping 验证
    let storage_for_test = storage.clone();
    let window_weak = window.as_weak();
    window.on_test_connection(move || {
        let Some(w) = window_weak.upgrade() else { return };
        // 先把当前界面配置落盘（保证用最新配置测试）
        {
            let s = storage_for_test.borrow();
            let _ = s.set_setting(
                SETTING_SYNC_ENABLED,
                if w.get_sync_enabled() { "1" } else { "0" },
            );
            let _ = s.set_setting(SETTING_SYNC_URL, w.get_sync_url().as_str());
            let _ = s.set_setting(SETTING_SYNC_USERNAME, w.get_sync_username().as_str());
            let _ = s.set_setting(SETTING_SYNC_PASSWORD, w.get_sync_password().as_str());
        }
        // 检查必填项
        let url = w.get_sync_url().to_string();
        let username = w.get_sync_username().to_string();
        if url.trim().is_empty() || username.trim().is_empty() {
            show_toast(&w, "请填写 WebDAV 地址和用户名", "error");
            return;
        }
        show_toast(&w, "测试中…", "info");
        // 临时构建协调器测试（不替换全局协调器）
        let config = sync::config::SyncConfig::load(&storage_for_test);
        let window_weak_for_thread = window_weak.clone();
        std::thread::spawn(move || {
            let result = if config.is_usable() {
                let coord = sync::SyncCoordinator::new(config);
                coord.ping().map(|_| ()).map_err(|e| e)
            } else {
                Err("配置不完整或同步未启用".to_string())
            };
            // 回主线程显示结果
            let msg = match result {
                Ok(_) => ("连接成功".to_string(), "success"),
                Err(e) => (format!("连接失败: {e}"), "error"),
            };
            slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak_for_thread.upgrade() {
                    show_toast(&w, &msg.0, msg.1);
                }
            })
            .ok();
        });
    });

    // 立即同步：触发一次推送+拉取，结果通过 Arc 缓冲由短期 Timer 回主线程应用
    let sync_coord_for_now = sync_coordinator.clone();
    let storage_for_now = storage.clone();
    let window_weak = window.as_weak();
    window.on_sync_now(move || {
        let coord = match sync_coord_for_now.borrow().as_ref() {
            Some(c) => c.clone(),
            None => {
                if let Some(w) = window_weak.upgrade() {
                    show_toast(&w, "同步未启用", "error");
                }
                return;
            }
        };
        // 主线程收集本地待推送记录（不碰网络，不阻塞 UI）
        let items_to_push = {
            let storage = storage_for_now.borrow();
            let c = coord.lock().unwrap();
            c.collect_push_all(&storage)
        };
        if let Some(w) = window_weak.upgrade() {
            show_toast(&w, &format!("同步中…（本地 {} 条待推送）", items_to_push.len()), "info");
        }
        // 推送结果缓冲：后台线程写入，主线程轮询消费
        let push_buf: Arc<Mutex<Option<Result<usize, String>>>> = Arc::new(Mutex::new(None));
        let pull_buf: Arc<Mutex<Option<Result<Vec<sync::event::SyncEvent>, String>>>> =
            Arc::new(Mutex::new(None));
        // 后台全量推送：去重 + flush
        let push_buf_for_thread = push_buf.clone();
        let coord_for_flush = coord.clone();
        std::thread::spawn(move || {
            let result = match coord_for_flush.lock() {
                Ok(c) => c.flush_push_all(items_to_push),
                Err(e) => Err(format!("协调器锁失败: {e}")),
            };
            *push_buf_for_thread.lock().unwrap() = Some(result);
        });
        // 后台拉取，结果写入 Arc 缓冲
        let pull_buf_for_thread = pull_buf.clone();
        sync::spawn_pull(coord.clone(), move |result| {
            *pull_buf_for_thread.lock().unwrap() = Some(result);
        });
        // 短期 Timer：主线程轮询两个缓冲，都完成后合并显示 + 自停
        let storage_for_check = storage_for_now.clone();
        let coord_for_check = coord;
        let window_weak_for_check = window_weak.clone();
        let check_timer = Rc::new(slint::Timer::default());
        let check_timer_for_closure = check_timer.clone();
        check_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                // 两个结果都到了才处理（先检查不取，避免单边 take 导致丢失）
                let (push_ready, pull_ready) = {
                    let pb = push_buf.lock().unwrap();
                    let pl = pull_buf.lock().unwrap();
                    (pb.is_some(), pl.is_some())
                };
                if !(push_ready && pull_ready) {
                    return;
                }
                // 两个都就绪，同时取走
                let push_res = push_buf.lock().unwrap().take().unwrap();
                let pull_res = pull_buf.lock().unwrap().take().unwrap();
                let win = window_weak_for_check.upgrade();
                // 推送结果
                let pushed_str = match push_res {
                    Ok(n) => format!("推送 {}", n),
                    Err(e) => {
                        if let Some(w) = &win {
                            show_toast(w, &format!("推送失败: {e}"), "error");
                        }
                        check_timer_for_closure.stop();
                        return;
                    }
                };
                // 拉取结果
                let pull_str = match pull_res {
                    Err(e) => {
                        if let Some(w) = &win {
                            show_toast(
                                w,
                                &format!("{}，拉取失败: {}", pushed_str, e),
                                "error",
                            );
                        }
                        check_timer_for_closure.stop();
                        return;
                    }
                    Ok(events) => {
                        let count = events.len();
                        let applied = {
                            let storage = storage_for_check.borrow();
                            let c = coord_for_check.lock().unwrap();
                            c.apply_events(&storage, events)
                        };
                        format!(
                            "{}，拉取 {}（应用 {}，删除 {}）",
                            pushed_str, count, applied.applied, applied.deleted
                        )
                    }
                };
                if let Some(w) = &win {
                    show_toast(w, &pull_str, "success");
                }
                check_timer_for_closure.stop();
            },
        );
    });

    // 打开外部链接：调用系统默认浏览器
    window.on_open_url(|url| {
        let url = url.to_string();
        #[cfg(target_os = "windows")]
        {
            use std::ffi::OsStr;
            use std::os::windows::ffi::OsStrExt;
            extern "system" {
                fn ShellExecuteW(
                    hwnd: *mut std::ffi::c_void,
                    op: *const u16,
                    file: *const u16,
                    params: *const u16,
                    dir: *const u16,
                    show: i32,
                ) -> *mut std::ffi::c_void;
            }
            const SW_SHOWNORMAL: i32 = 1;
            let to_wide = |s: &str| -> Vec<u16> {
                OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
            };
            let op = to_wide("open");
            let file = to_wide(&url);
            unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    op.as_ptr(),
                    file.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                );
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = url;
        }
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
    let refresh_stats_confirm = refresh_stats.clone();
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
        refresh_stats_confirm();
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

// ===== 同步配置加载/保存 =====

/// 从存储加载同步开关（未设置默认 false）
pub fn load_sync_enabled(storage: &Rc<RefCell<Storage>>) -> bool {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_ENABLED)
        .ok()
        .flatten()
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn load_sync_url(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_URL)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn load_sync_username(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_USERNAME)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn load_sync_password(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_PASSWORD)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn load_sync_image_enabled(storage: &Rc<RefCell<Storage>>) -> bool {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_IMAGE_ENABLED)
        .ok()
        .flatten()
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 仅同步标记内容（默认 true）：减少垃圾数据流量
pub fn load_sync_marked_only(storage: &Rc<RefCell<Storage>>) -> bool {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_MARKED_ONLY)
        .ok()
        .flatten()
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(true) // 默认 true
}

/// 保留期显示文本（0 显示为空，未设置默认 3）
pub fn load_sync_retain_months_display(storage: &Rc<RefCell<Storage>>) -> String {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_RETAIN_MONTHS)
        .ok()
        .flatten()
        .and_then(|s| {
            let v: usize = s.parse().ok()?;
            if v == 0 { None } else { Some(s) }
        })
        .unwrap_or_else(|| "3".to_string())
}

/// 保留期数值（未设置默认 3）
#[allow(dead_code)]
pub fn load_sync_retain_months_value(storage: &Rc<RefCell<Storage>>) -> usize {
    storage
        .borrow()
        .get_setting(SETTING_SYNC_RETAIN_MONTHS)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
}

/// 把 Slint Key 文本归一化为小写主键名
fn normalize_key(text: &str) -> String {
    let k = text.trim().to_lowercase();
    match k.as_str() {
        "return" => "enter".into(),
        _ => k,
    }
}

/// 显示 toast 通知：设置消息和类型并显示，2 秒后自动隐藏
/// kind: "info" | "success" | "error"
fn show_toast(window: &SettingsWindow, message: &str, kind: &str) {
    window.set_toast_message(SharedString::from(message));
    window.set_toast_kind(SharedString::from(kind));
    window.set_toast_visible(true);
    // 2 秒后自动隐藏：用一次性 Timer
    let window_weak = window.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(2000),
        move || {
            if let Some(w) = window_weak.upgrade() {
                w.set_toast_visible(false);
            }
        },
    );
    // Timer 必须 forget 保持存活，否则离开作用域即销毁
    std::mem::forget(timer);
}




