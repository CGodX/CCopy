#![windows_subsystem = "windows"]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use slint::{ComponentHandle, Model, SharedString, Timer, TimerMode, VecModel};

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::clipboard_item::ClipboardItem;
use crate::filter::{append_model, ensure_thumbnail, fill_model, thumbnail_path};
use crate::hotkey::HotkeyRegistrar;
use crate::platform::PasteTarget;
use crate::storage::Storage;
use crate::updater::{CheckResult, UpdateInfo};

mod autostart;
mod categories;
mod clipboard_history;
mod clipboard_item;
mod common;
mod drag;
mod filter;
mod hotkey;
mod platform;
mod settings;
mod storage;
mod tray;
mod ui;
mod updater;

pub use ui::*;

/// 设置项键名
pub const SETTING_HOTKEY: &str = "hotkey";
pub const SETTING_MAX_HISTORY: &str = "max_history";
pub const SETTING_MAX_AGE_DAYS: &str = "max_age_days";

/// 单实例检测：通过命名互斥体保证程序只能运行一个实例
/// 返回 true 表示这是唯一实例，可以继续运行；返回 false 表示已有实例在运行
#[cfg(target_os = "windows")]
fn ensure_single_instance() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    // 互斥体名称使用应用标识，保证全局唯一
    let name: Vec<u16> = OsStr::new("CCopy_SingleInstance_Mutex")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        // binitialowner 传 0（FALSE），不主动占有互斥体
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // 创建失败，放行让程序继续运行（避免完全无法启动）
            return true;
        }
        // GetLastError 检查是否已存在同名互斥体
        let err = windows_sys::Win32::Foundation::GetLastError();
        if err == ERROR_ALREADY_EXISTS as u32 {
            CloseHandle(handle);
            return false;
        }
        // 互斥体句柄保持不关闭，进程退出时系统自动回收
        // 使用静态变量持有句柄，防止被优化回收
        static MUTEX_HANDLE: std::sync::atomic::AtomicPtr<std::ffi::c_void> =
            std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
        MUTEX_HANDLE.store(handle, std::sync::atomic::Ordering::SeqCst);
    }
    true
}

#[cfg(not(target_os = "windows"))]
fn ensure_single_instance() -> bool {
    true
}

fn main() {
    // 单实例限制：已有实例在运行则直接退出
    if !ensure_single_instance() {
        return;
    }

    let app = MainWindow::new().unwrap();

    let storage = Rc::new(RefCell::new(Storage::open().unwrap()));

    let history_model = Rc::new(VecModel::default());
    app.set_history(history_model.clone().into());

    // 填充分类元数据，驱动 UI 按钮渲染与键盘循环
    {
        let cat_model = Rc::new(VecModel::from(
            categories::CATEGORIES
                .iter()
                .map(|c| CategoryDef {
                    id: c.id.into(),
                    label: c.label.into(),
                })
                .collect::<Vec<_>>(),
        ));
        app.set_categories(cat_model.into());
    }

    // 当前最大历史数（0=不限制），从设置加载，运行时可变更
    let max_history = Rc::new(Cell::new(settings::load_max_history_value(&storage)));
    // 当前最大保留天数（0=不限制），从设置加载，运行时可变更
    let max_age_days = Rc::new(Cell::new(settings::load_max_age_value(&storage)));

    // 分页加载状态：列表与清理规则解耦，列表展示按页加载
    const PAGE_SIZE: usize = 50;
    let loaded_count = Rc::new(Cell::new(0usize));
    let has_more = Rc::new(Cell::new(true));
    let loading = Rc::new(Cell::new(false));
    let current_query = Rc::new(RefCell::new(String::new()));
    let current_category = Rc::new(RefCell::new(categories::DEFAULT_CATEGORY.to_string()));

    // 重载历史（重置分页，查第一页）：用于打开面板、切换分类、搜索、删除等场景
    let selected_category = Rc::new(RefCell::new(categories::DEFAULT_CATEGORY.to_string()));
    let selected_id = Rc::new(Cell::new(0));

    // 异步缩略图加载：后台线程只做磁盘 IO（ensure_thumbnail），就绪后回传 id，
    // 解码成 slint::Image 必须在主线程（slint::Image 非 Send）。
    // 用取消令牌避免旧批次（切换分类/搜索）的结果覆盖新批次
    let thumb_cancel: Arc<std::sync::atomic::AtomicBool> = Arc::new(false.into());
    let (thumb_tx, thumb_rx) = mpsc::channel::<(i32, String)>();
    let thumb_rx = Rc::new(RefCell::new(thumb_rx));
    let history_model_for_thumb = history_model.clone();
    let app_weak_for_thumb = app.as_weak();
    let thumb_timer = Timer::default();
    {
        let thumb_rx = thumb_rx.clone();
        let history_model_for_thumb = history_model_for_thumb.clone();
        let app_weak_for_thumb = app_weak_for_thumb.clone();
        thumb_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
            // 一次性取出当前所有就绪 id，批量解码后统一写入，减少重绘
            let mut ready = Vec::new();
            while let Ok(pair) = thumb_rx.borrow().try_recv() {
                ready.push(pair);
            }
            if ready.is_empty() {
                return;
            }
            let Some(app) = app_weak_for_thumb.upgrade() else { return };
            let data_dir = crate::storage::data_dir();
            // 构建 id->index 映射，避免每张图都全表线性查找
            let model = app.get_history();
            let mut idx_map: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
            for i in 0..model.row_count() {
                if let Some(row) = model.row_data(i) {
                    idx_map.insert(row.id, i);
                }
            }
            for (id, blob_path) in ready {
                let Some(&idx) = idx_map.get(&id) else { continue };
                let Some(thumb_path) = thumbnail_path(&data_dir, &blob_path) else { continue };
                // 主线程解码缩略图并写入对应行
                if let Ok(img) = slint::Image::load_from_path(&thumb_path) {
                    if let Some(mut row) = model.row_data(idx) {
                        row.thumbnail = img;
                        let _ = history_model_for_thumb.set_row_data(idx, row);
                    }
                }
            }
        });
    }
    std::mem::forget(thumb_timer);

    let load_thumbnails_fn: Rc<dyn Fn(Vec<(i32, String)>)> = Rc::new({
        let thumb_cancel = thumb_cancel.clone();
        let thumb_tx = thumb_tx.clone();
        move |pending: Vec<(i32, String)>| {
            if pending.is_empty() {
                return;
            }
            // 取消上一批次：翻转令牌，旧线程检测到即退出
            thumb_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            let cancelled = thumb_cancel.clone();
            thumb_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
            let tx = thumb_tx.clone();
            let data_dir = crate::storage::data_dir();
            std::thread::spawn(move || {
                for (id, blob_path) in pending {
                    if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    ensure_thumbnail(&data_dir, &blob_path);
                    if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    // 只回传 id 与 blob_path，解码留在主线程
                    let _ = tx.send((id, blob_path));
                }
            });
        }
    });

    // 首次加载第一页
    {
        if let Ok(items) = storage.borrow().query_items("", categories::DEFAULT_CATEGORY, PAGE_SIZE, 0) {
            let pending = fill_model(&history_model, &items);
            loaded_count.set(items.len());
            has_more.set(items.len() == PAGE_SIZE);
            load_thumbnails_fn.clone()(pending);
        }
    }

let ignore_next_clipboard_change = Rc::new(Cell::new(false));
let paste_target = Rc::new(Cell::new(None));
// 面板最近一次显示时间，用于失焦自动隐藏的延迟判断
let last_shown = Rc::new(Cell::new(None::<Instant>));

app.set_edit_note_id(-1);

fn reset_selected_id(app: &MainWindow, selected_id: &Cell<i32>, _storage: &Storage) {
    let first = app.get_history().iter().next().map(|i| i.id).unwrap_or(0);
    selected_id.set(first);
    app.set_selected_id(first);
}

reset_selected_id(&app, &selected_id, &storage.borrow());

// 重载历史第一页：重置分页状态，按当前 query/category 查询并填充 model
let reload_history: Rc<dyn Fn()> = Rc::new({
    let storage = storage.clone();
    let history_model = history_model.clone();
    let loaded_count = loaded_count.clone();
    let has_more = has_more.clone();
    let current_query = current_query.clone();
    let current_category = current_category.clone();
    let load_thumbs = load_thumbnails_fn.clone();
    let app_weak = app.as_weak();
    move || {
        let query = current_query.borrow().clone();
        let category = current_category.borrow().clone();
        if let Ok(items) = storage
            .borrow()
            .query_items(&query, &category, PAGE_SIZE, 0)
        {
            let pending = fill_model(&history_model, &items);
            loaded_count.set(items.len());
            has_more.set(items.len() == PAGE_SIZE);
            // app 仍存活才回填缩略图
            if app_weak.upgrade().is_some() {
                load_thumbs(pending);
            }
        }
    }
});

let drag_state = Rc::new(RefCell::new(None::<drag::DragState>));
let app_weak = app.as_weak();
let drag_state_clone = drag_state.clone();
app.on_drag_started(move || {
    if let Some(app) = app_weak.upgrade() {
        if let Some((cursor_x, cursor_y)) = drag::cursor_position() {
            let window_position = app.window().position();
            *drag_state_clone.borrow_mut() = Some(drag::DragState {
                window_x: window_position.x,
                window_y: window_position.y,
                cursor_x,
                cursor_y,
            });
        }
    }
});

let app_weak = app.as_weak();
let drag_state_weak = drag_state.clone();
app.on_dragged(move || {
    if let Some(state) = *drag_state_weak.borrow() {
        if let Some((cursor_x, cursor_y)) = drag::cursor_position() {
            let dx = cursor_x - state.cursor_x;
            let dy = cursor_y - state.cursor_y;
            if let Some(app) = app_weak.upgrade() {
                app.window().set_position(slint::PhysicalPosition::new(
                    state.window_x + dx,
                    state.window_y + dy,
                ));
            }
        }
    }
});

    let selected_id_for_open = selected_id.clone();
    let selected_category_for_open = selected_category.clone();
    let reload_for_open = reload_history.clone();
    let current_query_for_open = current_query.clone();
    let current_category_for_open = current_category.clone();
    let last_shown_for_open = last_shown.clone();
    let storage_for_open = storage.clone();
    let max_history_for_open = max_history.clone();
    let max_age_for_open = max_age_days.clone();
    let app_weak = app.as_weak();
    app.on_panel_opened(move || {
        // 面板打开时按规则清理一次，覆盖「不复制但会看面板」的场景
        let _ = storage_for_open
            .borrow()
            .purge_by_rule(max_history_for_open.get(), max_age_for_open.get());
        // 重置搜索和分类，重新加载第一页
        *current_query_for_open.borrow_mut() = String::new();
        *current_category_for_open.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
        *selected_category_for_open.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
        reload_for_open();
        last_shown_for_open.set(Some(Instant::now()));
        if let Some(app) = app_weak.upgrade() {
            app.set_search_text(String::new().into());
            app.set_selected_category(categories::DEFAULT_CATEGORY.into());
            app.set_edit_note_id(-1);
            app.set_viewport_y(0.0);
            reset_selected_id(&app, &selected_id_for_open, &storage_for_open.borrow());
            // 延迟触发 focus，等窗口完成系统激活后再设置输入焦点
            let app_weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                if let Some(app) = app_weak.upgrade() {
                    app.invoke_focus_search();
                }
            });
        }
    });

let selected_id_for_search = selected_id.clone();
let current_query_for_search = current_query.clone();
let current_category_for_search = current_category.clone();
let selected_category_for_search = selected_category.clone();
let storage_for_search = storage.clone();
let reload_for_search = reload_history.clone();
let app_weak = app.as_weak();
app.on_search_changed(move |query| {
    *current_query_for_search.borrow_mut() = query.to_string();
    *current_category_for_search.borrow_mut() = selected_category_for_search.borrow().clone();
    reload_for_search();
    if let Some(app) = app_weak.upgrade() {
        app.set_viewport_y(0.0);
        reset_selected_id(&app, &selected_id_for_search, &storage_for_search.borrow());
    }
});

    // 应用分类切换：写 selected/current category，按当前 query 重载列表，重置选中项
    let apply_category: Rc<dyn Fn(&str)> = Rc::new({
        let selected_category = selected_category.clone();
        let current_category = current_category.clone();
        let current_query = current_query.clone();
        let reload_history = reload_history.clone();
        let selected_id = selected_id.clone();
        let storage_for_category = storage.clone();
        let app_weak = app.as_weak();
        move |category: &str| {
            *selected_category.borrow_mut() = category.to_string();
            *current_category.borrow_mut() = category.to_string();
            let q = if let Some(app) = app_weak.upgrade() {
                app.get_search_text().to_string()
            } else {
                current_query.borrow().clone()
            };
            *current_query.borrow_mut() = q;
            reload_history();
            if let Some(app) = app_weak.upgrade() {
                app.set_selected_category(category.into());
                app.set_viewport_y(0.0);
                reset_selected_id(&app, &selected_id, &storage_for_category.borrow());
            }
        }
    });

    let apply_category_for_changed = apply_category.clone();
    app.on_category_changed(move |category| {
        apply_category_for_changed(category.as_str());
    });

    // 键盘 Alt+←/→ 循环分类：Rust 算出下一个，复用 apply_category 落地
    let apply_category_for_cycle = apply_category.clone();
    let app_weak_for_cycle = app.as_weak();
    app.on_cycle_category(move |direction| {
        let cur = app_weak_for_cycle
            .upgrade()
            .map(|a| a.get_selected_category().to_string())
            .unwrap_or_else(|| categories::DEFAULT_CATEGORY.to_string());
        let next = categories::cycle(&cur, direction);
        apply_category_for_cycle(next);
    });

    let history_model_for_move = history_model.clone();
    let selected_id_for_move = selected_id.clone();
    let app_weak = app.as_weak();
    app.on_move_selection(move |delta| {
        // 从已加载的 model 读取可见项做键盘导航
        let visible: Vec<i32> = history_model_for_move
            .iter()
            .map(|i| i.id)
            .collect();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|id| *id == selected_id_for_move.get())
            .unwrap_or(0);
        let next = (pos as i32 + delta).clamp(0, visible.len() as i32 - 1) as usize;
        let id = visible[next];
        selected_id_for_move.set(id);
        if let Some(app) = app_weak.upgrade() {
            app.set_selected_id(id);
            // 滚动确保选中项可见：图片项 150px，文本项 85px，间距 8px
            const IMAGE_ITEM_H: f32 = 150.0;
            const TEXT_ITEM_H: f32 = 85.0;
            const SPACING: f32 = 8.0;
            let has_thumb = history_model_for_move
                .row_data(next)
                .map(|r| r.has_thumbnail)
                .unwrap_or(false);
            let item_h = if has_thumb { IMAGE_ITEM_H } else { TEXT_ITEM_H };
            // 累加前面所有项高度得到当前项顶部偏移
            let mut item_top = 0.0;
            for i in 0..next {
                let h = history_model_for_move
                    .row_data(i)
                    .map(|r| if r.has_thumbnail { IMAGE_ITEM_H } else { TEXT_ITEM_H })
                    .unwrap_or(TEXT_ITEM_H);
                item_top += h + SPACING;
            }
            let item_bottom = item_top + item_h;
            let visible_h = app.get_visible_height() as f32;
            let mut vp = app.get_viewport_y() as f32;
            if item_top < -vp {
                vp = -item_top;
            } else if item_bottom > -vp + visible_h {
                vp = -(item_bottom - visible_h);
            }
            let max_vp = app.get_content_height() as f32 - visible_h;
            let vp = vp.min(0.0).max(-max_vp.max(0.0));
            app.set_viewport_y(vp);
        }
    });

    let app_weak = app.as_weak();
    let selected_category_for_confirm = selected_category.clone();
    let selected_id_for_confirm = selected_id.clone();
    let ignore_for_confirm = ignore_next_clipboard_change.clone();
    let storage_for_confirm = storage.clone();
    let paste_target_for_confirm = paste_target.clone();
    let reload_for_confirm = reload_history.clone();
    let current_query_for_confirm = current_query.clone();
    let current_category_for_confirm = current_category.clone();
    app.on_confirm_selection(move || {
        // 按 id 从库查单条，不再依赖内存 visible 列表
        let item = match storage_for_confirm
            .borrow()
            .get_item(selected_id_for_confirm.get() as i64)
        {
            Ok(Some(item)) => item,
            _ => return,
        };

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_confirm.set(true);
            if let Some(id) = item.id {
                let _ = storage_for_confirm
                    .borrow_mut()
                    .mark_used(id, crate::common::now_millis());
            }
            // 重置搜索和分类，重新加载第一页
            *current_query_for_confirm.borrow_mut() = String::new();
            *current_category_for_confirm.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
            *selected_category_for_confirm.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
            reload_for_confirm();
            if let Some(app) = app_weak.upgrade() {
                app.set_search_text(String::new().into());
                app.set_selected_category(categories::DEFAULT_CATEGORY.into());
                reset_selected_id(&app, &selected_id_for_confirm, &storage_for_confirm.borrow());
                let _ = app.hide();
            }
            if let Some(target) = paste_target_for_confirm.get() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            } else if let Some(target) = crate::platform::foreground_window() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            }
        }
    });

    let app_weak = app.as_weak();
    let history_model_for_note = history_model.clone();
    let storage_for_note = storage.clone();
    app.on_edit_note_confirm(move |id, note_text| {
        let note = if note_text.is_empty() {
            None
        } else {
            Some(note_text.to_string())
        };
        if storage_for_note.borrow_mut().update_note(id as i64, note).is_ok() {
            // 只更新 model 中对应行的 note 显示，不重置分页
            let target_id = id;
            for idx in 0..history_model_for_note.row_count() {
                let row = history_model_for_note.row_data(idx).unwrap();
                if row.id == target_id {
                    let mut new_row = row;
                    new_row.note = SharedString::from(
                        note_text.to_string(),
                    );
                    history_model_for_note.set_row_data(idx, new_row);
                    break;
                }
            }
        }
        if let Some(app) = app_weak.upgrade() {
            app.set_edit_note_id(-1);
            app.invoke_focus_search();
        }
    });

    let app_weak = app.as_weak();
    app.on_edit_note_cancel(move || {
        if let Some(app) = app_weak.upgrade() {
            app.set_edit_note_id(-1);
            app.invoke_focus_search();
        }
    });

    // F2 编辑选中项备注：根据 selected_id 查找对应 index
    let history_model_for_edit = history_model.clone();
    let selected_id_for_edit = selected_id.clone();
    let app_weak = app.as_weak();
    app.on_edit_selected_note(move || {
        let target_id = selected_id_for_edit.get();
        for idx in 0..history_model_for_edit.row_count() {
            let row = history_model_for_edit.row_data(idx).unwrap();
            if row.id == target_id {
                if let Some(app) = app_weak.upgrade() {
                    app.set_edit_note_id(target_id);
                    app.set_edit_note_text(row.note);
                    app.set_edit_note_row_index(idx as i32);
                }
                break;
            }
        }
    });

    let app_weak = app.as_weak();
    app.on_hide_requested(move || {
        if let Some(app) = app_weak.upgrade() {
            let _ = app.hide();
        }
    });

    let app_weak = app.as_weak();
    let selected_category_for_select = selected_category.clone();
    let selected_id_for_select = selected_id.clone();
    let ignore_for_select = ignore_next_clipboard_change.clone();
    let storage_for_select = storage.clone();
    let paste_target_for_select = paste_target.clone();
    let reload_for_select = reload_history.clone();
    let current_query_for_select = current_query.clone();
    let current_category_for_select = current_category.clone();
    app.on_item_selected(move |id| {
        // 按 id 从库查单条
        let item = match storage_for_select.borrow().get_item(id as i64) {
            Ok(Some(item)) => item,
            _ => return,
        };
        selected_id_for_select.set(id);

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_select.set(true);
            if let Some(item_id) = item.id {
                let _ = storage_for_select
                    .borrow_mut()
                    .mark_used(item_id, crate::common::now_millis());
            }
            // 重置搜索和分类，重新加载第一页
            *current_query_for_select.borrow_mut() = String::new();
            *current_category_for_select.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
            *selected_category_for_select.borrow_mut() = categories::DEFAULT_CATEGORY.to_string();
            reload_for_select();
            if let Some(app) = app_weak.upgrade() {
                app.set_search_text(String::new().into());
                app.set_selected_category(categories::DEFAULT_CATEGORY.into());
                reset_selected_id(&app, &selected_id_for_select, &storage_for_select.borrow());
                let _ = app.hide();
            }
            if let Some(target) = paste_target_for_select.get() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            } else if let Some(target) = crate::platform::foreground_window() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            }
        }
    });

    let history_model_for_delete = history_model.clone();
    let storage_for_delete = storage.clone();
    let loaded_count_for_delete = loaded_count.clone();
    app.on_item_deleted(move |id| {
        let _ = storage_for_delete.borrow_mut().delete_item(id as i64);
        // 从 model 移除对应行，不重置分页
        for idx in 0..history_model_for_delete.row_count() {
            if history_model_for_delete.row_data(idx).unwrap().id == id {
                history_model_for_delete.remove(idx);
                let cur = loaded_count_for_delete.get();
                if cur > 0 {
                    loaded_count_for_delete.set(cur - 1);
                }
                break;
            }
        }
    });

    let (tx, rx) = mpsc::channel();
    crate::clipboard_history::start_clipboard_watcher(tx);
    let ignore_next = ignore_next_clipboard_change.clone();
    let storage_for_cb = storage.clone();
    let max_history_for_cb = max_history.clone();
    let max_age_for_cb = max_age_days.clone();
    let reload_for_cb = reload_history.clone();
    let rx = std::rc::Rc::new(std::cell::RefCell::new(rx));
    let watcher_timer = slint::Timer::default();
    watcher_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            let rx = rx.borrow();
            if let Ok(item) = rx.try_recv() {
                if ignore_next.get() {
                    ignore_next.set(false);
                    return;
                }
                // 写入 + 清理在同一个 borrow 内完成，之后释放再 reload
                let upserted = {
                    let storage = storage_for_cb.borrow_mut();
                    let id = storage.upsert_item(&item);
                    if id.is_ok() {
                        let _ = storage
                            .purge_by_rule(max_history_for_cb.get(), max_age_for_cb.get());
                    }
                    id
                };
                if upserted.is_ok() {
                    // 刷新第一页（新条目排在最前）
                    reload_for_cb();
                }
            }
        },
    );
    std::mem::forget(watcher_timer);

    // 滚动到底部加载更多：按当前 query/category 查下一页，追加到 model
    let storage_for_more = storage.clone();
    let history_model_for_more = history_model.clone();
    let loaded_count_for_more = loaded_count.clone();
    let has_more_for_more = has_more.clone();
    let loading_for_more = loading.clone();
    let current_query_for_more = current_query.clone();
    let current_category_for_more = current_category.clone();
    let load_thumbs_for_more = load_thumbnails_fn.clone();
    app.on_load_more(move || {
        // 无更多数据或正在加载时跳过，防重复
        if !has_more_for_more.get() || loading_for_more.get() {
            return;
        }
        loading_for_more.set(true);
        let query = current_query_for_more.borrow().clone();
        let category = current_category_for_more.borrow().clone();
        let offset = loaded_count_for_more.get();
        if let Ok(items) = storage_for_more
            .borrow()
            .query_items(&query, &category, PAGE_SIZE, offset)
        {
            let pending = append_model(&history_model_for_more, &items);
            let new_count = offset + items.len();
            loaded_count_for_more.set(new_count);
            has_more_for_more.set(items.len() == PAGE_SIZE);
            // 仅追加页的缩略图进后台队列，不影响已加载项
            load_thumbs_for_more(pending);
        }
        loading_for_more.set(false);
    });

    let app_weak = app.as_weak();
    app.window().on_close_requested(move || {
        if let Some(app) = app_weak.upgrade() {
            let _ = app.hide();
        }
        slint::CloseRequestResponse::HideWindow
    });

    // 热键注册器：从设置加载快捷键规格，支持运行时替换
    let mut registrar = match HotkeyRegistrar::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("创建热键管理器失败: {e}");
            app.run().unwrap();
            return;
        }
    };
    let spec = settings::load_hotkey_spec(&storage);
    let current_hotkey_id = Rc::new(Cell::new(registrar.register(&spec)));
    let registrar = Rc::new(RefCell::new(registrar));

    // 设置窗口强引用（保持存活），关闭时置 None
    let settings_window_ref: Rc<RefCell<Option<SettingsWindow>>> =
        Rc::new(RefCell::new(None));

    // 共享的更新检查结果：启动自动检查与设置页手动检查共用（跨线程，用 Arc+Mutex）
    let update_info: Arc<Mutex<Option<UpdateInfo>>> = Arc::new(Mutex::new(None));

    // 打开设置的统一逻辑：主窗口按钮和托盘菜单共用
    let open_settings_fn: Rc<dyn Fn()> = Rc::new({
        let storage = storage.clone();
        let registrar = registrar.clone();
        let hotkey_id = current_hotkey_id.clone();
        let settings_ref = settings_window_ref.clone();
        let selected_id = selected_id.clone();
        let max_history = max_history.clone();
        let max_age_days = max_age_days.clone();
        let reload_history = reload_history.clone();
        let update_info = update_info.clone();
        let app_weak = app.as_weak();
        move || {
            let app = app_weak.clone();
            let storage = storage.clone();
            let registrar = registrar.clone();
            let hotkey_id = hotkey_id.clone();
            let settings_ref = settings_ref.clone();
            let selected_id = selected_id.clone();
            let max_history = max_history.clone();
            let max_age_days = max_age_days.clone();
            let reload_history = reload_history.clone();
            let update_info = update_info.clone();
            let storage_for_refresh = storage.clone();
            let refresh = move || {
                max_history.set(settings::load_max_history_value(&storage_for_refresh));
                max_age_days.set(settings::load_max_age_value(&storage_for_refresh));
                // 清理规则变更后重新加载第一页
                reload_history();
                if let Some(app) = app.upgrade() {
                    reset_selected_id(&app, &selected_id, &storage_for_refresh.borrow());
                }
            };
            settings::open(storage, registrar, hotkey_id, settings_ref, refresh, update_info);
        }
    });

    // 主窗口设置按钮
    let open_for_btn = open_settings_fn.clone();
    app.on_open_settings(move || open_for_btn());

    // 固钉切换：状态已由 UI 双向绑定，此处无需额外处理
    app.on_pin_toggled(|| {});

    // 监听全局热键事件
    let app_weak = app.as_weak();
    let paste_target_for_hotkey = paste_target.clone();
    let hotkey_id_for_timer = current_hotkey_id.clone();
    let last_shown_for_timer = last_shown.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
        let Some(expected_id) = hotkey_id_for_timer.get() else {
            return;
        };
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == expected_id && event.state == HotKeyState::Pressed {
                if let Some(app) = app_weak.upgrade() {
                    crate::platform::open_panel_for_target(&app, &paste_target_for_hotkey);
                }
            }
        }
        // 窗口失去焦点自动隐藏（显示超过 250ms 且应用不在前台）
        // 固钉模式下窗口保持显示，不自动隐藏
        if let Some(app) = app_weak.upgrade() {
            if !app.get_pinned()
                && app.window().is_visible()
                && last_shown_for_timer.get().is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(250))
                && !crate::platform::is_app_foreground(&app)
            {
                let _ = app.hide();
                last_shown_for_timer.set(None);
            }
        }
    });

    let open_for_tray = open_settings_fn.clone();
    let _tray = tray::create_tray_icon(app.as_weak(), paste_target.clone(), Box::new(move || open_for_tray()));

    // 启动后后台静默检查更新（延迟 3 秒，避免启动卡顿），结果存入共享状态供设置页读取
    {
        let update_info = update_info.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            if let Ok(CheckResult::Available(info)) = updater::check() {
                *update_info.lock().unwrap() = Some(info);
            }
        });
    }

    let _keep_alive = (registrar, timer);
    slint::run_event_loop_until_quit().unwrap();
}

fn restore_clipboard_item(item: &ClipboardItem) -> clipboard_rs::Result<()> {
    use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext};
    let clipboard = ClipboardContext::new()?;

    match item.kind {
        crate::clipboard_item::ClipboardKind::Text
        | crate::clipboard_item::ClipboardKind::Html
        | crate::clipboard_item::ClipboardKind::Rtf => {
            if let Some(ref text) = item.plain_text {
                clipboard.set(vec![ClipboardContent::Text(text.clone())])?;
            }
        }
        crate::clipboard_item::ClipboardKind::Image => {
            if let Some(ref blob) = item.blob_path {
                let full_path = crate::storage::data_dir().join(blob);
                if let Ok(data) = std::fs::read(full_path) {
                    use clipboard_rs::common::{RustImage, RustImageData};
                    if let Ok(img) = RustImageData::from_bytes(&data) {
                        clipboard.set(vec![clipboard_rs::ClipboardContent::Image(img)])?;
                    }
                }
            }
        }
        crate::clipboard_item::ClipboardKind::Files => {
            let files: Vec<String> = item.files.iter().map(|f| f.path.clone()).collect();
            clipboard.set(vec![ClipboardContent::Files(files)])?;
        }
        _ => {}
    }
    Ok(())
}
