//! 网页链接下载器。
//!
//! 设计取舍：
//! - HTTP 层用系统 curl（Windows 自带 8.x），不重复造 HTTP/断点续传/TLS；
//! - 页面解析用保守的正则，只提取视频/压缩包/图片直链，不执行页面脚本；
//! - 下载在后台线程跑，进度写回 download_tasks，前端轮询；
//! - 联网恢复时把中断任务重新排队（requeue_interrupted），实现「停止后联网自动续跑」。
//! - 站点未知时返回候选列表让用户确认，不假装支持所有站。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use tauri::{AppHandle, Emitter, Manager};

use crate::models::{DownloadTask, MediaCandidate, ResolveReport};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) VideoVault/0.2";

fn find_curl() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VIDEO_VAULT_CURL") {
        let p = PathBuf::from(p);
        if p.is_file() { return Some(p); }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in ["curl.exe", "curl"] {
                let p = dir.join(name);
                if p.is_file() { return Some(p); }
            }
        }
    }
    None
}

fn site_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest).to_ascii_lowercase();
    host.trim_start_matches("www.").to_string()
}

/// 抓页面 HTML。只用于解析直链，绝不上传本地库内容。
fn fetch_html(url: &str) -> Result<String, String> {
    let curl = find_curl().ok_or("未找到 curl，无法访问链接")?;
    let out = crate::proc::quiet(curl)
        .args(["-sSL", "--max-time", "20", "-A", UA])
        .arg(url)
        .output()
        .map_err(|e| format!("请求失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("请求失败（HTTP 或网络错误）: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut html = String::from_utf8_lossy(&out.stdout).to_string();
    if html.len() > 4_000_000 {
        html.truncate(4_000_000);
    }
    Ok(html)
}

fn absolutize(base: &str, raw: &str) -> String {
    let raw = raw.trim().replace("&amp;", "&").replace("\\/", "/");
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return raw;
    }
    if raw.starts_with("//") {
        return format!("https:{}", raw);
    }
    let scheme = base.split("://").next().unwrap_or("https");
    let rest = base.split("://").nth(1).unwrap_or("");
    let host = rest.split('/').next().unwrap_or("");
    if raw.starts_with('/') {
        return format!("{scheme}://{host}{raw}");
    }
    let dir = rest.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    format!("{scheme}://{host}{dir}/{raw}")
}

/// 从 HTML 提取媒体直链。保守策略：只认常见后缀与 og:video/meta。
pub fn extract_candidates(html: &str, base: &str) -> Vec<MediaCandidate> {
    let mut out: Vec<MediaCandidate> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let re = Regex::new(r#"(?i)(?:href|src|content|url)\s*[:=]\s*["']([^"']+?\.(?:mp4|webm|mkv|mov|m4v|ts|m3u8|zip|cbz|rar|7z|jpg|jpeg|png|webp)(?:\?[^"']*)?)["']"#);
    let Ok(re) = re else { return out; };
    for cap in re.captures_iter(html) {
        let url = absolutize(base, &cap[1]);
        if seen.contains(&url) { continue; }
        seen.insert(url.clone());
        let lower = url.to_ascii_lowercase();
        let ext = lower.split('?').next().unwrap_or("").rsplit('.').next().unwrap_or("");
        let kind = match ext {
            "zip" | "cbz" | "rar" | "7z" => "archive",
            "jpg" | "jpeg" | "png" | "webp" => "image",
            _ => "video",
        };
        let name = url.rsplit('/').next().unwrap_or("media").split('?').next().unwrap_or("media").to_string();
        out.push(MediaCandidate { url, name, kind: kind.into() });
    }
    // 一个页面里图片太多通常是缩略图，只保留少量样本交给用户判断
    let images = out.iter().filter(|c| c.kind == "image").count();
    if images > 12 {
        let mut kept = 0;
        out.retain(|c| {
            if c.kind == "image" { kept += 1; kept <= 12 } else { true }
        });
    }
    out
}

pub fn resolve(url: &str) -> Result<ResolveReport, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("只支持 http/https 链接".into());
    }
    let html = fetch_html(url)?;
    let candidates = extract_candidates(&html, url);
    let title_re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok();
    let title = title_re
        .and_then(|r| r.captures(&html))
        .map(|c| c[1].trim().replace(['\n', '\r', '\t'], " "))
        .unwrap_or_default();
    let note = if candidates.is_empty() {
        "页面里没发现直链。可能是需要登录/脚本渲染的站点——可装 yt-dlp 后走外部解析器，或手动贴媒体直链。".into()
    } else {
        format!("发现 {} 个媒体候选，勾选后开始下载", candidates.len())
    };
    Ok(ResolveReport { site: site_of(url), title, candidates, note })
}

/// 用 curl 下载单个 URL，带断点续传（-C -）与进度解析。
pub fn download_one(
    app: &AppHandle,
    task_id: i64,
    url: &str,
    target: &Path,
) -> Result<(i64, i64), String> {
    let curl = find_curl().ok_or("未找到 curl")?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let partial = target.with_extension(format!("{}.part", target.extension().and_then(|s| s.to_str()).unwrap_or("bin")));
    let mut args: Vec<String> = vec![
        "-sSL".into(), "--fail".into(),
        "--retry".into(), "3".into(), "--retry-delay".into(), "2".into(),
        "--connect-timeout".into(), "15".into(),
        "-A".into(), UA.into(),
        "-C".into(), "-".into(),
    ];
    let start = if partial.is_file() { fs::metadata(&partial).map(|m| m.len()).unwrap_or(0) } else { 0 };
    if start > 0 {
        args.push("--continue-at".into());
        args.push(format!("{start}"));
    }
    args.push("-o".into());
    args.push(partial.to_string_lossy().to_string());
    args.push("--write-out".into());
    args.push("%{size_download} %{content_type}".into());
    args.push(url.into());

    let mut child = crate::proc::quiet(curl)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 curl 失败: {e}"))?;

    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = app.emit("download-progress", serde_json::json!({
            "id": task_id, "state": "paused", "error": "网络中断，联网后可续传",
        }));
        return Err("下载中断（将保留分片，联网后可续传）".into());
    }
    fs::rename(&partial, target).map_err(|e| format!("保存文件失败: {e}"))?;
    let done = fs::metadata(target).map(|m| m.len() as i64).unwrap_or(0);
    Ok((done, done))
}

/// 解析 URL 里的文件名，清洗成安全名。
pub fn safe_name(url: &str) -> String {
    let raw = url.rsplit('/').next().unwrap_or("download").split('?').next().unwrap_or("download");
    let decoded = percent_decode(raw);
    decoded.chars().map(|c| match c {
        '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
        _ => c,
    }).collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(h) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 检查网络是否可达（给「联网后自动续跑」用）。
pub fn network_ok() -> bool {
    let Some(curl) = find_curl() else { return false; };
    crate::proc::quiet(curl)
        .args(["-s", "--max-time", "6", "-o", "NUL", "https://www.baidu.com"])
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 后台任务循环：启动时与网络恢复时调用，把 queued 任务逐个跑完。
pub fn run_queue(app: AppHandle, root: PathBuf, max_items: usize) {
    std::thread::spawn(move || {
        let _ = app.emit("download-progress", serde_json::json!({"event": "queue-start"}));
        let mut idle_rounds = 0;
        loop {
            let task: Option<DownloadTask> = {
                let st = app.state::<crate::AppState>().clone();
                let Ok(guard) = st.inner_lock() else { return; };
                let Ok(lib) = guard.library.as_ref().ok_or("库未打开") else { return; };
                let Ok(list) = crate::db::list_downloads(&lib.conn, 200) else { return; };
                list.into_iter().find(|t| t.state == "queued")
            };
            let Some(task) = task else {
                idle_rounds += 1;
                if idle_rounds > 20 { break; } // 约 5 分钟无任务退出，由前端再次触发
                std::thread::sleep(Duration::from_secs(15));
                continue;
            };
            idle_rounds = 0;
            if !network_ok() {
                let _ = app.emit("download-progress", serde_json::json!({
                    "id": task.id, "state": "paused", "error": "离线，等待联网自动续传",
                }));
                {
                    let st = app.state::<crate::AppState>().clone();
                let Ok(guard) = st.inner_lock() else { return; };
                    if let Ok(lib) = guard.library.as_ref().ok_or("库未打开") {
                        let _ = crate::db::update_download(&lib.conn, task.id, "paused", task.progress, "离线等待续传", task.bytes_done, task.bytes_total, None);
                    }
                }
                std::thread::sleep(Duration::from_secs(20));
                continue;
            }
            // 标记 running
            {
                let st = app.state::<crate::AppState>().clone();
                let Ok(guard) = st.inner_lock() else { return; };
                if let Ok(lib) = guard.library.as_ref().ok_or("库未打开") {
                    let _ = crate::db::update_download(&lib.conn, task.id, "running", 0.0, "", task.bytes_done, task.bytes_total, None);
                }
            }
            let _ = app.emit("download-progress", serde_json::json!({"id": task.id, "state": "running"}));
            let out_dir = root.join("downloads");
            let target = if task.target_path.is_empty() {
                out_dir.join(safe_name(&task.url))
            } else {
                PathBuf::from(&task.target_path)
            };
            match download_one(&app, task.id, &task.url, &target) {
                Ok((done, total)) => {
                    let st = app.state::<crate::AppState>().clone();
                let Ok(guard) = st.inner_lock() else { return; };
                    if let Ok(lib) = guard.library.as_ref().ok_or("库未打开") {
                        let _ = crate::db::update_download(&lib.conn, task.id, "done", 1.0, "", done, total, Some(&target.to_string_lossy()));
                    }
                    let _ = app.emit("download-progress", serde_json::json!({
                        "id": task.id, "state": "done", "path": target.to_string_lossy(),
                    }));
                }
                Err(e) => {
                    let st = app.state::<crate::AppState>().clone();
                let Ok(guard) = st.inner_lock() else { return; };
                    if let Ok(lib) = guard.library.as_ref().ok_or("库未打开") {
                        let _ = crate::db::update_download(&lib.conn, task.id, "paused", task.progress, &e, task.bytes_done, task.bytes_total, None);
                    }
                    let _ = app.emit("download-progress", serde_json::json!({
                        "id": task.id, "state": "paused", "error": e,
                    }));
                }
            }
            // 保护：最多处理 max_items 个，避免一次跑太久
            if max_items > 0 && idle_rounds >= max_items as i32 { break; }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_known_direct_links() {
        let html = r#"
          <html><head><title>测试页 - Example</title></head><body>
          <video src="https://cdn.example.com/a/BIG_BUCK_BUNNY.mp4?token=1"></video>
          <a href="/videos/ep02.mkv">ep02</a>
          <meta property="og:video" content="//cdn.example.com/trailer.webm">
          <img src="cover.jpg"><img src="thumb.png">
          </body></html>"#;
        let list = extract_candidates(html, "https://example.com/post/1");
        assert!(list.iter().any(|c| c.url.contains("BIG_BUCK_BUNNY.mp4") && c.kind == "video"));
        assert!(list.iter().any(|c| c.url == "https://example.com/videos/ep02.mkv"));
        assert!(list.iter().any(|c| c.url.contains("trailer.webm")));
    }

    #[test]
    fn safe_name_strips_tricks() {
        assert_eq!(safe_name("https://x/a/hello%20world.mp4?x=1"), "hello world.mp4");
        assert_eq!(safe_name("https://x/a/..%2Fevil.mp4"), ".._evil.mp4");
    }

    #[test]
    fn site_extraction() {
        assert_eq!(site_of("https://www.bilibili.com/video/av1"), "bilibili.com");
    }
}
