//! WebDAV 客户端封装：基于 ureq + http crate 实现 PROPFIND/GET/PUT/DELETE/MKCOL
//! 对接坚果云等标准 WebDAV 服务
//! ureq 3.x 通过 http::Request::builder 构造自定义方法请求，需启用 allow_non_standard_methods

use std::io::Read;
use std::time::Duration;

use http::Request;
use ureq::config::Config;

/// WebDAV 客户端：持有基础地址与鉴权信息
pub struct WebdavClient {
    base_url: String,
    agent: ureq::Agent,
    auth_header: String,
}

/// PROPFIND 返回的单个资源项
#[derive(Debug, Clone)]
pub struct WebdavResource {
    /// 相对 base_url 的路径（以 / 开头）
    pub href: String,
    /// 是否为目录（collection）
    pub is_dir: bool,
    /// etag，可能为空
    pub etag: String,
    /// 最后修改时间（HTTP 日期格式），可能为空
    #[allow(dead_code)]
    pub last_modified: String,
}

impl WebdavClient {
    /// 创建客户端：base_url 末尾不带 /，内部统一拼接
    pub fn new(base_url: &str, username: &str, password: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        // 启用非标准方法（PROPFIND/MKCOL），否则 ureq 会拒绝
        let agent = ureq::Agent::new_with_config(
            Config::builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .allow_non_standard_methods(true)
                .build(),
        );
        Self {
            base_url,
            agent,
            auth_header: basic_auth(username, password),
        }
    }

    /// 拼接完整 URL：base_url + 相对路径
    fn url(&self, rel: &str) -> String {
        let rel = rel.trim_start_matches('/');
        format!("{}/{}", self.base_url, rel)
    }

    /// 发送无 body 请求
    fn run_no_body(&self, method: &str, url: &str, extra: &[(&str, &str)]) -> Result<http::Response<ureq::Body>, String> {
        let mut builder = Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", &self.auth_header);
        for (k, v) in extra {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(()).map_err(|e| format!("构造请求失败: {e}"))?;
        self.agent.run(req).map_err(map_err)
    }

    /// 发送带 body 请求
    fn run_with_body(&self, method: &str, url: &str, body: Vec<u8>, extra: &[(&str, &str)]) -> Result<http::Response<ureq::Body>, String> {
        let mut builder = Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", &self.auth_header);
        for (k, v) in extra {
            builder = builder.header(*k, *v);
        }
        let req = builder.body(body).map_err(|e| format!("构造请求失败: {e}"))?;
        self.agent.run(req).map_err(map_err)
    }

    #[allow(dead_code)]
    /// 探测连通性：PROPFIND 根目录深度 0
    pub fn ping(&self) -> Result<(), String> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><displayname/></prop></propfind>"#;
        let resp = self.run_with_body(
            "PROPFIND",
            &self.url(""),
            body.as_bytes().to_vec(),
            &[("Depth", "0")],
        )?;
        if resp.status().as_u16() < 400 {
            Ok(())
        } else {
            Err(format!("HTTP {}", resp.status()))
        }
    }

    /// 确保目录存在：逐级 MKCOL，已存在则忽略 405/409
    pub fn ensure_dir(&self, rel: &str) -> Result<(), String> {
        let parts: Vec<&str> = rel.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        let mut cur = String::new();
        for p in parts {
            cur = format!("{cur}/{p}");
            match self.run_no_body("MKCOL", &self.url(&cur), &[]) {
                Ok(_) => {}
                Err(e) if e.contains("HTTP 405") || e.contains("HTTP 409") => {}
                Err(e) => return Err(format!("MKCOL {cur} 失败: {e}")),
            }
        }
        Ok(())
    }

    /// 上传文件内容（PUT 覆盖）
    pub fn put(&self, rel: &str, data: Vec<u8>) -> Result<(), String> {
        let resp = self.run_with_body("PUT", &self.url(rel), data, &[])?;
        if resp.status().as_u16() < 400 {
            Ok(())
        } else {
            Err(format!("HTTP {}", resp.status()))
        }
    }

    /// 下载文件内容（GET）
    pub fn get(&self, rel: &str) -> Result<Vec<u8>, String> {
        let resp = self.run_no_body("GET", &self.url(rel), &[])?;
        let mut buf = Vec::new();
        resp.into_body()
            .into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        Ok(buf)
    }

    /// 删除文件或目录（DELETE）
    pub fn delete(&self, rel: &str) -> Result<(), String> {
        match self.run_no_body("DELETE", &self.url(rel), &[]) {
            Ok(_) => Ok(()),
            Err(e) if e.contains("HTTP 404") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 列目录（PROPFIND 深度 1）：返回子资源（不含自身）
    pub fn list(&self, rel: &str) -> Result<Vec<WebdavResource>, String> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop>
    <resourcetype/>
    <getetag/>
    <getlastmodified/>
  </prop>
</propfind>"#;
        let resp = self.run_with_body(
            "PROPFIND",
            &self.url(rel),
            body.as_bytes().to_vec(),
            &[("Depth", "1"), ("Content-Type", "application/xml; charset=utf-8")],
        )?;
        let mut xml = String::new();
        resp.into_body()
            .into_reader()
            .read_to_string(&mut xml)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        let resources = parse_propfind(&xml, &self.base_url);
        // 去掉第一个（自身）
        Ok(resources.into_iter().skip(1).collect())
    }
}

/// 把 ureq 错误转为字符串：StatusCode 错误带 HTTP 码，便于上层判断
fn map_err(e: ureq::Error) -> String {
    match &e {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        _ => format!("{e}"),
    }
}

/// 拼接 Basic Auth 头
fn basic_auth(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!("Basic {}", base64_encode(raw.as_bytes()))
}

/// 极简 Base64 编码（标准字母表，带填充），避免引入额外依赖
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

/// 解析 PROPFIND 多状态 XML：提取每个 response 的 href、resourcetype、etag、lastmodified
fn parse_propfind(xml: &str, base_url: &str) -> Vec<WebdavResource> {
    let mut result = Vec::new();
    let base_decoded = urldecode(base_url);
    for resp_block in split_all(xml, "<D:response>", "</D:response>")
        .into_iter()
        .chain(split_all(xml, "<d:response>", "</d:response>"))
        .chain(split_all(xml, "<response>", "</response>"))
    {
        let href = extract_tag(&resp_block, "href")
            .or_else(|| extract_tag(&resp_block, "D:href"))
            .or_else(|| extract_tag(&resp_block, "d:href"))
            .unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let href_decoded = urldecode(&href);
        let rel = if href_decoded.starts_with(&base_decoded) {
            href_decoded[base_decoded.len()..].to_string()
        } else if href_decoded.starts_with("http") {
            if let Some(idx) = href_decoded.find("://") {
                let after = &href_decoded[idx + 3..];
                if let Some(slash) = after.find('/') {
                    after[slash..].to_string()
                } else {
                    String::new()
                }
            } else {
                href_decoded
            }
        } else {
            href_decoded
        };
        let is_dir = resp_block.contains("<D:collection/>")
            || resp_block.contains("<d:collection/>")
            || resp_block.contains("<collection xmlns=\"DAV:\"/>");
        let etag = extract_tag(&resp_block, "getetag")
            .or_else(|| extract_tag(&resp_block, "D:getetag"))
            .or_else(|| extract_tag(&resp_block, "d:getetag"))
            .unwrap_or_default();
        let last_modified = extract_tag(&resp_block, "getlastmodified")
            .or_else(|| extract_tag(&resp_block, "D:getlastmodified"))
            .or_else(|| extract_tag(&resp_block, "d:getlastmodified"))
            .unwrap_or_default();
        result.push(WebdavResource {
            href: rel,
            is_dir,
            etag,
            last_modified,
        });
    }
    result
}

/// 提取 XML 标签内容（匹配多种命名空间前缀）
fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    for (open, close) in [
        (format!("<{tag}>"), format!("</{tag}>")),
        (format!("<D:{tag}>"), format!("</D:{tag}>")),
        (format!("<d:{tag}>"), format!("</d:{tag}>")),
    ] {
        if let Some(start) = xml.find(&open) {
            let content_start = start + open.len();
            if let Some(end) = xml[content_start..].find(&close) {
                return Some(xml[content_start..content_start + end].trim().to_string());
            }
        }
    }
    None
}

/// 提取所有匹配 <open>...</close> 的块（用于切分 response）
fn split_all(xml: &str, open: &str, close: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let content_start = start + open.len();
        if let Some(end) = rest[content_start..].find(close) {
            result.push(format!("{open}{}{close}", &rest[content_start..content_start + end]));
            rest = &rest[content_start + end + close.len()..];
        } else {
            break;
        }
    }
    result
}

/// 简单 URL 解码（处理 %xx）
fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                result.push(b as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_basic() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn urldecode_basic() {
        assert_eq!(urldecode("abc"), "abc");
        assert_eq!(urldecode("a%2Fb"), "a/b");
        assert_eq!(urldecode("%20"), " ");
    }

    #[test]
    fn extract_tag_works() {
        let xml = "<D:href>/dav/path</D:href>";
        assert_eq!(extract_tag(xml, "href").unwrap(), "/dav/path");
        assert_eq!(extract_tag(xml, "D:href").unwrap(), "/dav/path");
    }
}
