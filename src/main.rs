#![windows_subsystem = "windows"]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use slint::{Model, Timer, TimerMode, VecModel};

use crate::clipboard_item::ClipboardItem;
use crate::filter::{apply_filter, set_history};
use crate::platform::PasteTarget;
use crate::storage::Storage;

mod clipboard_history;
mod clipboard_item;
mod common;
mod drag;
mod filter;
mod platform;
mod storage;
mod tray;
mod ui;

pub use ui::*;

pub const MAX_HISTORY: usize = 30;

fn main() {
    let app = MainWindow::new().unwrap();

    let storage = Rc::new(RefCell::new(Storage::open().unwrap()));

    let history = Rc::new(RefCell::new(Vec::new()));
    let visible_history = Rc::new(RefCell::new(Vec::new()));
    let history_model = Rc::new(VecModel::default());
    app.set_history(history_model.clone().into());

    if let Ok(items) = storage.borrow().recent_items(MAX_HISTORY) {
        set_history(&history, &history_model, items);
        apply_filter(&history, &visible_history, &history_model, "", "all");
    }

    let selected_category = Rc::new(RefCell::new("all".to_string()));
let selected_id = Rc::new(Cell::new(0));
let ignore_next_clipboard_change = Rc::new(Cell::new(false));
let paste_target = Rc::new(Cell::new(None));

app.set_edit_note_id(-1);

fn reset_selected_id(app: &MainWindow, selected_id: &Cell<i32>) {
    let first = app.get_history().iter().next().map(|i| i.id).unwrap_or(0);
    selected_id.set(first);
    app.set_selected_id(first);
}

reset_selected_id(&app, &selected_id);

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
    let all_history_for_open = history.clone();
    let visible_history_for_open = visible_history.clone();
    let history_model_for_open = history_model.clone();
    let app_weak = app.as_weak();
    app.on_panel_opened(move || {
        *selected_category_for_open.borrow_mut() = "all".to_string();
        apply_filter(
            &all_history_for_open,
            &visible_history_for_open,
            &history_model_for_open,
            "",
            "all",
        );
        if let Some(app) = app_weak.upgrade() {
            app.set_search_text(String::new().into());
            app.set_selected_category("all".into());
            app.set_edit_note_id(-1);
            reset_selected_id(&app, &selected_id_for_open);
            // 延迟触发 focus，等窗口完成系统激活后再设置输入焦点
            let app_weak = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                if let Some(app) = app_weak.upgrade() {
                    app.invoke_focus_search();
                }
            });
        }
    });

let all_history_for_search = history.clone();
let visible_history_for_search = visible_history.clone();
let history_model_for_search = history_model.clone();
let selected_category_for_search = selected_category.clone();
let selected_id_for_search = selected_id.clone();
let app_weak = app.as_weak();
app.on_search_changed(move |query| {
    apply_filter(
        &all_history_for_search,
        &visible_history_for_search,
        &history_model_for_search,
        query.as_str(),
        selected_category_for_search.borrow().as_str(),
    );
    if let Some(app) = app_weak.upgrade() {
        reset_selected_id(&app, &selected_id_for_search);
    }
});

    let app_weak = app.as_weak();
    let all_history_for_category = history.clone();
    let visible_history_for_category = visible_history.clone();
    let history_model_for_category = history_model.clone();
    let selected_id_for_category = selected_id.clone();
    let selected_category_for_category = selected_category.clone();
    app.on_category_changed(move |category| {
        *selected_category_for_category.borrow_mut() = category.to_string();
        if let Some(app) = app_weak.upgrade() {
            let query = app.get_search_text().to_string();
            apply_filter(
                &all_history_for_category,
                &visible_history_for_category,
                &history_model_for_category,
                query.as_str(),
                category.as_str(),
            );
            reset_selected_id(&app, &selected_id_for_category);
        }
    });

    let visible_history_for_move = visible_history.clone();
    let selected_id_for_move = selected_id.clone();
    let app_weak = app.as_weak();
    app.on_move_selection(move |delta| {
        let visible = visible_history_for_move.borrow();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|i| i.id == Some(selected_id_for_move.get() as i64))
            .unwrap_or(0);
        let next = (pos as i32 + delta).clamp(0, visible.len() as i32 - 1) as usize;
        let id = visible[next].id.unwrap_or(0) as i32;
        selected_id_for_move.set(id);
        if let Some(app) = app_weak.upgrade() {
            app.set_selected_id(id);
            const ITEM_H: f32 = 78.0;
            const SPACING: f32 = 10.0;
            const STEP: f32 = ITEM_H + SPACING;
            let item_top = next as f32 * STEP;
            let item_bottom = item_top + ITEM_H;
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
    let visible_history_for_confirm = visible_history.clone();
    let all_history_for_confirm = history.clone();
    let history_model_for_confirm = history_model.clone();
    let selected_category_for_confirm = selected_category.clone();
    let selected_id_for_confirm = selected_id.clone();
    let ignore_for_confirm = ignore_next_clipboard_change.clone();
    let storage_for_confirm = storage.clone();
    let paste_target_for_confirm = paste_target.clone();
    app.on_confirm_selection(move || {
        let Some(item) = visible_history_for_confirm
            .borrow()
            .iter()
            .find(|i| i.id == Some(selected_id_for_confirm.get() as i64))
            .cloned()
        else {
            return;
        };

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_confirm.set(true);
            if let Some(id) = item.id {
                let _ = storage_for_confirm
                    .borrow_mut()
                    .mark_used(id, crate::common::now_millis());
            }
            if let Ok(items) = storage_for_confirm.borrow().recent_items(MAX_HISTORY) {
                *all_history_for_confirm.borrow_mut() = items;
                if let Some(app) = app_weak.upgrade() {
                    app.set_search_text(String::new().into());
                    app.set_selected_category("all".into());
                    *selected_category_for_confirm.borrow_mut() = "all".to_string();
                    apply_filter(
                        &all_history_for_confirm,
                        &visible_history_for_confirm,
                        &history_model_for_confirm,
                        "",
                        "all",
                    );
                    reset_selected_id(&app, &selected_id_for_confirm);
                    let _ = app.hide();
                }
            } else if let Some(app) = app_weak.upgrade() {
                let _ = app.hide();
            }
            if let Some(target) = paste_target_for_confirm.get() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            } else {
                crate::platform::paste_to_target(PasteTarget::Foreground(unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
                }));
            }
        }
    });

    let app_weak = app.as_weak();
    let all_history_for_note = history.clone();
    let visible_history_for_note = visible_history.clone();
    let history_model_for_note = history_model.clone();
    let selected_category_for_note = selected_category.clone();
    let storage_for_note = storage.clone();
    app.on_edit_note_confirm(move |id, note_text| {
        let note = if note_text.is_empty() {
            None
        } else {
            Some(note_text.to_string())
        };
        if storage_for_note.borrow_mut().update_note(id as i64, note).is_ok() {
            if let Ok(items) = storage_for_note.borrow().recent_items(MAX_HISTORY) {
                *all_history_for_note.borrow_mut() = items;
                if let Some(app) = app_weak.upgrade() {
                    let query = app.get_search_text().to_string();
                    apply_filter(
                        &all_history_for_note,
                        &visible_history_for_note,
                        &history_model_for_note,
                        query.as_str(),
                        selected_category_for_note.borrow().as_str(),
                    );
                }
            }
        }
        if let Some(app) = app_weak.upgrade() {
            app.set_edit_note_id(-1);
        }
    });

    let app_weak = app.as_weak();
    app.on_edit_note_cancel(move || {
        if let Some(app) = app_weak.upgrade() {
            app.set_edit_note_id(-1);
        }
    });

    let app_weak = app.as_weak();
    app.on_hide_requested(move || {
        if let Some(app) = app_weak.upgrade() {
            let _ = app.hide();
        }
    });

    let app_weak = app.as_weak();
    let visible_history_for_select = visible_history.clone();
    let all_history_for_select = history.clone();
    let history_model_for_select = history_model.clone();
    let selected_category_for_select = selected_category.clone();
    let selected_id_for_select = selected_id.clone();
    let ignore_for_select = ignore_next_clipboard_change.clone();
    let storage_for_select = storage.clone();
    let paste_target_for_select = paste_target.clone();
    app.on_item_selected(move |id| {
        let Some(item) = visible_history_for_select
            .borrow()
            .iter()
            .find(|i| i.id == Some(id as i64))
            .cloned()
        else {
            return;
        };
        selected_id_for_select.set(id);

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_select.set(true);
            if let Some(item_id) = item.id {
                let _ = storage_for_select
                    .borrow_mut()
                    .mark_used(item_id, crate::common::now_millis());
            }
            if let Ok(items) = storage_for_select.borrow().recent_items(MAX_HISTORY) {
                *all_history_for_select.borrow_mut() = items;
                if let Some(app) = app_weak.upgrade() {
                    app.set_search_text(String::new().into());
                    app.set_selected_category("all".into());
                    *selected_category_for_select.borrow_mut() = "all".to_string();
                    apply_filter(
                        &all_history_for_select,
                        &visible_history_for_select,
                        &history_model_for_select,
                        "",
                        "all",
                    );
                    reset_selected_id(&app, &selected_id_for_select);
                    let _ = app.hide();
                }
            } else if let Some(app) = app_weak.upgrade() {
                let _ = app.hide();
            }
            if let Some(target) = paste_target_for_select.get() {
                crate::platform::paste_to_target(PasteTarget::Foreground(target));
            } else {
                crate::platform::paste_to_target(PasteTarget::Foreground(unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
                }));
            }
        }
    });

    let app_weak = app.as_weak();
    let all_history_for_delete = history.clone();
    let visible_history_for_delete = visible_history.clone();
    let history_model_for_delete = history_model.clone();
    let storage_for_delete = storage.clone();
    app.on_item_deleted(move |id| {
        let _ = storage_for_delete.borrow_mut().delete_item(id as i64);
        if let Ok(items) = storage_for_delete.borrow().recent_items(MAX_HISTORY) {
            *all_history_for_delete.borrow_mut() = items;
            if let Some(app) = app_weak.upgrade() {
                let query = app.get_search_text().to_string();
                let category = app.get_selected_category().to_string();
                apply_filter(
                    &all_history_for_delete,
                    &visible_history_for_delete,
                    &history_model_for_delete,
                    query.as_str(),
                    category.as_str(),
                );
            }
        }
    });

    let (tx, rx) = mpsc::channel();
    crate::clipboard_history::start_clipboard_watcher(tx);
    let ignore_next = ignore_next_clipboard_change.clone();
    let storage_for_cb = storage.clone();
    let history_for_cb = history.clone();
    let visible_history_for_cb = visible_history.clone();
    let history_model_for_cb = history_model.clone();
    let selected_category_for_cb = selected_category.clone();
    let app_handle = app.clone_strong();
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
                let storage = storage_for_cb.borrow_mut();
                if let Ok(id) = storage.upsert_item(&item) {
                    let _item = ClipboardItem {
                        id: Some(id),
                        ..item
                    };
                    if let Ok(items) = storage.recent_items(MAX_HISTORY) {
                        *history_for_cb.borrow_mut() = items;
                        let query = app_handle.get_search_text().to_string();
                        apply_filter(
                            &history_for_cb,
                            &visible_history_for_cb,
                            &history_model_for_cb,
                            query.as_str(),
                            selected_category_for_cb.borrow().as_str(),
                        );
                    }
                }
            }
        },
    );
    std::mem::forget(watcher_timer);

    let app_weak = app.as_weak();
    app.window().on_close_requested(move || {
        if let Some(app) = app_weak.upgrade() {
            let _ = app.hide();
        }
        slint::CloseRequestResponse::HideWindow
    });

    let hotkey_manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("注册全局热键失败: {e}");
            app.run().unwrap();
            return;
        }
    };
    let toggle_hotkey = HotKey::new(Some(Modifiers::ALT), Code::KeyV);
    if let Err(e) = hotkey_manager.register(toggle_hotkey) {
        eprintln!("注册 Alt+V 失败: {e}");
    }

    let app_weak = app.as_weak();
    let paste_target_for_hotkey = paste_target.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == toggle_hotkey.id() && event.state == HotKeyState::Pressed {
                if let Some(app) = app_weak.upgrade() {
                    crate::platform::open_panel_for_target(&app, &paste_target_for_hotkey);
                }
            }
        }
    });

    let _tray = tray::create_tray_icon(app.as_weak(), paste_target.clone());

    let _keep_alive = (hotkey_manager, timer);
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
