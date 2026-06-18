use std::cell::RefCell;
use std::rc::Rc;

use slint::{SharedString, VecModel};

use crate::clipboard_item::ClipboardItem;
use crate::ui::HistoryItem;

pub fn item_to_history_item(item: &ClipboardItem, data_dir: &std::path::Path) -> HistoryItem {
    let thumbnail = item.blob_path.as_ref().and_then(|blob_path| {
        if item.kind != crate::clipboard_item::ClipboardKind::Image {
            return None;
        }
        let full_blob = data_dir.join(blob_path);
        let file_stem = full_blob.file_stem()?.to_string_lossy();
        let thumb_path = full_blob.with_file_name(format!("{file_stem}.thumb.png"));
        slint::Image::load_from_path(&thumb_path).ok()
    });
    let has_thumbnail = thumbnail.is_some();

    HistoryItem {
        id: item.id.unwrap_or(-1) as i32,
        text: SharedString::from(item.preview.as_str()),
        note: SharedString::from(item.note.as_ref().map(|s| s.as_str()).unwrap_or("")),
        thumbnail: thumbnail.unwrap_or_default(),
        has_thumbnail,
    }
}

pub fn set_history(
    history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
    items: Vec<ClipboardItem>,
) {
    *history.borrow_mut() = items;
    refresh_history_model(history, history_model);
}

pub fn refresh_history_model(
    history: &Rc<RefCell<Vec<ClipboardItem>>>,
    history_model: &Rc<VecModel<HistoryItem>>,
) {
    let data_dir = crate::storage::data_dir();
    history_model.set_vec(
        history
            .borrow()
            .iter()
            .map(|item| item_to_history_item(item, &data_dir))
            .collect::<Vec<_>>(),
    );
}

pub fn apply_filter(
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
                "text" => matches!(
                    item.kind,
                    crate::clipboard_item::ClipboardKind::Text
                        | crate::clipboard_item::ClipboardKind::Html
                        | crate::clipboard_item::ClipboardKind::Rtf
                ),
                "image" => matches!(item.kind, crate::clipboard_item::ClipboardKind::Image),
                "files" => matches!(item.kind, crate::clipboard_item::ClipboardKind::Files),
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
                    .note
                    .as_ref()
                    .is_some_and(|n| n.to_lowercase().contains(&query))
        })
        .cloned()
        .collect::<Vec<_>>();
    let data_dir = crate::storage::data_dir();
    history_model.set_vec(
        filtered
            .iter()
            .map(|item| item_to_history_item(item, &data_dir))
            .collect::<Vec<_>>(),
    );
    *visible_history.borrow_mut() = filtered;
}
