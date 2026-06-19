use std::path::Path;
use std::rc::Rc;

use slint::{SharedString, VecModel};

use crate::clipboard_item::ClipboardItem;
use crate::ui::HistoryItem;

/// 把单条 ClipboardItem 转成 Slint HistoryItem，图片类加载缩略图
pub fn item_to_history_item(item: &ClipboardItem, data_dir: &Path) -> HistoryItem {
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

/// 用查询结果替换整个 model（重置分页、切换分类/搜索时调用）
pub fn fill_model(history_model: &Rc<VecModel<HistoryItem>>, items: &[ClipboardItem]) {
    let data_dir = crate::storage::data_dir();
    history_model.set_vec(
        items
            .iter()
            .map(|item| item_to_history_item(item, &data_dir))
            .collect::<Vec<_>>(),
    );
}

/// 追加查询结果到 model 末尾（分页加载更多时调用）
pub fn append_model(history_model: &Rc<VecModel<HistoryItem>>, items: &[ClipboardItem]) {
    if items.is_empty() {
        return;
    }
    let data_dir = crate::storage::data_dir();
    for item in items {
        history_model.push(item_to_history_item(item, &data_dir));
    }
}
