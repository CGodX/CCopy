#![windows_subsystem = "windows"]

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clipboard_rs::{common::RustImage, Clipboard, ClipboardContext};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, PhysicalPosition, SharedString, Timer,
    TimerMode, VecModel,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    MouseButton, TrayIconBuilder, TrayIconEvent,
};

mod clipboard_history;
mod clipboard_item;
mod drag;
mod platform;
mod storage;
mod tray;

use clipboard_history::{
    now_millis, start_clipboard_watcher, MAX_HISTORY,
};
use clipboard_item::{ClipboardItem, ClipboardKind};
use drag::{cursor_position, DragState};
use platform::{is_app_foreground, open_panel_for_target, paste_to_target};
use storage::Storage;
use tray::create_tray_icon;

slint::include_modules!();

fn item_to_history_item(item: &ClipboardItem, data_dir: &std::path::Path) -> HistoryItem {
    let thumbnail = item.blob_path.as_ref().and_then(|blob_path| {
        if item.kind != ClipboardKind::Image {
            return None;
        }
        let full_blob = data_dir.join(blob_path);
        let file_stem = full_blob.file_stem()?.to_string_lossy();
        let thumb_path = full_blob.with_file_name(format!("{file_stem}.thumb.png"));
        slint::Image::load_from_path(&thumb_path).ok()
    });
    let has_thumbnail = thumbnail.is_some();

    HistoryItem {
        text: SharedString::from(item.preview.as_str()),
        note: SharedString::from(item.note.as_ref().map(|s| s.as_str()).unwrap_or("")),
        thumbnail: thumbnail.unwrap_or_default(),
        has_thumbnail,
    }
}

fn set_history(
    history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
    items: Vec<ClipboardItem>,
) {
    *history.borrow_mut() = items;
    refresh_history_model(history, history_model);
}

fn refresh_history_model(
    history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
) {
    let data_dir = storage::data_dir();
    history_model.set_vec(
        history
            .borrow()
            .iter()
            .map(|item| item_to_history_item(item, &data_dir))
            .collect::<Vec<_>>(),
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let app = MainWindow::new()?;
    app.window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);

    let storage = Rc::new(Storage::open()?);
    let (clipboard_sender, clipboard_receiver) = mpsc::channel();
    start_clipboard_watcher(clipboard_sender);

    let history_model = Rc::new(VecModel::from(Vec::<HistoryItem>::new()));
    let history = Rc::new(RefCell::new(Vec::<ClipboardItem>::new()));
    let visible_history = Rc::new(RefCell::new(Vec::<ClipboardItem>::new()));
    let ignore_next_clipboard_change = Rc::new(Cell::new(false));
    let drag_state = Rc::new(Cell::new(None::<DragState>));
    let paste_target = Rc::new(Cell::new(None));
    let selected_category = Rc::new(RefCell::new(String::from("all")));
    let selected_index = Rc::new(Cell::new(0));
    app.set_history(ModelRc::from(history_model.clone()));
    app.set_search_text(String::new().into());
    app.set_selected_category("all".into());
    app.set_selected_index(0);
    if let Ok(items) = storage.recent_items(MAX_HISTORY) {
        set_history(&history, &history_model, items.clone());
        *visible_history.borrow_mut() = items;
    }

    let app_weak = app.as_weak();
    let history_for_search = history.clone();
    let visible_history_for_search = visible_history.clone();
    let history_model_for_search = history_model.clone();
    let category_for_search = selected_category.clone();
    let selected_index_for_search = selected_index.clone();
    app.on_search_changed(move |query| {
        apply_filter(
            &history_for_search,
            &visible_history_for_search,
            &history_model_for_search,
            query.as_str(),
            category_for_search.borrow().as_str(),
        );
        selected_index_for_search.set(0);
        if let Some(app) = app_weak.upgrade() {
            app.set_selected_index(0);
        }
    });

    let app_weak = app.as_weak();
    let history_for_category = history.clone();
    let visible_history_for_category = visible_history.clone();
    let history_model_for_category = history_model.clone();
    let selected_category_for_category = selected_category.clone();
    let selected_index_for_category = selected_index.clone();
    app.on_category_changed(move |category| {
        *selected_category_for_category.borrow_mut() = category.to_string();
        let query = app_weak
            .upgrade()
            .map(|app| app.get_search_text().to_string())
            .unwrap_or_default();
        apply_filter(
            &history_for_category,
            &visible_history_for_category,
            &history_model_for_category,
            query.as_str(),
            selected_category_for_category.borrow().as_str(),
        );
        selected_index_for_category.set(0);
        if let Some(app) = app_weak.upgrade() {
            app.set_selected_index(0);
        }
    });

    let selected_index_for_selection = selected_index.clone();
    app.on_selection_changed(move |index| {
        selected_index_for_selection.set(index);
    });

    let app_weak = app.as_weak();
    let visible_history_for_confirm = visible_history.clone();
    let all_history_for_confirm = history.clone();
    let history_model_for_confirm = history_model.clone();
    let selected_category_for_confirm = selected_category.clone();
    let selected_index_for_confirm = selected_index.clone();
    let ignore_for_confirm = ignore_next_clipboard_change.clone();
    let paste_target_for_confirm = paste_target.clone();
    let storage_for_confirm = storage.clone();
    app.on_confirm_selection(move || {
        let Some(item) = visible_history_for_confirm.borrow().get(selected_index_for_confirm.get() as usize).cloned() else {
            return;
        };

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_confirm.set(true);
            if let Some(id) = item.id {
                let _ = storage_for_confirm.mark_used(id, now_millis());
            }
            if let Ok(items) = storage_for_confirm.recent_items(MAX_HISTORY) {
                *all_history_for_confirm.borrow_mut() = items;
                let query = app_weak
                    .upgrade()
                    .map(|app| app.get_search_text().to_string())
                    .unwrap_or_default();
                apply_filter(
                    &all_history_for_confirm,
                    &visible_history_for_confirm,
                    &history_model_for_confirm,
                    query.as_str(),
                    selected_category_for_confirm.borrow().as_str(),
                );
                if let Some(app) = app_weak.upgrade() {
                    app.set_search_text(String::new().into());
                    app.set_selected_category("all".into());
                    *selected_category_for_confirm.borrow_mut() = "all".to_string();
                    app.set_selected_index(0);
                    selected_index_for_confirm.set(0);
                    apply_filter(
                        &all_history_for_confirm,
                        &visible_history_for_confirm,
                        &history_model_for_confirm,
                        "",
                        "all",
                    );
                    let _ = app.hide();
                }
            } else if let Some(app) = app_weak.upgrade() {
                let _ = app.hide();
            }
            paste_to_target(paste_target_for_confirm.get());
        }
    });

    let app_weak = app.as_weak();
    let visible_history_for_note = visible_history.clone();
    let all_history_for_note = history.clone();
    let history_model_for_note = history_model.clone();
    let selected_category_for_note = selected_category.clone();
    let storage_for_note = storage.clone();
    app.on_edit_note_confirm(move |visible_index, note_text| {
        let note = if note_text.is_empty() {
            None
        } else {
            Some(note_text.to_string())
        };
        let Some(item) = visible_history_for_note.borrow().get(visible_index as usize).cloned() else {
            return;
        };
        if let Some(id) = item.id {
            if storage_for_note.update_note(id, note).is_ok() {
                if let Ok(items) = storage_for_note.recent_items(MAX_HISTORY) {
                    *all_history_for_note.borrow_mut() = items;
                    let query = app_weak
                        .upgrade()
                        .map(|app| app.get_search_text().to_string())
                        .unwrap_or_default();
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
            app.set_edit_note_index(-1);
        }
    });

    let app_weak = app.as_weak();
    app.on_edit_note_cancel(move || {
        if let Some(app) = app_weak.upgrade() {
            app.set_edit_note_index(-1);
        }
    });

    let app_weak = app.as_weak();
    app.on_hide_requested(move || {
        if let Some(app) = app_weak.upgrade() {
            let _ = app.hide();
        }
    });

    let app_weak = app.as_weak();
    let history_for_select = visible_history.clone();
    let all_history_for_select = history.clone();
    let visible_history_for_select = visible_history.clone();
    let history_model_for_select = history_model.clone();
    let selected_category_for_select = selected_category.clone();
    let ignore_for_select = ignore_next_clipboard_change.clone();
    let paste_target_for_select = paste_target.clone();
    let storage_for_select = storage.clone();
    app.on_item_selected(move |index| {
        let Some(item) = history_for_select.borrow().get(index as usize).cloned() else {
            return;
        };

        if restore_clipboard_item(&item).is_ok() {
            ignore_for_select.set(true);
            if let Some(id) = item.id {
                let _ = storage_for_select.mark_used(id, now_millis());
            }
            if let Ok(items) = storage_for_select.recent_items(MAX_HISTORY) {
                set_history(
                    &all_history_for_select,
                    &history_model_for_select,
                    items.clone(),
                );
                *visible_history_for_select.borrow_mut() = items;
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
                    let _ = app.hide();
                }
            } else if let Some(app) = app_weak.upgrade() {
                let _ = app.hide();
            }
            paste_to_target(paste_target_for_select.get());
        }
    });

    let history_for_delete = visible_history.clone();
    let history_model_for_delete = history_model.clone();
    let storage_for_delete = storage.clone();
    let all_history_for_delete = history.clone();
    let visible_history_for_delete = visible_history.clone();
    let selected_category_for_delete = selected_category.clone();
    let app_weak = app.as_weak();
    app.on_item_deleted(move |index| {
        let Some(item) = history_for_delete.borrow().get(index as usize).cloned() else {
            return;
        };
        if let Some(id) = item.id {
            if storage_for_delete.delete_item(id).is_ok() {
                if let Ok(items) = storage_for_delete.recent_items(MAX_HISTORY) {
                    *all_history_for_delete.borrow_mut() = items;
                    let query = app_weak
                        .upgrade()
                        .map(|app| app.get_search_text().to_string())
                        .unwrap_or_default();
                    apply_filter(
                        &all_history_for_delete,
                        &visible_history_for_delete,
                        &history_model_for_delete,
                        query.as_str(),
                        selected_category_for_delete.borrow().as_str(),
                    );
                }
            }
        }
    });

    let app_weak = app.as_weak();
    let drag_state_for_start = drag_state.clone();
    app.on_drag_started(move || {
        if let (Some(app), Some((cursor_x, cursor_y))) = (app_weak.upgrade(), cursor_position()) {
            let position = app.window().position();
            drag_state_for_start.set(Some(DragState {
                window_x: position.x,
                window_y: position.y,
                cursor_x,
                cursor_y,
            }));
        }
    });

    let app_weak = app.as_weak();
    let drag_state_for_move = drag_state.clone();
    app.on_dragged(move || {
        let (Some(state), Some((cursor_x, cursor_y))) =
            (drag_state_for_move.get(), cursor_position())
        else {
            return;
        };
        if let Some(app) = app_weak.upgrade() {
            app.window().set_position(PhysicalPosition::new(
                state.window_x + cursor_x - state.cursor_x,
                state.window_y + cursor_y - state.cursor_y,
            ));
        }
    });

    let exit_item = MenuItem::with_id("exit", "退出", true, None);
    let menu = Menu::new();
    menu.append(&exit_item)?;

    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("CCopy")
        .with_icon(create_tray_icon()?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let hotkey_manager = GlobalHotKeyManager::new()?;
    let toggle_hotkey = HotKey::new(Some(Modifiers::ALT), Code::KeyV);
    hotkey_manager.register(toggle_hotkey)?;

    let app_weak = app.as_weak();
    let history_for_timer = history.clone();
    let visible_history_for_timer = visible_history.clone();
    let history_model_for_timer = history_model.clone();
    let storage_for_timer = storage.clone();
    let selected_category = selected_category.clone();
    let selected_index_for_timer = selected_index.clone();
    let mut last_shown = None;
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        if let Some(app) = app_weak.upgrade() {
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if matches!(
                    event,
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                ) {
                    reset_panel_state(
                        &history_for_timer,
                        &visible_history_for_timer,
                        &history_model_for_timer,
                        &selected_category,
                        &selected_index_for_timer,
                        &storage_for_timer,
                    );
                    open_panel_for_target(&app, &paste_target);
                    last_shown = Some(Instant::now());
                }
            }

            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.id == toggle_hotkey.id() && event.state == HotKeyState::Pressed {
                    reset_panel_state(
                        &history_for_timer,
                        &visible_history_for_timer,
                        &history_model_for_timer,
                        &selected_category,
                        &selected_index_for_timer,
                        &storage_for_timer,
                    );
                    open_panel_for_target(&app, &paste_target);
                    last_shown = Some(Instant::now());
                }
            }

            while let Ok(item) = clipboard_receiver.try_recv() {
                if ignore_next_clipboard_change.replace(false) {
                    continue;
                }
                if storage_for_timer.upsert_item(&item).is_ok() {
                    let _ = storage_for_timer.compact_text_duplicates();
                    if let Ok(items) = storage_for_timer.recent_items(MAX_HISTORY) {
                        set_history(&history_for_timer, &history_model_for_timer, items.clone());
                        *visible_history_for_timer.borrow_mut() = items;
                        apply_filter(
                            &history_for_timer,
                            &visible_history_for_timer,
                            &history_model_for_timer,
                            app.get_search_text().as_str(),
                            selected_category.borrow().as_str(),
                        );
                    } else {
                        refresh_history_model(&history_for_timer, &history_model_for_timer);
                    }
                }
            }

            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == *exit_item.id() {
                    let _ = slint::quit_event_loop();
                }
            }

            if app.window().is_visible()
                && last_shown.is_some_and(|shown| shown.elapsed() > Duration::from_millis(250))
                && !is_app_foreground(&app)
            {
                // let _ = app.hide();
            }
        }
    });

    let _keep_alive = (tray_icon, hotkey_manager, timer, app);
    slint::run_event_loop_until_quit()?;
    Ok(())
}

fn reset_panel_state(
    history: &Rc<RefCell<Vec<ClipboardItem>>>,
    visible_history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
    selected_category: &Rc<RefCell<String>>,
    selected_index: &Rc<Cell<i32>>,
    storage: &Storage,
) {
    *selected_category.borrow_mut() = "all".to_string();
    selected_index.set(0);
    if let Ok(items) = storage.recent_items(MAX_HISTORY) {
        *history.borrow_mut() = items;
        apply_filter(history, visible_history, history_model, "", "all");
    }
}

fn apply_filter(
    all_history: &Rc<RefCell<Vec<ClipboardItem>>>,
    visible_history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
    query: &str,
    category: &str,
) {
    let query = query.trim().to_lowercase();
    let all_history = all_history.borrow();
    let filtered: Vec<ClipboardItem> = all_history
        .iter()
        .filter(|item| {
            let matches_category = match category {
                "all" => true,
                "text" => matches!(item.kind, ClipboardKind::Text | ClipboardKind::Html | ClipboardKind::Rtf),
                "image" => matches!(item.kind, ClipboardKind::Image),
                "files" => matches!(item.kind, ClipboardKind::Files),
                _ => true,
            };
            if !matches_category {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            item.preview.to_lowercase().contains(&query)
                || item
                    .plain_text
                    .as_ref()
                    .is_some_and(|t| t.to_lowercase().contains(&query))
                || item
                    .files
                    .iter()
                    .any(|f| f.path.to_lowercase().contains(&query))
                || item
                    .note
                    .as_ref()
                    .is_some_and(|n| n.to_lowercase().contains(&query))
        })
        .cloned()
        .collect::<Vec<_>>();
    let data_dir = storage::data_dir();
    history_model.set_vec(
        filtered
            .iter()
            .map(|item| item_to_history_item(item, &data_dir))
            .collect::<Vec<_>>(),
    );
    *visible_history.borrow_mut() = filtered;
}

fn restore_clipboard_item(item: &ClipboardItem) -> clipboard_rs::Result<()> {
    let ctx = ClipboardContext::new()?;
    let mut contents = Vec::new();
    match item.kind {
        ClipboardKind::Text => {
            if let Some(text) = &item.text_content {
                contents.push(clipboard_rs::common::ClipboardContent::Text(text.clone()));
            }
        }
        ClipboardKind::Html => {
            if let Some(html) = &item.text_content {
                contents.push(clipboard_rs::common::ClipboardContent::Html(html.clone()));
            }
            if let Some(text) = &item.plain_text {
                contents.push(clipboard_rs::common::ClipboardContent::Text(text.clone()));
            }
        }
        ClipboardKind::Rtf => {
            if let Some(rtf) = &item.text_content {
                contents.push(clipboard_rs::common::ClipboardContent::Rtf(rtf.clone()));
            }
            if let Some(text) = &item.plain_text {
                contents.push(clipboard_rs::common::ClipboardContent::Text(text.clone()));
            }
        }
        ClipboardKind::Image => {
            if let Some(blob_path) = &item.blob_path {
                let data_dir = storage::data_dir();
                let full_path = data_dir.join(blob_path);
                if let Ok(image) = clipboard_rs::common::RustImageData::from_path(
                    full_path.to_string_lossy().as_ref(),
                ) {
                    contents.push(clipboard_rs::common::ClipboardContent::Image(image));
                }
            }
        }
        ClipboardKind::Files => {
            let paths: Vec<String> = item.files.iter().map(|f| f.path.clone()).collect();
            if !paths.is_empty() {
                contents.push(clipboard_rs::common::ClipboardContent::Files(paths));
            }
        }
        ClipboardKind::Other => {}
    }
    if !contents.is_empty() {
        ctx.set(contents)?;
    }
    Ok(())
}
