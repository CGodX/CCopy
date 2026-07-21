//! 同步协调器：管理推送、拉取、定时任务
//! 设计要点：
//! - 耗时网络操作在独立线程执行，不阻塞 UI
//! - 推送：本机变更后追加全量快照事件到 devices/<本机名>/<YYYY-MM>.jsonl
//! - 拉取：PROPFIND 列 devices/ 目录，按 etag 缓存增量下载，合并后落库
//! - 防回环：跳过 source_device == 本机名 的事件

pub mod config;
pub mod event;
pub mod merge;
pub mod webdav;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::clipboard_item::{ClipboardItem, ClipboardKind};
use crate::common::now_millis;
use crate::storage::Storage;
use crate::sync::config::SyncConfig;
use crate::sync::event::SyncEvent;
use crate::sync::merge::merge_events;
use crate::sync::webdav::WebdavClient;

/// WebDAV 上的同步根目录（相对 base_url）
#[allow(dead_code)]
const SYNC_ROOT: &str = "CCopy/sync";
/// upsert 事件目录：每条事件一个文件 events/<hash>.json，同 hash 覆盖
const EVENTS_DIR: &str = "CCopy/sync/events";
/// 删除标记目录：tombstones/<hash>.json，同 hash 覆盖
const TOMBSTONES_DIR: &str = "CCopy/sync/tombstones";

/// 协调器：持有配置与本地状态，提供推送/拉取接口
pub struct SyncCoordinator {
    config: SyncConfig,
    client: WebdavClient,
    device_name: String,
    /// 本地缓存的各设备文件 etag：path -> etag，避免重复下载
    etag_cache: Arc<Mutex<HashMap<String, String>>>,
    /// 待推送的事件队列：后台线程消费
    pending: Arc<Mutex<Vec<SyncEvent>>>,
    /// 已创建的目录集合：避免重复 MKCOL
    dir_created: Arc<Mutex<std::collections::HashSet<String>>>,
    /// 限流退避截止时间戳（毫秒）：此时间前跳过所有网络请求
    backoff_until: Arc<std::sync::atomic::AtomicI64>,
}

/// 默认拉取间隔（秒）
pub const PULL_INTERVAL_SECS: u64 = 60;
/// 限流退避时长（秒）
const BACKOFF_SECS: i64 = 120;

impl SyncCoordinator {
    /// 构造协调器：配置必须可用（调用方先判断 is_usable）
    pub fn new(config: SyncConfig) -> Self {
        let client = WebdavClient::new(&config.url, &config.username, &config.password);
        Self {
            config,
            client,
            // device_name 仅作事件元数据（source_device），固定值，用户无需配置
            // 防回环已由 apply_events 的 LWW 策略处理，不依赖 device_name
            device_name: "ccopy".to_string(),
            etag_cache: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(Vec::new())),
            dir_created: Arc::new(Mutex::new(std::collections::HashSet::new())),
            backoff_until: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }
    }

    /// 是否处于限流退避期
    fn in_backoff(&self) -> bool {
        now_millis() < self.backoff_until.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 标记限流退避：错误信息含限流特征时，延长退避时间
    fn maybe_backoff(&self, err: &str) {
        let limited = err.contains("Too many requests")
            || err.contains("BlockedTemporarily")
            || err.contains("HTTP 429")
            || err.contains("HTTP 503");
        if limited {
            let until = now_millis() + BACKOFF_SECS * 1000;
            self.backoff_until
                .store(until, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// 探测连通性
    #[allow(dead_code)]
    pub fn ping(&self) -> Result<(), String> {
        self.client.ping()
    }

    /// 推送 upsert 事件：生成全量快照入队
    /// 过滤规则（两个开关正交，语义清晰）：
    /// - 图片类：仅受 image_enabled 控制，与 marked_only 无关
    ///   （图片本身就是显性内容，不应被「仅标记」挡住）
    /// - 文本/HTML/RTF 等非图片类：marked_only 开启时需有备注才同步
    pub fn push_upsert(&self, item: &ClipboardItem) {
        let is_image = matches!(item.kind, ClipboardKind::Image);
        if is_image {
            // 图片：只看 image_enabled
            if !self.config.image_enabled {
                return;
            }
        } else {
            // 非图片：marked_only 开启时需有备注
            if self.config.marked_only && item.note.is_none() {
                return;
            }
        }
        let event = SyncEvent::from_item(item, &self.device_name);
        if let Ok(mut q) = self.pending.lock() {
            q.push(event);
        }
    }

    /// 推送 delete 事件
    pub fn push_delete(&self, hash: &str) {
        let event = SyncEvent::delete(hash, &self.device_name, now_millis());
        if let Ok(mut q) = self.pending.lock() {
            q.push(event);
        }
    }

    /// 准备全量推送：返回符合过滤规则的本地记录（主线程调用，不碰网络）
    /// 用于「立即同步」时把本地已有数据一次性推上去
    pub fn collect_push_all(&self, storage: &Storage) -> Vec<ClipboardItem> {
        let items = match storage.list_all_items(false) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        items
            .into_iter()
            .filter(|item| {
                let is_image = matches!(item.kind, ClipboardKind::Image);
                if is_image {
                    self.config.image_enabled
                } else {
                    !self.config.marked_only || item.note.is_some()
                }
            })
            .collect()
    }

    /// 后台执行全量推送：对已收集的 items 做去重（远端已有则跳过）后入队并 flush
    /// 返回实际推送的条数
    pub fn flush_push_all(&self, items: Vec<ClipboardItem>) -> Result<usize, String> {
        if self.in_backoff() {
            return Ok(0);
        }
        // 列出远端已有的事件 hash 集合（失败则当作空集，全量推）
        let remote_hashes: std::collections::HashSet<String> = self
            .list_remote_event_hashes()
            .unwrap_or_default();
        let mut to_push = Vec::new();
        for item in &items {
            if remote_hashes.contains(&item.hash) {
                continue;
            }
            let event = SyncEvent::from_item(item, &self.device_name);
            to_push.push(event);
        }
        if to_push.is_empty() {
            return Ok(0);
        }
        match self.do_flush_push(&to_push) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.maybe_backoff(&e);
                Err(e)
            }
        }
    }

    /// 列出远端 events 目录下所有事件文件的 hash 集合
    /// 文件名形如 <hash>.json，提取 hash 部分
    fn list_remote_event_hashes(&self) -> Result<std::collections::HashSet<String>, String> {
        self.ensure_dir_cached(EVENTS_DIR)?;
        let files = self.client.list(EVENTS_DIR)?;
        let mut hashes = std::collections::HashSet::new();
        for f in &files {
            if f.is_dir {
                continue;
            }
            // href 形如 /dav/CCopy/sync/events/abc123.json，取最后一段文件名去 .json 后缀
            if let Some(name) = f.href.rsplit('/').next() {
                if let Some(hash) = name.strip_suffix(".json") {
                    hashes.insert(hash.to_string());
                }
            }
        }
        Ok(hashes)
    }

    /// 确保目录存在（带缓存）：已创建过的目录不重复 MKCOL
    fn ensure_dir_cached(&self, rel: &str) -> Result<(), String> {
        {
            let cache = self.dir_created.lock().map_err(|e| format!("锁失败: {e}"))?;
            if cache.contains(rel) {
                return Ok(());
            }
        }
        self.client.ensure_dir(rel)?;
        if let Ok(mut cache) = self.dir_created.lock() {
            cache.insert(rel.to_string());
        }
        Ok(())
    }

    /// 执行一次推送：把待推送事件逐条 PUT 到 WebDAV
    /// 在后台线程调用。限流退避期间跳过，事件保留在队列里下次重试
    /// 返回成功推送的条数
    pub fn flush_push(&self) -> Result<usize, String> {
        if self.in_backoff() {
            return Ok(0); // 退避中，事件留在队列
        }
        let events: Vec<SyncEvent> = {
            let mut q = self.pending.lock().map_err(|e| format!("锁失败: {e}"))?;
            std::mem::take(&mut *q)
        };
        if events.is_empty() {
            return Ok(0);
        }
        // 执行推送，失败时把未推事件放回队列并触发退避
        match self.do_flush_push(&events) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.maybe_backoff(&e);
                // 事件放回队列尾部，下次重试
                if let Ok(mut q) = self.pending.lock() {
                    q.extend(events);
                }
                Err(e)
            }
        }
    }

    /// 实际推送逻辑：每条事件单独 PUT 一个文件
    /// upsert -> events/<hash>.json，delete -> tombstones/<hash>.json
    /// 同 hash 覆盖式更新，文件不堆积，流量=单条大小
    /// 图片项：入队阶段已按 image_enabled 过滤，能进队列的图片都需上传图片文件
    /// 返回成功推送的条数（部分失败时返回已成功数，遇错中止后续）
    fn do_flush_push(&self, events: &[SyncEvent]) -> Result<usize, String> {
        let mut pushed = 0usize;
        for ev in events {
            // 图片项上传图片文件（非图片原样返回）
            let ev_to_write = self.maybe_upload_image(ev)?;
            let json = ev_to_write.to_jsonl()?;
            match &ev_to_write {
                SyncEvent::Upsert(item) => {
                    self.ensure_dir_cached(EVENTS_DIR)?;
                    let rel = format!("{EVENTS_DIR}/{}.json", item.id);
                    self.client.put(&rel, json.into_bytes())?;
                }
                SyncEvent::Delete(d) => {
                    self.ensure_dir_cached(TOMBSTONES_DIR)?;
                    let rel = format!("{TOMBSTONES_DIR}/{}.json", d.id);
                    self.client.put(&rel, json.into_bytes())?;
                }
            }
            pushed += 1;
        }
        Ok(pushed)
    }

    /// 图片项上传：把本地 blob 文件上传到 images/<hash>.png，返回改写 blob_path 后的事件
    /// 非图片项原样返回；image_enabled 关闭时图片元数据照传但不传图片文件
    fn maybe_upload_image(&self, ev: &SyncEvent) -> Result<SyncEvent, String> {
        let SyncEvent::Upsert(item) = ev else { return Ok(ev.clone()) };
        if item.kind != "image" {
            return Ok(ev.clone());
        }
        // image_enabled 关闭：图片元数据照传（不带 blob_path），不传图片文件
        if !self.config.image_enabled {
            let mut new_item = item.clone();
            new_item.blob_path = None;
            return Ok(SyncEvent::Upsert(new_item));
        }
        let local_blob = match &item.blob_path {
            Some(p) if !p.is_empty() => p.clone(),
            _ => return Ok(ev.clone()),
        };
        // 已经是同步路径（images/...）则不再上传
        if local_blob.starts_with("images/") {
            return Ok(ev.clone());
        }
        // 读取本地图片文件
        let local_path = crate::storage::data_dir().join(&local_blob);
        let data = match std::fs::read(&local_path) {
            Ok(d) => d,
            Err(_) => return Ok(ev.clone()), // 本地文件缺失，放弃图片但保留事件
        };
        let remote = format!("CCopy/sync/images/{}", item.id);
        // 确保 images 目录存在
        self.client.ensure_dir("CCopy/sync/images")?;
        self.client.put(&remote, data)?;
        // 改写 blob_path 为同步相对路径
        let mut new_item = item.clone();
        new_item.blob_path = Some(format!("images/{}", item.id));
        Ok(SyncEvent::Upsert(new_item))
    }

    /// 执行一次拉取：列所有设备文件，增量下载，合并出待应用事件
    /// 仅做网络与合并，不碰 storage（在后台线程调用）
    /// 限流退避期间返回空列表，调用方据此跳过
    pub fn pull_collect(&self) -> Result<Vec<SyncEvent>, String> {
        if self.in_backoff() {
            return Ok(Vec::new());
        }
        // 执行拉取，限流错误时触发退避
        match self.do_pull_collect() {
            Ok(v) => Ok(v),
            Err(e) => {
                self.maybe_backoff(&e);
                Err(e)
            }
        }
    }

    /// 实际拉取逻辑：列 events/ 和 tombstones/ 两个扁平目录，etag 增量下载
    fn do_pull_collect(&self) -> Result<Vec<SyncEvent>, String> {
        let mut all_events: Vec<SyncEvent> = Vec::new();
        // 拉取 upsert 事件
        all_events.extend(self.pull_dir(EVENTS_DIR)?);
        // 拉取删除标记
        all_events.extend(self.pull_dir(TOMBSTONES_DIR)?);
        // 合并：按 id 取最新（delete 时间戳更大则胜出）
        let merged = merge_events(all_events);
        Ok(merged.into_iter().map(|m| m.event).collect())
    }

    /// 拉取单个目录下所有事件文件：etag 缓存增量下载，防回环跳过本机事件
    fn pull_dir(&self, dir: &str) -> Result<Vec<SyncEvent>, String> {
        self.ensure_dir_cached(dir)?;
        let files = match self.client.list(dir) {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };
        let mut events = Vec::new();
        for f in &files {
            if f.is_dir {
                continue;
            }
            // etag 缓存：未变化则跳过
            let cache_key = f.href.clone();
            let cached = {
                let cache = self.etag_cache.lock().map_err(|e| format!("锁失败: {e}"))?;
                cache.get(&cache_key).cloned()
            };
            if let Some(et) = &cached {
                if !et.is_empty() && et == &f.etag {
                    continue;
                }
            }
            // 下载文件内容（单条事件）
            let data = match self.client.get(&f.href) {
                Ok(d) => d,
                Err(_) => continue,
            };
            // 更新 etag 缓存
            if let Ok(mut cache) = self.etag_cache.lock() {
                cache.insert(cache_key, f.etag.clone());
            }
            // 解析：每个文件一条事件
            let text = String::from_utf8_lossy(&data);
            if let Ok(ev) = SyncEvent::from_jsonl(text.trim()) {
                // 防回环由 apply_events 的 LWW 策略处理：
                // 本机产生的事件拉回时本地 updated_at 相同，会被跳过
                events.push(ev);
            }
        }
        Ok(events)
    }

    /// 把合并后的事件应用到本地存储（在主线程调用）
    /// LWW 策略：本地已有该 hash 且本地 updated_at >= 远端 updated_at 时跳过（不覆盖较新数据）
    /// 这同时实现了防回环：本机产生的事件拉回时本地 updated_at 相同，跳过
    /// 图片项：blob_path 为 images/<hash>.png 时下载图片到本地 blobs 并改写为本地路径
    /// 图片下载失败则跳过该记录（避免存入无效路径）
    pub fn apply_events(&self, storage: &Storage, events: Vec<SyncEvent>) -> PullStats {
        let mut applied = 0usize;
        let mut deleted = 0usize;
        let mut skipped = 0usize;
        for ev in events {
            match ev {
                SyncEvent::Upsert(mut item) => {
                    // LWW：本地已有且本地 updated_at >= 远端 updated_at 则跳过
                    // item.id 即内容 hash（同步用 hash 作 ID）
                    if let Ok(Some(local)) = storage.find_by_hash(&item.id) {
                        if local.updated_at >= item.updated_at {
                            skipped += 1;
                            continue;
                        }
                    }
                    // 图片同步路径处理：下载图片到本地 blobs
                    if item.kind == "image" {
                        if let Some(ref blob) = item.blob_path {
                            if blob.starts_with("images/") {
                                match self.download_image(&item.id, blob) {
                                    Ok(local_blob) => {
                                        item.blob_path = Some(local_blob);
                                    }
                                    Err(_) => continue, // 图片下载失败，跳过该记录
                                }
                            }
                        }
                    }
                    let cli_item = item.to_clipboard_item();
                    if storage.upsert_item(&cli_item).is_ok() {
                        applied += 1;
                    }
                }
                SyncEvent::Delete(d) => {
                    // LWW：本地已有且本地 updated_at > 删除时间戳则跳过（删除后又更新了）
                    if let Ok(Some(local)) = storage.find_by_hash(&d.id) {
                        if local.updated_at > d.updated_at {
                            skipped += 1;
                            continue;
                        }
                        if let Some(id) = local.id {
                            let _ = storage.delete_item(id);
                            deleted += 1;
                        }
                    }
                }
            }
        }
        PullStats {
            downloaded: 0,
            skipped,
            applied,
            deleted,
        }
    }

    /// 下载同步图片到本地 blobs 目录，返回本地 blob_path
    fn download_image(&self, hash: &str, sync_blob: &str) -> Result<String, String> {
        // sync_blob 形如 images/<hash>.png，对应 WebDAV 路径 CCopy/sync/images/<hash>.png
        let remote = format!("CCopy/sync/{}", sync_blob);
        let data = self.client.get(&remote)?;
        // 本地存储路径：blobs/image/<YYYY>/<MM>/<hash>.png
        let now = now_millis();
        let secs = (now / 1000) as i64;
        use chrono::{Datelike, TimeZone, Utc};
        let dt = Utc.timestamp_opt(secs, 0).single().unwrap_or_else(|| Utc::now());
        let local_blob = format!("blobs/image/{:04}/{:02}/{}.png", dt.year(), dt.month(), hash);
        let local_path = crate::storage::data_dir().join(&local_blob);
        if let Some(parent) = local_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&local_path, &data).map_err(|e| format!("写入图片失败: {e}"))?;
        Ok(local_blob)
    }

    /// 清理过期归档：删除超过保留期的 events 和 tombstones 文件
    /// 按 last-modified 时间判断，retain_months=0 表示不清理
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        if self.config.retain_months == 0 {
            return Ok(0);
        }
        let cutoff = cutoff_month(self.config.retain_months);
        let mut removed = 0usize;
        // 清理两个目录：按文件名无时间信息，改用 last_modified 日期判断
        for dir in [EVENTS_DIR, TOMBSTONES_DIR] {
            let files = match self.client.list(dir) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for f in files {
                if f.is_dir {
                    continue;
                }
                // last_modified 形如 "Mon, 21 Jul 2026 12:00:00 GMT"，取月份判断
                if let Some(month) = extract_month_from_http_date(&f.last_modified) {
                    if month < cutoff {
                        if self.client.delete(&f.href).is_ok() {
                            removed += 1;
                        }
                    }
                }
            }
        }
        Ok(removed)
    }
}

/// 拉取统计
#[derive(Debug, Default, Clone)]
pub struct PullStats {
    #[allow(dead_code)]
    pub downloaded: usize,
    #[allow(dead_code)]
    pub skipped: usize,
    pub applied: usize,
    pub deleted: usize,
}

/// N 个月前的月份字符串（用于清理阈值）
fn cutoff_month(retain_months: usize) -> String {
    let now = now_millis();
    let secs = (now / 1000) as i64;
    use chrono::{Datelike, Months, TimeZone, Utc};
    let dt = Utc.timestamp_opt(secs, 0).single().unwrap_or_else(|| Utc::now());
    let cutoff = dt.checked_sub_months(Months::new(retain_months as u32)).unwrap_or(dt);
    format!("{:04}-{:02}", cutoff.year(), cutoff.month())
}

/// 从 HTTP 日期（RFC 2822）提取月份：如 "Mon, 21 Jul 2026 12:00:00 GMT" -> "2026-07"
fn extract_month_from_http_date(date: &str) -> Option<String> {
    use chrono::{Datelike, NaiveDate, Utc};
    // 尝试解析 RFC 2822 格式
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(date) {
        return Some(format!("{:04}-{:02}", dt.year(), dt.month()));
    }
    // 兜底：手动解析 "Mon, 21 Jul 2026 ..." 格式
    let parts: Vec<&str> = date.split_whitespace().collect();
    if parts.len() >= 4 {
        let day: u32 = parts[1].parse().ok()?;
        let month = match parts[2] {
            "Jan" => 1, "Feb" => 2, "Mar" => 3, "Apr" => 4, "May" => 5, "Jun" => 6,
            "Jul" => 7, "Aug" => 8, "Sep" => 9, "Oct" => 10, "Nov" => 11, "Dec" => 12,
            _ => return None,
        };
        let year: i32 = parts[3].parse().ok()?;
        let _ = NaiveDate::from_ymd_opt(year, month, day)?; // 校验日期合法
        let _ = Utc::now();
        return Some(format!("{:04}-{:02}", year, month));
    }
    None
}

/// 协调器句柄：跨线程共享（Arc + Mutex 保护协调器内部可变状态）
pub type SharedCoordinator = Arc<Mutex<SyncCoordinator>>;

/// 启动同步后台线程：定时推送 + 拉取
/// storage 为共享的 Rc<RefCell<Storage>>，但后台线程不能直接用 Rc，
/// 因此由调用方在主线程定时触发 flush_push/pull_once，这里只提供工具函数
pub fn build_coordinator(config: SyncConfig) -> Option<SharedCoordinator> {
    if !config.is_usable() {
        return None;
    }
    Some(Arc::new(Mutex::new(SyncCoordinator::new(config))))
}

/// 后台执行单次推送（无需回调）
pub fn spawn_flush_silent(coordinator: SharedCoordinator) {
    std::thread::spawn(move || {
        if let Ok(c) = coordinator.lock() {
            let _ = c.flush_push();
        }
    });
}

/// 后台执行单次拉取：网络下载+合并，完成后回调主线程落库
/// on_events 在后台线程执行，拿到事件后需通过 invoke_from_event_loop 回主线程
pub fn spawn_pull<F>(coordinator: SharedCoordinator, on_events: F)
where
    F: FnOnce(Result<Vec<SyncEvent>, String>) + Send + 'static,
{
    std::thread::spawn(move || {
        let result = match coordinator.lock() {
            Ok(c) => c.pull_collect(),
            Err(e) => Err(format!("协调器锁失败: {e}")),
        };
        on_events(result);
    });
}

/// 后台清理过期归档
pub fn spawn_cleanup(coordinator: SharedCoordinator) {
    std::thread::spawn(move || {
        if let Ok(c) = coordinator.lock() {
            let _ = c.cleanup_expired();
        }
    });
}
