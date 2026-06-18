//! 通用工具函数

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub fn short_text(text: &str) -> String {
    let mut chars = text.chars().take(300).collect::<String>();
    if text.len() > 300 {
        chars.push_str("...");
    }
    chars
}

pub fn hash_bytes(kind: &str, bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    bytes.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub fn hash_content(kind: &str, content: &str) -> String {
    hash_bytes(kind, content.as_bytes())
}
