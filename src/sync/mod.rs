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

use crate::clipboard_item::ClipboardItem;
use crate::common::now_millis;
use crate::storage::Storage;
use crate::sync::config::SyncConfig;
use crate::sync::event::SyncEvent;
use crate::sync::merge::merge_events;
use crate::sync::webdav::WebdavClient;

/// WebDAV 上的同步根目录（相对 base_url）
#[allow(dead_code)]
const SYNC_ROOT: &str = "CCopy/sync";
/// 设备事件文件目录
const DEVICES_DIR: &str = "CCopy/sync/devices";

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
        let device_name = config.device_name.clone();
        let client = WebdavClient::new(&config.url, &config.username, &config.password);
        Self {
            config,
            client,
            device_name,
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

    /// 推送 upsert 事件：读出完整 item 生成全量快照，追加到本月 jsonl
    pub fn push_upsert(&self, item: &ClipboardItem) {
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

    /// 执行一次推送：把待推送事件追加到 WebDAV 本月 jsonl
    /// 在后台线程调用。限流退避期间跳过，事件保留在队列里下次重试
    pub fn flush_push(&self) -> Result<(), String> {
        if self.in_backoff() {
            return Ok(()); // 退避中，事件留在队列
        }
        let events: Vec<SyncEvent> = {
            let mut q = self.pending.lock().map_err(|e| format!("锁失败: {e}"))?;
            std::mem::take(&mut *q)
        };
        if events.is_empty() {
            return Ok(());
        }
        // 执行推送，失败时把事件放回队列并触发退避
        if let Err(e) = self.do_flush_push(&events) {
            self.maybe_backoff(&e);
            // 事件放回队列尾部，下次重试
            if let Ok(mut q) = self.pending.lock() {
                q.extend(events);
            }
            return Err(e);
        }
        Ok(())
    }

    /// 实际推送逻辑：确保目录、读旧内容、追加、PUT
    fn do_flush_push(&self, events: &[SyncEvent]) -> Result<(), String> {
        let device_dir = format!("{DEVICES_DIR}/{}", self.device_name);
        self.ensure_dir_cached(&device_dir)?;
        // 读取本月文件已有内容
        let month = current_month();
        let rel = format!("{device_dir}/{month}.jsonl");
        let mut content = match self.client.get(&rel) {
            Ok(data) => String::from_utf8_lossy(&data).to_string(),
            Err(_) => String::new(), // 文件不存在
        };
        // 追加新事件（图片项需先上传图片）
        for ev in events {
            let ev_to_write = if self.config.image_enabled {
                self.maybe_upload_image(ev)?
            } else {
                ev.clone()
            };
            if let Ok(line) = ev_to_write.to_jsonl() {
                content.push_str(&line);
                content.push('\n');
            }
        }
        self.client.put(&rel, content.into_bytes())
    }

    /// 图片项上传：把本地 blob 文件上传到 images/<hash>.png，返回改写 blob_path 后的事件
    /// 非图片项或上传失败时原样返回
    fn maybe_upload_image(&self, ev: &SyncEvent) -> Result<SyncEvent, String> {
        let SyncEvent::Upsert(item) = ev else { return Ok(ev.clone()) };
        if item.kind != "image" {
            return Ok(ev.clone());
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

    /// 实际拉取逻辑
    fn do_pull_collect(&self) -> Result<Vec<SyncEvent>, String> {
        // 确保 devices 目录存在（带缓存）
        self.ensure_dir_cached(DEVICES_DIR)?;
        // 列 devices 目录，获取各设备子目录
        let device_dirs = self.client.list(DEVICES_DIR)?;
        let mut all_events: Vec<SyncEvent> = Vec::new();

        for d in &device_dirs {
            if !d.is_dir {
                continue;
            }
            // 列该设备目录下的月份文件
            let sub_files = match self.client.list(&d.href) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for f in sub_files {
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
                // 下载文件内容
                let data = match self.client.get(&f.href) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                // 更新 etag 缓存
                if let Ok(mut cache) = self.etag_cache.lock() {
                    cache.insert(cache_key, f.etag.clone());
                }
                // 解析每行事件
                let text = String::from_utf8_lossy(&data);
                for line in text.lines() {
                    if let Ok(ev) = SyncEvent::from_jsonl(line) {
                        // 防回环：跳过本机产生的事件
                        let is_self = match &ev {
                            SyncEvent::Upsert(i) => i.source_device == self.device_name,
                            SyncEvent::Delete(d) => d.source_device == self.device_name,
                        };
                        if !is_self {
                            all_events.push(ev);
                        }
                    }
                }
            }
        }

        // 合并：按 id 取最新
        let merged = merge_events(all_events);
        Ok(merged.into_iter().map(|m| m.event).collect())
    }

    /// 把合并后的事件应用到本地存储（在主线程调用）
    /// 图片项：blob_path 为 images/<hash>.png 时下载图片到本地 blobs 并改写为本地路径
    /// 图片下载失败则跳过该记录（避免存入无效路径）
    pub fn apply_events(&self, storage: &Storage, events: Vec<SyncEvent>) -> PullStats {
        let mut applied = 0usize;
        let mut deleted = 0usize;
        for ev in events {
            match ev {
                SyncEvent::Upsert(mut item) => {
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
                    if let Ok(Some(local)) = storage.find_by_hash(&d.id) {
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
            skipped: 0,
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

    /// 清理过期归档：删除超过保留期的设备月份文件
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        if self.config.retain_months == 0 {
            return Ok(0);
        }
        let cutoff = cutoff_month(self.config.retain_months);
        let device_dirs = self.client.list(DEVICES_DIR)?;
        let mut removed = 0usize;
        for d in &device_dirs {
            if !d.is_dir {
                continue;
            }
            let sub_files = match self.client.list(&d.href) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for f in sub_files {
                if f.is_dir {
                    continue;
                }
                // 文件名形如 2026-07.jsonl
                if let Some(month) = extract_month_from_path(&f.href) {
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

/// 当前月份字符串：YYYY-MM
fn current_month() -> String {
    let now = now_millis();
    let secs = (now / 1000) as i64;
    use chrono::{Datelike, TimeZone, Utc};
    let dt = Utc.timestamp_opt(secs, 0).single().unwrap_or_else(|| Utc::now());
    format!("{:04}-{:02}", dt.year(), dt.month())
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

/// 从路径提取月份：如 devices/pc/2026-07.jsonl -> 2026-07
fn extract_month_from_path(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let stem = name.strip_suffix(".jsonl")?;
    // 校验 YYYY-MM 格式
    if stem.len() == 7
        && stem.as_bytes()[4] == b'-'
        && stem[..4].chars().all(|c| c.is_ascii_digit())
        && stem[5..].chars().all(|c| c.is_ascii_digit())
    {
        Some(stem.to_string())
    } else {
        None
    }
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

/// 后台执行单次推送：事件已通过 push_upsert/push_delete 入队
pub fn spawn_flush(coordinator: SharedCoordinator) {
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
