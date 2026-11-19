use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use zip::ZipArchive;

use crate::db::VideoRecord;

const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts", "mpg", "mpeg",
    "3gp", "vob", "rmvb", "rm", "mts", "m2v", "asf", "f4v",
];
const ARCHIVE_EXT: &[&str] = &["zip", "cbz"];

#[derive(Debug, Clone, Default)]
pub struct MediaMeta {
    pub duration_sec: f64,
    pub width: i64,
    pub height: i64,
    pub codec_video: String,
    pub codec_audio: String,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    format: Option<ProbeFormat>,
    streams: Option<Vec<ProbeStream>>,
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| VIDEO_EXT.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn is_supported_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ARCHIVE_EXT.contains(&ext.to_ascii_lowercase().as_str()))
}

/// 依次尝试：显式配置 → PATH → 本机已知目录（MESAVAGE Tools）。
pub fn find_tool(exe: &str) -> Option<PathBuf> {
    let env_key = if exe == "ffmpeg" { "VIDEO_VAULT_FFMPEG" } else { "VIDEO_VAULT_FFPROBE" };
    if let Some(p) = std::env::var_os(env_key) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = which(exe) {
        return Some(p);
    }
    for base in [
        r"D:\MESAVAGE\Tools\ffmpeg",
        r"C:\ffmpeg\bin",
        r"C:\Program Files\ffmpeg\bin",
    ] {
        let p = PathBuf::from(base).join(exe);
        if p.is_file() {
            return Some(p);
        }
        let p = PathBuf::from(base).join(format!("{exe}.exe"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn which(exe: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in [exe.to_string(), format!("{exe}.exe")] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 用 ffprobe 取时长/分辨率/编码。失败不阻断入库（返回空元数据）。
pub fn probe(path: &Path) -> MediaMeta {
    let Some(ffprobe) = find_tool("ffprobe") else {
        return MediaMeta::default();
    };
    let output = Command::new(ffprobe)
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            &path.to_string_lossy(),
        ])
        .output();
    let Ok(output) = output else {
        return MediaMeta::default();
    };
    if !output.status.success() {
        return MediaMeta::default();
    }
    let parsed: ProbeOutput = match serde_json::from_slice(&output.stdout) {
        Ok(p) => p,
        Err(_) => return MediaMeta::default(),
    };
    let mut meta = MediaMeta::default();
    if let Some(fmt) = parsed.format {
        meta.duration_sec = fmt.duration.and_then(|s| s.parse().ok()).unwrap_or(0.0);
    }
    if let Some(streams) = parsed.streams {
        for s in streams {
            match s.codec_type.as_str() {
                "video" => {
                    meta.width = s.width.unwrap_or(0);
                    meta.height = s.height.unwrap_or(0);
                    meta.codec_video = s.codec_name.unwrap_or_default();
                    if meta.duration_sec <= 0.0 {
                        meta.duration_sec = s.duration.and_then(|d| d.parse().ok()).unwrap_or(0.0);
                    }
                }
                "audio" => {
                    if meta.codec_audio.is_empty() {
                        meta.codec_audio = s.codec_name.unwrap_or_default();
                    }
                }
                _ => {}
            }
        }
    }
    meta
}

fn file_meta(path: &Path) -> (i64, i64) {
    match fs::metadata(path) {
        Ok(m) => {
            let size = m.len() as i64;
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (size, mtime)
        }
        Err(_) => (0, 0),
    }
}

pub fn record_for_file(path: &Path) -> VideoRecord {
    let (size_bytes, mtime) = file_meta(path);
    let meta = probe(path);
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let container = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    VideoRecord {
        path: path.to_string_lossy().to_string(),
        file_name,
        container,
        size_bytes,
        mtime,
        duration_sec: meta.duration_sec,
        width: meta.width,
        height: meta.height,
        codec_video: meta.codec_video,
        codec_audio: meta.codec_audio,
        entry_index: 0,
        entry_count: 1,
    }
}

/// 压缩包内的视频条目：登记为 (archive_path, entry_index) 的独立记录。
/// 用 Central Directory 读文件名，不解压内容，千条压缩包也很快。
pub fn archive_records(path: &Path, entry_limit: usize) -> Result<Vec<VideoRecord>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("无法读取压缩包: {e}"))?;
    let (size_bytes, mtime) = file_meta(path);
    let container = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let archive_path = path.to_string_lossy().to_string();
    let mut names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(f) = archive.by_index(i) {
            if !f.is_dir() && is_video(Path::new(f.name())) {
                names.push(f.name().to_string());
            }
        }
    }
    names.sort_by(|a, b| natord::compare(a, b));
    let total = names.len();
    let mut out = Vec::new();
    for (idx, name) in names.into_iter().take(entry_limit).enumerate() {
        let file_name = format!(
            "{} › {}",
            path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
            Path::new(&name)
                .file_name()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        );
        out.push(VideoRecord {
            path: archive_path.clone(),
            file_name,
            container: container.clone(),
            size_bytes,
            mtime,
            duration_sec: 0.0,
            width: 0,
            height: 0,
            codec_video: String::new(),
            codec_audio: String::new(),
            entry_index: idx as i64,
            entry_count: total as i64,
        });
    }
    Ok(out)
}

/// 从压缩包里抽出指定序号的视频到临时目录（供播放器/抽帧使用）。
/// 返回临时文件路径，命名带条目序号，重名不冲突。
pub fn extract_entry(archive_path: &Path, entry_index: usize, cache_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let file = fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(f) = archive.by_index(i) {
            if !f.is_dir() && is_video(Path::new(f.name())) {
                names.push(f.name().to_string());
            }
        }
    }
    names.sort_by(|a, b| natord::compare(a, b));
    let name = names
        .get(entry_index)
        .ok_or_else(|| format!("压缩包内没有第 {} 个视频", entry_index + 1))?;
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4");
    let digest = blake3::hash(format!("{}|{}", archive_path.to_string_lossy(), entry_index).as_bytes());
    let target = cache_dir.join(format!("entry-{}-{}.{}", entry_index, digest.to_hex(), ext));
    if !target.is_file() {
        let mut src = archive.by_name(name).map_err(|e| e.to_string())?;
        let mut dst = fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut src, &mut dst).map_err(|e| e.to_string())?;
    }
    Ok(target)
}

/// 生成 4×4 接触表（contact sheet）：比单张封面信息量大得多，
/// 是视频库区别于漫画库的关键预览形态。
pub fn contact_sheet(source: &Path, out: &Path) -> Result<(), String> {
    let Some(ffmpeg) = find_tool("ffmpeg") else {
        return Err("未找到 ffmpeg，请在设置里指定路径".into());
    };
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let filter = "fps=1/45,scale=240:-2,tile=4x4:margin=2:padding=2";
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error", "-y",
            "-i", &source.to_string_lossy(),
            "-vf", filter,
            "-frames:v", "1",
            "-q:v", "4",
            &out.to_string_lossy(),
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() && out.is_file() {
        Ok(())
    } else {
        Err("ffmpeg 生成预览失败（可能是视频过短或格式不支持）".into())
    }
}
