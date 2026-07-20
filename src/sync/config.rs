//! 同步配置：从存储读取的运行时配置，供协调器使用

use std::cell::RefCell;
use std::rc::Rc;

use crate::settings::{
    load_sync_device_name, load_sync_enabled, load_sync_image_enabled, load_sync_password,
    load_sync_retain_months_value, load_sync_url, load_sync_username,
};
use crate::storage::Storage;

/// 同步运行时配置：协调器启动时一次性读取
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub enabled: bool,
    pub url: String,
    pub username: String,
    pub password: String,
    pub device_name: String,
    pub image_enabled: bool,
    pub retain_months: usize,
}

impl SyncConfig {
    /// 从存储加载同步配置
    pub fn load(storage: &Rc<RefCell<Storage>>) -> Self {
        Self {
            enabled: load_sync_enabled(storage),
            url: load_sync_url(storage),
            username: load_sync_username(storage),
            password: load_sync_password(storage),
            device_name: load_sync_device_name(storage),
            image_enabled: load_sync_image_enabled(storage),
            retain_months: load_sync_retain_months_value(storage),
        }
    }

    /// 配置是否完整可用：启用且地址/账号非空
    pub fn is_usable(&self) -> bool {
        self.enabled
            && !self.url.trim().is_empty()
            && !self.username.trim().is_empty()
            && !self.device_name.trim().is_empty()
    }
}
