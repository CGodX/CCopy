//! 同步事件：全量快照事件定义与 JSONL 序列化
//! 每条事件自包含完整字段，合并时按 id 分组取 updated_at 最大的快照

use serde::{Deserialize, Serialize};

use crate::clipboard_item::{ClipboardItem, ClipboardKind};

/// 同步事件：upsert 为全量快照，delete 仅需 id
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum SyncEvent {
    #[serde(rename = "upsert")]
    Upsert(SyncItem),
    #[serde(rename = "delete")]
    Delete(SyncDelete),
}

/// 全量快照记录：对应本地 ClipboardItem 的可同步字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncItem {
    pub id: String,
    pub source_device: String,
    pub kind: String,
    pub preview: String,
    pub text_content: Option<String>,
    pub plain_text: Option<String>,
    pub blob_path: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 删除事件：仅需 id 与时间戳
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDelete {
    pub id: String,
    pub source_device: String,
    pub updated_at: i64,
}

impl SyncEvent {
    /// 事件对应的记录 id
    pub fn id(&self) -> &str {
        match self {
            SyncEvent::Upsert(item) => &item.id,
            SyncEvent::Delete(d) => &d.id,
        }
    }

    /// 事件的时间戳（用于 LWW 合并）
    pub fn updated_at(&self) -> i64 {
        match self {
            SyncEvent::Upsert(item) => item.updated_at,
            SyncEvent::Delete(d) => d.updated_at,
        }
    }

    /// 从本地 ClipboardItem 构造 upsert 全量快照
    pub fn from_item(item: &ClipboardItem, device: &str) -> Self {
        let kind = match item.kind {
            ClipboardKind::Text => "text",
            ClipboardKind::Html => "html",
            ClipboardKind::Rtf => "rtf",
            ClipboardKind::Image => "image",
            ClipboardKind::Files => "files",
            ClipboardKind::Other => "other",
        };
        SyncEvent::Upsert(SyncItem {
            id: item.hash.clone(),
            source_device: device.to_string(),
            kind: kind.to_string(),
            preview: item.preview.clone(),
            text_content: item.text_content.clone(),
            plain_text: item.plain_text.clone(),
            blob_path: item.blob_path.clone(),
            note: item.note.clone(),
            created_at: item.created_at,
            updated_at: item.updated_at,
        })
    }

    /// 构造删除事件
    pub fn delete(hash: &str, device: &str, updated_at: i64) -> Self {
        SyncEvent::Delete(SyncDelete {
            id: hash.to_string(),
            source_device: device.to_string(),
            updated_at,
        })
    }

    /// 序列化为 JSONL 单行
    pub fn to_jsonl(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("序列化失败: {e}"))
    }

    /// 从 JSONL 单行解析
    pub fn from_jsonl(line: &str) -> Result<Self, String> {
        let line = line.trim();
        if line.is_empty() {
            return Err("空行".to_string());
        }
        serde_json::from_str(line).map_err(|e| format!("解析失败: {e}"))
    }
}

/// 把 upsert 事件转回 ClipboardItem（用于落库）
impl SyncItem {
    pub fn to_clipboard_item(&self) -> ClipboardItem {
        let kind = ClipboardKind::from_str(&self.kind).unwrap_or(ClipboardKind::Other);
        ClipboardItem {
            id: None,
            kind,
            preview: self.preview.clone(),
            text_content: self.text_content.clone(),
            plain_text: self.plain_text.clone(),
            blob_path: self.blob_path.clone(),
            format_name: None,
            mime_type: None,
            width: None,
            height: None,
            size_bytes: None,
            hash: self.id.clone(),
            note: self.note.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_used_at: None,
            files: Vec::new(),
            tags: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_roundtrip() {
        let item = SyncItem {
            id: "abc".to_string(),
            source_device: "pc1".to_string(),
            kind: "text".to_string(),
            preview: "hi".to_string(),
            text_content: Some("hello".to_string()),
            plain_text: Some("hello".to_string()),
            blob_path: None,
            note: Some("备注".to_string()),
            created_at: 1000,
            updated_at: 2000,
        };
        let event = SyncEvent::Upsert(item);
        let line = event.to_jsonl().unwrap();
        let parsed = SyncEvent::from_jsonl(&line).unwrap();
        assert_eq!(parsed.id(), "abc");
        assert_eq!(parsed.updated_at(), 2000);
    }

    #[test]
    fn delete_event_roundtrip() {
        let event = SyncEvent::delete("xyz", "pc2", 3000);
        let line = event.to_jsonl().unwrap();
        let parsed = SyncEvent::from_jsonl(&line).unwrap();
        assert_eq!(parsed.id(), "xyz");
        assert_eq!(parsed.updated_at(), 3000);
    }
}
