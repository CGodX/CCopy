use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::clipboard_item::{ClipboardItem, ClipboardKind};

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open() -> Result<Self, Box<dyn Error>> {
        let data_dir = data_dir();
        fs::create_dir_all(data_dir.join("blobs").join("image"))?;
        fs::create_dir_all(data_dir.join("blobs").join("other"))?;

        let conn = Connection::open(data_dir.join("ccopy.db"))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let storage = Self { conn };
        storage.init()?;
        storage.compact_text_duplicates()?;
        Ok(storage)
    }

    fn init(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL,
                text_content TEXT,
                plain_text TEXT,
                blob_path TEXT,
                format_name TEXT,
                mime_type TEXT,
                width INTEGER,
                height INTEGER,
                size_bytes INTEGER,
                hash TEXT NOT NULL,
                note TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_used_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS clipboard_item_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                position INTEGER NOT NULL,
                FOREIGN KEY(item_id) REFERENCES clipboard_items(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                color TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS clipboard_item_tags (
                item_id INTEGER NOT NULL,
                tag_id INTEGER NOT NULL,
                PRIMARY KEY (item_id, tag_id),
                FOREIGN KEY(item_id) REFERENCES clipboard_items(id) ON DELETE CASCADE,
                FOREIGN KEY(tag_id) REFERENCES tags(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_clipboard_items_kind_hash
            ON clipboard_items(kind, hash);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_created_at
            ON clipboard_items(created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_updated_at
            ON clipboard_items(updated_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_last_used_at
            ON clipboard_items(last_used_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_kind
            ON clipboard_items(kind);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_preview
            ON clipboard_items(preview);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_plain_text
            ON clipboard_items(plain_text);

            CREATE INDEX IF NOT EXISTS idx_clipboard_item_files_item_id
            ON clipboard_item_files(item_id);

            CREATE INDEX IF NOT EXISTS idx_clipboard_item_files_path
            ON clipboard_item_files(path);

            CREATE INDEX IF NOT EXISTS idx_tags_name
            ON tags(name);
            ",
        )
    }

    pub fn compact_text_duplicates(&self) -> rusqlite::Result<()> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, COALESCE(NULLIF(TRIM(plain_text), ''), NULLIF(TRIM(text_content), '')) AS visible_text
            FROM clipboard_items
            WHERE kind IN ('text', 'html', 'rtf')
            ORDER BY
                CASE kind WHEN 'html' THEN 0 WHEN 'rtf' THEN 1 ELSE 2 END,
                updated_at DESC,
                id DESC
            ",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?;

        let mut seen_text: HashMap<String, Vec<i64>> = HashMap::new();
        for row in rows {
            let (id, visible_text) = row?;
            if let Some(text) = visible_text {
                seen_text.entry(text).or_default().push(id);
            }
        }

        for (_, ids) in seen_text {
            for &id in &ids[1..] {
                self.conn
                    .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;
            }
        }

        Ok(())
    }

    pub fn upsert_item(&self, item: &ClipboardItem) -> rusqlite::Result<i64> {
        let existing_id = self
            .conn
            .query_row(
                "SELECT id FROM clipboard_items WHERE kind = ?1 AND hash = ?2",
                params![item.kind.as_str(), item.hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        if let Some(id) = existing_id {
            self.conn.execute(
                "
                UPDATE clipboard_items
                SET preview = ?1,
                    text_content = ?2,
                    plain_text = ?3,
                    blob_path = ?4,
                    format_name = ?5,
                    mime_type = ?6,
                    width = ?7,
                    height = ?8,
                    size_bytes = ?9,
                    updated_at = ?10
                WHERE id = ?11
                ",
                params![
                    item.preview,
                    item.text_content,
                    item.plain_text,
                    item.blob_path,
                    item.format_name,
                    item.mime_type,
                    item.width,
                    item.height,
                    item.size_bytes,
                    item.updated_at,
                    id
                ],
            )?;
            if !item.files.is_empty() {
                self.conn.execute(
                    "DELETE FROM clipboard_item_files WHERE item_id = ?1",
                    params![id],
                )?;
                for file in &item.files {
                    self.conn.execute(
                        "INSERT INTO clipboard_item_files (item_id, path, position) VALUES (?1, ?2, ?3)",
                        params![id, file.path, file.position],
                    )?;
                }
            }
            return Ok(id);
        }

        self.conn.execute(
            "
            INSERT INTO clipboard_items (
                kind, preview, text_content, plain_text, blob_path, format_name, mime_type,
                width, height, size_bytes, hash, note, created_at, updated_at, last_used_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ",
            params![
                item.kind.as_str(),
                item.preview,
                item.text_content,
                item.plain_text,
                item.blob_path,
                item.format_name,
                item.mime_type,
                item.width,
                item.height,
                item.size_bytes,
                item.hash,
                item.note,
                item.created_at,
                item.updated_at,
                item.last_used_at
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if !item.files.is_empty() {
            for file in &item.files {
                self.conn.execute(
                    "INSERT INTO clipboard_item_files (item_id, path, position) VALUES (?1, ?2, ?3)",
                    params![id, file.path, file.position],
                )?;
            }
        }
        Ok(id)
    }

    /// 分页查询：搜索 + 分类 + 分页一次查库完成。
    /// query 为空表示不搜，category 为 "all" 表示不筛分类。
    /// limit=0 表示不限制条数，offset 为分页偏移。
    pub fn query_items(
        &self,
        query: &str,
        category: &str,
        limit: usize,
        offset: usize,
    ) -> rusqlite::Result<Vec<ClipboardItem>> {
        let q = query.trim();
        // 分类映射到 SQL：text 覆盖 text/html/rtf；marked 复用清理规则的豁免表达式
        let (has_query, has_category) = (!q.is_empty(), !matches!(category, "all" | ""));
        let category_filter = match category {
            "text" => "kind IN ('text','html','rtf')",
            "image" => "kind = 'image'",
            "files" => "kind = 'files'",
            "marked" => Self::protected_expr(),
            _ => "",
        };

        // 动态拼 WHERE 子句，参数用位置占位符 ? 按顺序绑定
        let mut where_clauses: Vec<&str> = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let like_pattern = if has_query {
            Some(format!("%{q}%"))
        } else {
            None
        };
        if has_query {
            where_clauses.push("(preview LIKE ? ESCAPE '\\' OR plain_text LIKE ? ESCAPE '\\' OR COALESCE(note,'') LIKE ? ESCAPE '\\')");
            let like = like_pattern.clone().unwrap();
            params_vec.push(Box::new(like.clone()));
            params_vec.push(Box::new(like.clone()));
            params_vec.push(Box::new(like));
        }
        if has_category {
            where_clauses.push(category_filter);
        }
        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };
        let limit_sql = if limit == 0 {
            "LIMIT -1".to_string()
        } else {
            params_vec.push(Box::new(limit as i64));
            params_vec.push(Box::new(offset as i64));
            "LIMIT ? OFFSET ?".to_string()
        };

        // 搜索时备注匹配优先：备注匹配的条目排前面，其他匹配的排后面
        let order_sql = if has_query {
            let note_like = like_pattern.clone().unwrap();
            params_vec.insert(0, Box::new(note_like));
            "ORDER BY CASE WHEN COALESCE(note,'') LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END, updated_at DESC, id DESC"
        } else {
            "ORDER BY updated_at DESC, id DESC"
        };

        let sql = format!(
            "SELECT id, kind, preview, text_content, plain_text, blob_path, format_name, mime_type,
                   width, height, size_bytes, hash, note, created_at, updated_at, last_used_at
            FROM clipboard_items
            {where_sql}
            {order_sql}
            {limit_sql}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut items = Vec::new();
        let mut rows = stmt.query(params_ref.as_slice())?;
        self.collect_items(&mut rows, &mut items)?;
        Ok(items)
    }

    /// 按主键查询单条记录（含 files/tags），用于分页后按 id 取详情
    pub fn get_item(&self, id: i64) -> rusqlite::Result<Option<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, preview, text_content, plain_text, blob_path, format_name, mime_type,
                   width, height, size_bytes, hash, note, created_at, updated_at, last_used_at
            FROM clipboard_items
            WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        let mut items = Vec::new();
        self.collect_items(&mut rows, &mut items)?;
        Ok(items.into_iter().next())
    }

    /// 从已 prepare 的 rows 收集 ClipboardItem（含 files），复用给多个查询方法
    fn collect_items(
        &self,
        rows: &mut rusqlite::Rows<'_>,
        items: &mut Vec<ClipboardItem>,
    ) -> rusqlite::Result<()> {
        while let Some(row) = rows.next()? {
            let kind_text: String = row.get(1)?;
            let kind = ClipboardKind::from_str(&kind_text).unwrap_or(ClipboardKind::Other);
            let mut item = ClipboardItem {
                id: row.get(0)?,
                kind,
                preview: row.get(2)?,
                text_content: row.get(3)?,
                plain_text: row.get(4)?,
                blob_path: row.get(5)?,
                format_name: row.get(6)?,
                mime_type: row.get(7)?,
                width: row.get(8)?,
                height: row.get(9)?,
                size_bytes: row.get(10)?,
                hash: row.get(11)?,
                note: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                last_used_at: row.get(15)?,
                files: Vec::new(),
                tags: Vec::new(),
            };

            if item.kind == ClipboardKind::Files {
                if let Some(item_id) = item.id {
                    let mut stmt_files = self.conn.prepare(
                        "SELECT path, position FROM clipboard_item_files WHERE item_id = ?1 ORDER BY position",
                    )?;
                    let mut rows_files = stmt_files.query(params![item_id])?;
                    while let Some(row_file) = rows_files.next()? {
                        item.files.push(crate::clipboard_item::ClipboardFile {
                            path: row_file.get(0)?,
                            position: row_file.get(1)?,
                        });
                    }
                }
            }

            items.push(item);
        }
        Ok(())
    }

    pub fn mark_used(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET last_used_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn delete_item(&self, id: i64) -> rusqlite::Result<()> {
        let blob_path = self
            .conn
            .query_row(
                "SELECT blob_path FROM clipboard_items WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();

        self.conn
            .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;

        if let Some(blob_path) = blob_path {
            remove_blob_and_thumb(&blob_path);
        }

        Ok(())
    }

    pub fn update_note(&self, id: i64, note: Option<String>) -> rusqlite::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        self.conn.execute(
            "UPDATE clipboard_items SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now, id],
        )?;
        Ok(())
    }

    /// 读取设置项，返回 None 表示未设置
    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
    }

    /// 写入或更新设置项
    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 清理规则的豁免条件 SQL 片段：满足该条件的记录不会被清理。
    /// 当前为「有备注」，未来扩展置顶/收藏等只需在此追加 OR 分支。
    fn protected_expr() -> &'static str {
        "note IS NOT NULL AND note != ''"
    }

    /// 按规则清理：OR 保留语义。
    /// 非豁免记录满足「排在前 max_count 条内」或「updated_at 在 max_age_days 天内」任一即保留。
    /// max_count=0 表示不靠条数保留；max_age_days=0 表示不靠时间保留。
    /// 两者均为 0 时直接返回（无任何清理）。
    pub fn purge_by_rule(&self, max_count: usize, max_age_days: usize) -> rusqlite::Result<()> {
        if max_count == 0 && max_age_days == 0 {
            return Ok(());
        }

        let protected = Self::protected_expr();
        let now = crate::common::now_millis();
        let age_threshold = now - (max_age_days as i64) * 86_400_000;

        // OR 保留取反 = AND 删除：
        // 删除 = 非豁免 AND (若配条数: 排在 max_count 之外) AND (若配天数: updated_at 早于阈值)
        let mut conditions = vec![format!("NOT ({protected})")];
        if max_count > 0 {
            conditions.push(format!(
                "id NOT IN (SELECT id FROM clipboard_items ORDER BY updated_at DESC, id DESC LIMIT {max_count})"
            ));
        }
        if max_age_days > 0 {
            conditions.push(format!("updated_at < {age_threshold}"));
        }
        let where_clause = conditions.join(" AND ");

        // 取待删记录的 id 和 blob_path
        let to_delete: Vec<(i64, Option<String>)> = {
            let sql = format!("SELECT id, blob_path FROM clipboard_items WHERE {where_clause}");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };

        if to_delete.is_empty() {
            return Ok(());
        }

        // 删除 blob 文件及对应缩略图
        for (_, blob_path) in &to_delete {
            if let Some(p) = blob_path {
                remove_blob_and_thumb(p);
            }
        }

        // 删除数据库记录
        let ids: Vec<i64> = to_delete.iter().map(|(id, _)| *id).collect();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        self.conn.execute(
            &format!("DELETE FROM clipboard_items WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;

        Ok(())
    }

    /// 清空所有未标记（无备注）的记录，保留有备注的记录及其 blob 文件。
    pub fn clear_unnoted(&self) -> rusqlite::Result<()> {
        let protected = Self::protected_expr();
        let to_delete: Vec<Option<String>> = {
            let sql = format!(
                "SELECT blob_path FROM clipboard_items WHERE NOT ({protected})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        for blob_path in &to_delete {
            if let Some(p) = blob_path {
                remove_blob_and_thumb(p);
            }
        }

        let sql = format!("DELETE FROM clipboard_items WHERE NOT ({protected})");
        self.conn.execute(&sql, [])?;
        Ok(())
    }

    /// 统计剪贴板记录：总条数、各分类条数、有备注条数
    /// 返回 (total, marked, text, image, files)
    pub fn stats(&self) -> rusqlite::Result<(i64, i64, i64, i64, i64)> {
        let protected = Self::protected_expr();
        let sql = format!(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN ({protected}) THEN 1 ELSE 0 END),
                SUM(CASE WHEN kind IN ('text','html','rtf') THEN 1 ELSE 0 END),
                SUM(CASE WHEN kind = 'image' THEN 1 ELSE 0 END),
                SUM(CASE WHEN kind = 'files' THEN 1 ELSE 0 END)
            FROM clipboard_items"
        );
        self.conn
            .query_row(&sql, [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            })
    }

    /// 清空所有剪贴板历史记录，同时删除关联的 blob 文件及 blobs 目录下的孤儿文件
    pub fn clear_all(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM clipboard_items", [])?;

        // 删除整个 blobs 目录下的所有文件，确保缓存图片等彻底清理
        let blobs_dir = data_dir().join("blobs");
        if blobs_dir.exists() {
            if let Ok(entries) = fs::read_dir(&blobs_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let _ = fs::remove_dir_all(&path);
                }
            }
            // 重建空的子目录
            let _ = fs::create_dir_all(blobs_dir.join("image"));
            let _ = fs::create_dir_all(blobs_dir.join("other"));
        }

        Ok(())
    }
}

/// 删除 blob 源文件及其对应缩略图。
/// 缩略图与源文件同目录，命名为 `{stem}.thumb.png`。
fn remove_blob_and_thumb(blob_path: &str) {
    let full = data_dir().join(blob_path);
    let _ = fs::remove_file(&full);

    // 构造缩略图路径：去掉扩展名后追加 .thumb.png
    let path = std::path::Path::new(blob_path);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let thumb = path
            .with_file_name(format!("{stem}.thumb.png"));
        let _ = fs::remove_file(data_dir().join(thumb));
    }
}

pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join("CCopy")
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library").join("Application Support").join("CCopy"))
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config").join("CCopy"))
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}
