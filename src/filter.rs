use std::path::{Path, PathBuf};
use std::rc::Rc;

use clipboard_rs::common::{RustImage, RustImageData};
use slint::{SharedString, VecModel};

use crate::clipboard_item::{ClipboardItem, ClipboardKind};
use crate::ui::HistoryItem;

/// 缩略图最大边长上限：原图最大边超过此值才缩放，否则保留原尺寸避免放大失真
const THUMB_MAX: u32 = 300;

/// 计算缩略图文件路径：与源文件同目录，命名为 `{stem}.thumb.png`
pub fn thumbnail_path(data_dir: &Path, blob_path: &str) -> Option<PathBuf> {
    let full_blob = data_dir.join(blob_path);
    let stem = full_blob.file_stem()?.to_str()?;
    Some(full_blob.with_file_name(format!("{stem}.thumb.png")))
}

/// 生成缩略图：原图最大边 > THUMB_MAX 才缩放，小图保留原尺寸，杜绝放大
pub fn make_thumbnail(img: RustImageData) -> Option<RustImageData> {
    let (w, h) = img.get_size();
    let max_edge = w.max(h) as u32;
    // 小图直接返回原图，不放大
    if max_edge <= THUMB_MAX {
        return Some(img);
    }
    img.thumbnail(THUMB_MAX, THUMB_MAX).ok()
}

/// 确保缩略图合格：缺失或尺寸不匹配（旧版放大/缩小错误）则重新生成。后台线程调用
pub fn ensure_thumbnail(data_dir: &Path, blob_path: &str) {
    let Some(thumb_path) = thumbnail_path(data_dir, blob_path) else {
        return;
    };
    let full_blob = data_dir.join(blob_path);
    let Ok(src) = RustImageData::from_path(full_blob.to_string_lossy().as_ref()) else {
        return;
    };
    let (sw, sh) = src.get_size();
    let src_max = sw.max(sh) as u32;
    // 合格缩略图的最大边应为 min(原图最大边, THUMB_MAX)
    let expected = src_max.min(THUMB_MAX);
    let need_regen = match RustImageData::from_path(thumb_path.to_string_lossy().as_ref()) {
        Ok(thumb) => {
            let (tw, th) = thumb.get_size();
            (tw.max(th) as u32) != expected
        }
        Err(_) => true,
    };
    if !need_regen {
        return;
    }
    let Some(thumb) = make_thumbnail(src) else {
        return;
    };
    let _ = thumb.save_to_path(thumb_path.to_string_lossy().as_ref());
}

/// 构造占位 HistoryItem（缩略图留空，后台异步填入），并返回需要加载缩略图的任务
fn item_to_history_item(item: &ClipboardItem) -> (HistoryItem, Option<(i32, String)>) {
    let is_image = item.kind == ClipboardKind::Image;
    // 图片项才有缩略图加载任务：携带 id 与 blob_path 交给后台
    let load_task = if is_image {
        item.blob_path
            .as_ref()
            .map(|p| (item.id.unwrap_or(-1) as i32, p.clone()))
    } else {
        None
    };
    // 类型标签文本与对应前缀：文件/图片从 preview 剥离前缀，文本类不显示标签
    let (tag_text, display_text) = match item.kind {
        ClipboardKind::Files => {
            let prefix = "[文件] ";
            let txt = item.preview.strip_prefix(prefix).unwrap_or(&item.preview).to_string();
            ("文件".to_string(), txt)
        }
        ClipboardKind::Image => {
            let prefix = "[图片] ";
            let txt = item.preview.strip_prefix(prefix).unwrap_or(&item.preview).to_string();
            ("图片".to_string(), txt)
        }
        _ => (String::new(), item.preview.clone()),
    };
    let hi = HistoryItem {
        id: item.id.unwrap_or(-1) as i32,
        text: SharedString::from(display_text),
        note: SharedString::from(item.note.as_ref().map(|s| s.as_str()).unwrap_or("")),
        // 缩略图初始为空，has_thumbnail 按 kind 预先占位保证布局高度不跳变
        thumbnail: slint::Image::default(),
        has_thumbnail: is_image && item.blob_path.is_some(),
        blob_path: SharedString::from(item.blob_path.as_deref().unwrap_or("")),
        tag_text: SharedString::from(tag_text),
    };
    (hi, load_task)
}

/// 用查询结果替换整个 model（重置分页、切换分类/搜索时调用）
/// 返回需要异步加载缩略图的 (id, blob_path) 列表
pub fn fill_model(
    history_model: &Rc<VecModel<HistoryItem>>,
    items: &[ClipboardItem],
) -> Vec<(i32, String)> {
    let mut pending = Vec::new();
    history_model.set_vec(
        items
            .iter()
            .map(|item| {
                let (hi, task) = item_to_history_item(item);
                if let Some(t) = task {
                    pending.push(t);
                }
                hi
            })
            .collect::<Vec<_>>(),
    );
    pending
}

/// 追加查询结果到 model 末尾（分页加载更多时调用）
/// 返回需要异步加载缩略图的 (id, blob_path) 列表
pub fn append_model(
    history_model: &Rc<VecModel<HistoryItem>>,
    items: &[ClipboardItem],
) -> Vec<(i32, String)> {
    let mut pending = Vec::new();
    if items.is_empty() {
        return pending;
    }
    for item in items {
        let (hi, task) = item_to_history_item(item);
        if let Some(t) = task {
            pending.push(t);
        }
        history_model.push(hi);
    }
    pending
}
