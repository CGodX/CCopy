#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardKind {
    Text,
    Html,
    Rtf,
    Image,
    Files,
    Other,
}

impl ClipboardKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClipboardKind::Text => "text",
            ClipboardKind::Html => "html",
            ClipboardKind::Rtf => "rtf",
            ClipboardKind::Image => "image",
            ClipboardKind::Files => "files",
            ClipboardKind::Other => "other",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "text" => Some(ClipboardKind::Text),
            "html" => Some(ClipboardKind::Html),
            "rtf" => Some(ClipboardKind::Rtf),
            "image" => Some(ClipboardKind::Image),
            "files" => Some(ClipboardKind::Files),
            "other" => Some(ClipboardKind::Other),
            _ => None,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ClipboardItem {
    pub id: Option<i64>,
    pub kind: ClipboardKind,
    pub preview: String,
    pub text_content: Option<String>,
    pub plain_text: Option<String>,
    pub blob_path: Option<String>,
    pub format_name: Option<String>,
    pub mime_type: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: Option<i64>,
    pub hash: String,
    pub note: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
    pub files: Vec<ClipboardFile>,
    pub tags: Vec<Tag>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ClipboardFile {
    pub path: String,
    pub position: i64,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Tag {
    pub id: Option<i64>,
    pub name: String,
    pub color: Option<String>,
    pub created_at: i64,
}

impl ClipboardItem {
    pub fn text(text: String, now: i64, hash: String, preview: String) -> Self {
        Self {
            id: None,
            kind: ClipboardKind::Text,
            preview,
            text_content: Some(text.clone()),
            plain_text: Some(text),
            blob_path: None,
            format_name: None,
            mime_type: Some("text/plain".to_string()),
            width: None,
            height: None,
            size_bytes: None,
            hash,
            note: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            files: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub fn html(
        html: String,
        plain_text: Option<String>,
        now: i64,
        hash: String,
        preview: String,
    ) -> Self {
        Self {
            id: None,
            kind: ClipboardKind::Html,
            preview,
            text_content: Some(html),
            plain_text,
            blob_path: None,
            format_name: None,
            mime_type: Some("text/html".to_string()),
            width: None,
            height: None,
            size_bytes: None,
            hash,
            note: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            files: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub fn rtf(
        rtf: String,
        plain_text: Option<String>,
        now: i64,
        hash: String,
        preview: String,
    ) -> Self {
        Self {
            id: None,
            kind: ClipboardKind::Rtf,
            preview,
            text_content: Some(rtf),
            plain_text,
            blob_path: None,
            format_name: None,
            mime_type: Some("text/rtf".to_string()),
            width: None,
            height: None,
            size_bytes: None,
            hash,
            note: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            files: Vec::new(),
            tags: Vec::new(),
        }
    }
}
