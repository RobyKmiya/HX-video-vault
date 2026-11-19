use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::db::{get_video, LibraryContext};
use crate::media;
use crate::models::VideoItem;

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackInfo {
    pub video: VideoItem,
    /// browser = 前端 <video> 可直接播；extracted = 从压缩包解到缓存后可播；external = 需外挂播放器
    pub source: String,
    /// 交给前端 convertFileSrc 的本地路径（browser / extracted）
    pub media_path: String,
    pub start_sec: f64,
    pub player_hint: String,
}

/// 解析一条视频的播放来源。压缩包条目按需解压单个文件到缓存，
/// 不整包解包——这是影匣区别于「先解压再看」的关键体验。
pub fn resolve(ctx: &LibraryContext, id: i64) -> Result<PlaybackInfo, String> {
    let video = get_video(&ctx.conn, id)?;
    let path = PathBuf::from(&video.path);
    let cache = crate::db::LibraryContext::vault_dir(&ctx.root).join("cache");
    let start_sec = video.progress_sec;
    if video.entry_index == 0 && path.is_file() {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let browser = matches!(ext.as_str(), "mp4" | "webm" | "ogg" | "mov" | "m4v");
        return Ok(PlaybackInfo {
            video,
            source: if browser { "browser" } else { "external" }.into(),
            media_path: path.to_string_lossy().to_string(),
            start_sec,
            player_hint: String::new(),
        });
    }
    if video.entry_index > 0 && path.is_file() {
        let extracted = media::extract_entry(&path, video.entry_index as usize, &cache)?;
        let ext = extracted
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let browser = matches!(ext.as_str(), "mp4" | "webm" | "ogg" | "mov" | "m4v");
        return Ok(PlaybackInfo {
            video,
            source: if browser { "extracted" } else { "external" }.into(),
            media_path: extracted.to_string_lossy().to_string(),
            start_sec,
            player_hint: String::new(),
        });
    }
    Ok(PlaybackInfo {
        video,
        source: "missing".into(),
        media_path: String::new(),
        start_sec,
        player_hint: "文件已移动或删除，可在详情里重新定位".into(),
    })
}

pub(crate) fn find_vlc_public() -> Option<PathBuf> { find_vlc() }

fn find_vlc() -> Option<PathBuf> {
    for p in [
        r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
        r"C:\Program Files\VideoLAN\VLC\vlc.exe",
        r"D:\TOOLS\VLC\vlc.exe",
    ] {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 冷门格式与外链：调用本机 VLC（实测 3.0.21）从上次位置续播。
/// 这里只把 VLC 当黑盒，不复制它的解码/字幕实现。
pub fn launch_external(path: &Path, start_sec: f64) -> Result<String, String> {
    if let Some(vlc) = find_vlc() {
        let secs = start_sec.max(0.0) as u64;
        Command::new(vlc)
            .arg(format!("--start-time={secs}"))
            .arg("--no-video-title-show")
            .arg(path)
            .spawn()
            .map_err(|e| format!("无法启动 VLC: {e}"))?;
        return Ok("vlc".into());
    }
    if let Some(mpv) = media::find_tool("mpv") {
        let secs = start_sec.max(0.0);
        Command::new(mpv)
            .arg(format!("--start={secs}"))
            .arg(path)
            .spawn()
            .map_err(|e| format!("无法启动 mpv: {e}"))?;
        return Ok("mpv".into());
    }
    Err("未找到 VLC 或 mpv，请安装其一或把视频转为 MP4".into())
}
