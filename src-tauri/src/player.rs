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
///
/// 顺带做一件扫描阶段做不了的事：压缩包条目在入库时只有文件名，
/// 这里既然已经把真实文件解出来了，就补一次时长/分辨率。
pub fn backfill_meta(ctx: &LibraryContext, id: i64, source: &Path) {
    let Ok(video) = get_video(&ctx.conn, id) else { return };
    if video.duration_sec > 0.0 {
        return;
    }
    if !source.is_file() {
        return;
    }
    let meta = media::probe(source);
    let _ = crate::db::backfill_probe(
        &ctx.conn,
        id,
        meta.duration_sec,
        meta.width,
        meta.height,
        &meta.codec_video,
        &meta.codec_audio,
    );
}
pub fn resolve(ctx: &LibraryContext, id: i64) -> Result<PlaybackInfo, String> {
    let video = get_video(&ctx.conn, id)?;
    let path = PathBuf::from(&video.path);
    let cache = crate::db::LibraryContext::vault_dir(&ctx.root).join("cache");
    let start_sec = video.progress_sec;
    // 压缩包条目的 entry_index 是从 0 开始的——第 0 条既是「第一集」也是索引 0，
    // 不能靠 entry_index==0 判断「这是个普通文件」，否则第一集会直接把 .cbz 交给播放器。
    let is_archive = media::is_supported_archive(&path);
    if !is_archive && path.is_file() {
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
    if is_archive && path.is_file() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::VideoRecord;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vv-player-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    fn archive_record(zip: &Path, entry_index: i64) -> VideoRecord {
        VideoRecord {
            path: zip.to_string_lossy().to_string(),
            file_name: format!("合集.cbz › 第{:02}集.mp4", entry_index + 1),
            container: "cbz".into(),
            size_bytes: 2048,
            mtime: 100,
            duration_sec: 0.0,
            width: 0,
            height: 0,
            codec_video: String::new(),
            codec_audio: String::new(),
            entry_index,
            entry_count: 2,
        }
    }

    /// 回归：压缩包第 0 集也是 entry_index==0，不能被当成「普通文件」直接播 .cbz。
    #[test]
    fn archive_first_entry_extracts_instead_of_playing_archive_itself() {
        let dir = temp_dir("entry0");
        let zip_path = dir.join("合集.cbz");
        make_zip(&zip_path, &[("第01集.mp4", b"aaa"), ("第02集.mp4", b"bbb")]);

        let ctx = crate::db::open_library(&dir.join("lib")).unwrap();
        crate::db::upsert_video(&ctx.conn, &archive_record(&zip_path, 0)).unwrap();
        crate::db::upsert_video(&ctx.conn, &archive_record(&zip_path, 1)).unwrap();
        let id0: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE entry_index=0", [], |r| r.get(0))
            .unwrap();
        let id1: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE entry_index=1", [], |r| r.get(0))
            .unwrap();

        let first = resolve(&ctx, id0).unwrap();
        assert_eq!(first.source, "extracted", "第 0 集必须解压，不能直接播压缩包");
        assert_ne!(Path::new(&first.media_path), zip_path);
        assert!(first.media_path.ends_with(".mp4"));
        assert_eq!(std::fs::read(&first.media_path).unwrap(), b"aaa");

        let second = resolve(&ctx, id1).unwrap();
        assert_eq!(second.source, "extracted");
        assert_eq!(std::fs::read(&second.media_path).unwrap(), b"bbb");

        // 补元数据：解出来的字节不是真视频，ffprobe 取不到时长，但不能写坏已有数据
        backfill_meta(&ctx, id0, Path::new(&first.media_path));
        assert_eq!(crate::db::get_video(&ctx.conn, id0).unwrap().duration_sec, 0.0);

        drop(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 普通视频文件（非压缩包）仍走浏览器直播，不受上面修复影响。
    #[test]
    fn plain_video_file_still_plays_directly() {
        let dir = temp_dir("plain");
        let mp4 = dir.join("单个.mp4");
        std::fs::write(&mp4, b"xyz").unwrap();

        let ctx = crate::db::open_library(&dir.join("lib")).unwrap();
        let mut rec = archive_record(&mp4, 0);
        rec.path = mp4.to_string_lossy().to_string();
        rec.container = "mp4".into();
        rec.entry_count = 1;
        crate::db::upsert_video(&ctx.conn, &rec).unwrap();
        let id: i64 = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get(0)).unwrap();

        let info = resolve(&ctx, id).unwrap();
        assert_eq!(info.source, "browser");
        assert_eq!(Path::new(&info.media_path), mp4);

        drop(ctx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
