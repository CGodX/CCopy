//! 自动更新：通过 GitHub API 检查最新 Release，下载安装包并静默升级

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

/// 新版本信息
#[derive(Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    #[allow(dead_code)]
    pub notes: String,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const API_URL: &str = "https://api.github.com/repos/CGodX/CCopy/releases/latest";

/// 检查结果
pub enum CheckResult {
    /// 已是最新
    UpToDate,
    /// 有新版本可用
    Available(UpdateInfo),
}

/// GitHub Release 接口返回结构（仅取需要的字段）
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// 拉取 GitHub 最新 Release 并与当前版本对比
pub fn check() -> Result<CheckResult, String> {
    let body = http_get_text_with_retry(API_URL, 15)?;
    let release: GithubRelease =
        serde_json::from_str(&body).map_err(|e| format!("解析 Release 信息失败: {e}"))?;

    // 版本号去掉 v 前缀
    let remote_version = release.tag_name.trim_start_matches('v').to_string();
    if !version_lt(CURRENT_VERSION, &remote_version) {
        return Ok(CheckResult::UpToDate);
    }

    // 找 Windows 安装包 asset（cargo-packager 生成 *-setup.exe）
    let url = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("-setup.exe"))
        .map(|a| a.browser_download_url.clone())
        .ok_or_else(|| "未找到 Windows 安装包".to_string())?;

    Ok(CheckResult::Available(UpdateInfo {
        version: remote_version,
        url,
        notes: release.body,
    }))
}

/// 下载安装包到临时目录并静默安装，安装完成后程序退出由安装器重启
pub fn download_and_install(url: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("ccopy-update-{}.exe", std::process::id()));
    http_download(url, &tmp, 300)?;
    silent_install(&tmp);
    Ok(())
}

/// 静默安装：调用 NSIS 安装器，/P 静默模式 + /R 安装后自动重启程序
fn silent_install(installer: &PathBuf) {
    use std::process::Command;
    // 安装器会先 kill 当前进程（见 installer.nsi 的 CheckIfAppIsRunning），这里直接退出事件循环
    let _ = Command::new(installer).args(["/P", "/R"]).spawn();
    let _ = slint::quit_event_loop();
}

/// 简单的语义化版本比较：返回 true 表示 a < b
fn version_lt(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a
        .trim_start_matches('v')
        .split('.')
        .filter_map(|s| s.split('-').next()?.parse().ok())
        .collect();
    let pb: Vec<u64> = b
        .trim_start_matches('v')
        .split('.')
        .filter_map(|s| s.split('-').next()?.parse().ok())
        .collect();
    for i in 0..pa.len().max(pb.len()) {
        let va = pa.get(i).copied().unwrap_or(0);
        let vb = pb.get(i).copied().unwrap_or(0);
        if va != vb {
            return va < vb;
        }
    }
    false
}

/// GET 请求返回文本，带重试（最多 3 次）
fn http_get_text_with_retry(url: &str, timeout_secs: u64) -> Result<String, String> {
    let mut last_err = String::new();
    for i in 0..3 {
        match http_get_text_once(url, timeout_secs) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last_err = format!("第{}次失败: {e}", i + 1);
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    Err(format!("网络请求失败（已重试3次）{last_err}"))
}

/// 单次 GET 请求返回文本
fn http_get_text_once(url: &str, timeout_secs: u64) -> Result<String, String> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .build(),
    );
    let resp = agent
        .get(url)
        .header("User-Agent", "CCopy-Updater")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("{e}"))?;
    let mut buf = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut buf)
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok(buf)
}

/// 下载文件到指定路径（大文件，超时设长）
fn http_download(url: &str, dest: &PathBuf, timeout_secs: u64) -> Result<(), String> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .build(),
    );
    let resp = agent
        .get(url)
        .header("User-Agent", "CCopy-Updater")
        .call()
        .map_err(|e| format!("下载失败: {e}"))?;
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建文件失败: {e}"))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(())
}
