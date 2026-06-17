use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, TimeZone, Utc};
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher,
    ClipboardWatcherContext,
};


use crate::clipboard_item::ClipboardItem;
use crate::storage::data_dir;

pub const MAX_HISTORY: usize = 20;

struct ClipboardChangeHandler {
    ctx: ClipboardContext,
    sender: mpsc::Sender<ClipboardItem>,
}

impl ClipboardChangeHandler {
    fn new(sender: mpsc::Sender<ClipboardItem>) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Ok(Self {
            ctx: ClipboardContext::new()?,
            sender,
        })
    }
}

impl ClipboardHandler for ClipboardChangeHandler {
    fn on_clipboard_change(&mut self) {
        if let Some(item) = read_clipboard_item(&self.ctx) {
            let _ = self.sender.send(item);
        }
    }
}

pub fn start_clipboard_watcher(sender: mpsc::Sender<ClipboardItem>) {
    thread::spawn(move || {
        let Ok(handler) = ClipboardChangeHandler::new(sender) else {
            return;
        };
        let Ok(mut watcher) = ClipboardWatcherContext::new() else {
            return;
        };
        watcher.add_handler(handler);
        watcher.start_watch();
    });
}

pub fn short_text(text: &str) -> String {
    // let text = text.replace(['\r', '\n', '\t'], " ");
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(300).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

fn read_clipboard_item(ctx: &ClipboardContext) -> Option<ClipboardItem> {
    if let Ok(files) = ctx.get_files() {
        if files.is_empty() {
            return None;
        }
        let now = now_millis();
        let preview = match files.len() {
            1 => {
                let path = std::path::Path::new(files[0].as_str());
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_else(|| files[0].clone().into());
                format!("[文件] {}", name)
            }
            n => format!("[文件] {} 个文件", n),
        };
        let hash = hash_content("files", &files.join("\n"));
        let item = ClipboardItem {
            id: None,
            kind: crate::clipboard_item::ClipboardKind::Files,
            preview,
            text_content: None,
            plain_text: None,
            blob_path: None,
            format_name: None,
            mime_type: None,
            width: None,
            height: None,
            size_bytes: None,
            hash,
            note: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            files: files
                .into_iter()
                .enumerate()
                .map(|(i, path)| crate::clipboard_item::ClipboardFile {
                    path,
                    position: i as i64,
                })
                .collect(),
            tags: Vec::new(),
        };
        return Some(item);
    }

    if let Ok(image) = ctx.get_image() {
        if image.is_empty() {
            return None;
        }
        let (width, height) = image.get_size();
        let now = now_millis();

        let timestamp_millis = now;
        let secs = (timestamp_millis / 1000) as i64;
        let datetime = Utc.timestamp_opt(secs, 0).single()?;
        let year = datetime.year();
        let month = datetime.month();
        let png = image.to_png().ok()?;
        let png_bytes = png.get_bytes();
        let hash = hash_bytes("image", png_bytes);
        let blob_path = format!("blobs/image/{}/{}/{}.png", year, month, hash);
        let preview = format!("[图片] {}×{}", width, height);

        let data_dir = data_dir();
        let full_path = data_dir.join(&blob_path);
        if let Some(parent) = full_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = png.save_to_path(full_path.to_string_lossy().as_ref());
        let thumb_path = full_path.with_file_name(format!("{}.thumb.png", hash));
        if !thumb_path.exists() {
            if let Ok(thumbnail) = image.thumbnail(66, 66) {
                let _ = thumbnail.save_to_path(thumb_path.to_string_lossy().as_ref());
            }
        }

        let size_bytes = Some(png_bytes.len() as i64);

        return Some(ClipboardItem {
            id: None,
            kind: crate::clipboard_item::ClipboardKind::Image,
            preview,
            text_content: None,
            plain_text: None,
            blob_path: Some(blob_path),
            format_name: None,
            mime_type: Some("image/png".to_string()),
            width: Some(width as i64),
            height: Some(height as i64),
            size_bytes,
            hash,
            note: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            files: Vec::new(),
            tags: Vec::new(),
        });
    }

    if let Ok(html) = ctx.get_html() {
        let html = html.trim().to_string();
        if !html.is_empty() {
            let plain_text = ctx
                .get_text()
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
            let preview = plain_text
                .as_deref()
                .map(short_text)
                .unwrap_or_else(|| short_text(&html));
            let now = now_millis();
            return Some(ClipboardItem::html(
                html.clone(),
                plain_text,
                now,
                hash_content("html", &html),
                preview,
            ));
        }
    }

    if let Ok(rtf) = ctx.get_rich_text() {
        let rtf = rtf.trim().to_string();
        if !rtf.is_empty() {
            let plain_text = ctx
                .get_text()
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
            let preview = plain_text
                .as_deref()
                .map(short_text)
                .unwrap_or_else(|| short_text(&rtf));
            let now = now_millis();
            return Some(ClipboardItem::rtf(
                rtf.clone(),
                plain_text,
                now,
                hash_content("rtf", &rtf),
                preview,
            ));
        }
    }

    if let Ok(text) = ctx.get_text() {
        let text = text.trim().to_string();
        if !text.is_empty() {
            let now = now_millis();
            return Some(ClipboardItem::text(
                text.clone(),
                now,
                hash_content("text", &text),
                short_text(&text),
            ));
        }
    }

    None
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn hash_content(kind: &str, content: &str) -> String {
    hash_bytes(kind, content.as_bytes())
}

fn hash_bytes(kind: &str, bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
