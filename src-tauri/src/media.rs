use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use zip::ZipArchive;

use crate::db::VideoRecord;

const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts", "mpg", "mpeg",
    "3gp", "vob", "rmvb", "rm", "mts", "m2v", "asf", "f4v",
];
const ARCHIVE_EXT: &[&str] = &["zip", "cbz"];

/// 入库路径统一用反斜杠：历史上同一文件被 `F:/x` 和 `F:\x` 登记成两条，
/// 这里在生成记录时就归一化，配合 db v6 迁移把老库洗干净。
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

/// 指纹取样块大小（首尾各读这么多）
const FP_CHUNK: u64 = 8 * 1024 * 1024;

/// 视频内容指纹：`blake3(大小 + 首 8MB + 尾 8MB)`。
///
/// 不读全量是因为库里动辄几个 GB 的合集，全量哈希要分钟级；同大小且首尾各 8MB
/// 完全一致的视频，内容相同的概率极高。代价是「只改了中间片段」的剪辑变体会
/// 被判成重复，所以上层只拿它来**提示**候选，删文件由用户确认。
///
/// 特意不把 mtime 算进去：跨盘复制会改 mtime，算进去就认不出重复了。
pub fn fingerprint(path: &Path, size: u64) -> Option<String> {
    if size == 0 {
        return None;
    }
    let mut f = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buf = vec![0u8; FP_CHUNK as usize];

    let n = f.read(&mut buf).ok()?;
    hasher.update(&buf[..n]);

    // 文件不超过两个块时，头部那一次已经把内容读全了
    if size > FP_CHUNK * 2 {
        f.seek(std::io::SeekFrom::Start(size - FP_CHUNK)).ok()?;
        let n = f.read(&mut buf).ok()?;
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().to_hex().to_string())
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MediaMeta {
    pub duration_sec: f64,
    pub width: i64,
    pub height: i64,
    pub codec_video: String,
    pub codec_audio: String,
    /// 拼接重编码时对齐到第一段的帧率（0 = 未知）
    pub fps: f64,
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
    r_frame_rate: Option<String>,
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

/// 设置页里手工指定的工具路径。edition 2024 下 set_var 是 unsafe，
/// 所以用进程内单例保存，find_tool 优先查它。
static TOOL_PATHS: OnceLock<Mutex<(Option<PathBuf>, Option<PathBuf>)>> = OnceLock::new();

fn tool_paths() -> &'static Mutex<(Option<PathBuf>, Option<PathBuf>)> {
    TOOL_PATHS.get_or_init(|| Mutex::new((None, None)))
}

/// 由设置页驱动：每次启动 / 保存设置时同步一次。
pub fn set_tool_paths(ffmpeg: Option<&str>, ffprobe: Option<&str>) {
    let pick = |s: Option<&str>| s.map(PathBuf::from).filter(|p| p.is_file());
    let Ok(mut paths) = tool_paths().lock() else {
        return;
    };
    paths.0 = pick(ffmpeg);
    paths.1 = pick(ffprobe);
}

fn configured_tool(exe: &str) -> Option<PathBuf> {
    let paths = tool_paths().lock().ok()?;
    let slot = if exe == "ffmpeg" { &paths.0 } else { &paths.1 };
    slot.clone()
}

/// 依次尝试：设置页指定 → 环境变量 → PATH → 本机已知目录（MESAVAGE Tools）。
pub fn find_tool(exe: &str) -> Option<PathBuf> {
    if let Some(p) = configured_tool(exe) {
        return Some(p);
    }
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

/// 解析 ffmpeg 的分数写法（如 "30000/1001" → 29.97）；分母为 0 或解析失败返回 None。
pub(crate) fn parse_fraction(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let num: f64 = num.trim().parse().ok()?;
    let den: f64 = den.trim().parse().ok()?;
    if den > 0.0 {
        Some(num / den)
    } else {
        None
    }
}

/// 解码 canvas.toDataURL 产出的 base64 data URL（仅限 image/*）。
/// 不为此引 base64 依赖，自带一个最小解码器（只解码、不编码）。
pub fn decode_data_url(s: &str) -> Result<Vec<u8>, String> {
    let rest = s.strip_prefix("data:").ok_or("不是 data URL")?;
    let (meta, data) = rest.split_once(',').ok_or("data URL 缺少逗号")?;
    if !meta.contains("base64") {
        return Err("仅支持 base64 编码的 data URL".into());
    }
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut nbits = 0;
    for &c in data.as_bytes() {
        if c == b'=' {
            break;
        }
        let v = val(c).ok_or("data URL 含非法 base64 字符")?;
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((acc >> nbits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// 用 ffprobe 取时长/分辨率/编码。失败不阻断入库（返回空元数据）。
pub fn probe(path: &Path) -> MediaMeta {
    let Some(ffprobe) = find_tool("ffprobe") else {
        return MediaMeta::default();
    };
    let output = crate::proc::quiet(ffprobe)
        .args([
            "-v", "quiet",
            "-print_format", "json",
            // 只要这几个字段：整份 JSON 在字幕轨很多的文件上会大出几十倍，拖慢扫描
            "-show_entries", "format=duration:stream=codec_type,codec_name,width,height,duration,r_frame_rate",
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
                    if meta.fps <= 0.0 {
                        meta.fps = s.r_frame_rate.as_deref().and_then(parse_fraction).unwrap_or(0.0);
                    }
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

/// 只取大小与修改时间，用来判断文件是否变过（不探测内容）。
pub fn file_stamp(path: &Path) -> (i64, i64) {
    file_meta(path)
}

/// 只登记文件系统信息，不调用 ffprobe。
/// 扫描时对「元数据已经齐全且文件没变」的条目用它，几千条也只是一次 stat。
pub fn record_stub(path: &Path) -> VideoRecord {
    let (size_bytes, mtime) = file_meta(path);
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
        path: path_key(path),
        file_name,
        container,
        size_bytes,
        mtime,
        duration_sec: 0.0,
        width: 0,
        height: 0,
        codec_video: String::new(),
        codec_audio: String::new(),
        entry_index: 0,
        entry_count: 1,
    }
}

/// 登记并探测。探测失败也不阻断入库（元数据留空，下次扫描再补）。
pub fn record_for_file(path: &Path) -> VideoRecord {
    let mut rec = record_stub(path);
    let meta = probe(path);
    rec.duration_sec = meta.duration_sec;
    rec.width = meta.width;
    rec.height = meta.height;
    rec.codec_video = meta.codec_video;
    rec.codec_audio = meta.codec_audio;
    rec
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
    let archive_path = path_key(path);
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

/// 接触表可选格数与排布：格数必须能精确铺满矩形，否则 tile 会拿黑块补空。
/// 横屏源用宽排布、竖屏源用高排布，让成品图比例尽量贴近源视频。
const SHEET_LAYOUTS_WIDE: &[(usize, u32, u32)] = &[
    (1, 1, 1),
    (2, 2, 1),
    (4, 2, 2),
    (6, 3, 2),
    (8, 4, 2),
    (12, 4, 3),
    (16, 4, 4),
];
const SHEET_LAYOUTS_TALL: &[(usize, u32, u32)] = &[
    (1, 1, 1),
    (2, 1, 2),
    (4, 2, 2),
    (6, 2, 3),
    (8, 2, 4),
    (12, 3, 4),
    (16, 4, 4),
];

/// 按时长挑接触表排布：理想「每 45 秒一帧」向下取到最近的可整除格数。
/// 返回（帧数, 列, 行, fps）；fps 带 +1 帧超采样，防源比元数据略短时尾帧被丢、tile 补黑。
pub fn sheet_layout(duration_sec: f64, portrait: bool) -> (usize, u32, u32, f64) {
    let ideal = if duration_sec > 0.0 {
        ((duration_sec / 45.0).ceil() as usize).clamp(1, 16)
    } else {
        1
    };
    let table: &[(usize, u32, u32)] = if portrait { SHEET_LAYOUTS_TALL } else { SHEET_LAYOUTS_WIDE };
    let (count, cols, rows) = table
        .iter()
        .rev()
        .find(|(n, _, _)| *n <= ideal)
        .copied()
        .unwrap_or((1, 1, 1));
    let fps = (count + 1) as f64 / duration_sec.max(0.1);
    (count, cols, rows, fps)
}

/// 生成接触表（contact sheet）：比单张封面信息量大得多，
/// 是视频库区别于漫画库的关键预览形态。
/// 格数按时长在 1/2/4/6/8/12/16 里挑（可整除才不留黑空），横竖屏各有一套排布。
pub fn contact_sheet(source: &Path, out: &Path) -> Result<(), String> {
    let Some(ffmpeg) = find_tool("ffmpeg") else {
        return Err("未找到 ffmpeg，请在设置里指定路径".into());
    };
    let meta = probe(source);
    let duration = if meta.duration_sec > 0.0 { meta.duration_sec } else { 45.0 };
    let portrait = meta.width > 0 && meta.height > meta.width;
    let (_, cols, rows, fps) = sheet_layout(duration, portrait);
    let filter = format!("fps={fps:.6},scale=240:-2,tile={cols}x{rows}:margin=2:padding=2");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let status = crate::proc::quiet(ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error", "-y",
            "-i", &source.to_string_lossy(),
            "-vf", &filter,
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

/// 抽单帧当封面：默认取视频中间帧（输入侧 seek + 默认 accurate_seek，逐帧精确落点）。
/// 压缩包条目由调用方先 resolve 出真实文件再传进来。
pub fn extract_frame(source: &Path, at_sec: f64, out: &Path) -> Result<(), String> {
    let Some(ffmpeg) = find_tool("ffmpeg") else {
        return Err("未找到 ffmpeg，请在设置里指定路径".into());
    };
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let ss = format!("{at_sec:.3}");
    let status = crate::proc::quiet(ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error", "-y",
            "-ss", &ss,
            "-i", &source.to_string_lossy(),
            "-frames:v", "1",
            // 480 宽足够卡片与详情用，避免 4K 原帧一张几 MB
            "-vf", "scale=480:-2:flags=bicubic",
            "-q:v", "3",
            &out.to_string_lossy(),
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() && out.is_file() {
        Ok(())
    } else {
        Err("ffmpeg 抽帧失败（可能是时间点超出时长或格式不支持）".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_data_url_roundtrip_known_vectors() {
        // "Hello" 的 base64
        let out = decode_data_url("data:image/jpeg;base64,SGVsbG8=").unwrap();
        assert_eq!(out, b"Hello");
        // 带 padding 的 "a"
        let out = decode_data_url("data:image/png;base64,YQ==").unwrap();
        assert_eq!(out, b"a");
        // 1×1 白色 JPEG 的真实 data URL 头几字节（FF D8 FF = JPEG SOI）
        let out = decode_data_url("data:image/jpeg;base64,/9j/4AAQSkZJRg==").unwrap();
        assert_eq!(&out[..3], &[0xFF, 0xD8, 0xFF]);

        assert!(decode_data_url("http://x/y.png").is_err(), "非 data URL 应拒绝");
        assert!(decode_data_url("data:text/plain,raw").is_err(), "非 base64 应拒绝");
        assert!(decode_data_url("data:image/jpeg;base64,@@@").is_err(), "非法字符应拒绝");
    }

    #[test]
    fn sheet_layout_follows_duration_and_orientation() {
        // 短视频：1 帧 1×1（绝无黑空）；fps 是 (帧数+1)/时长的超采样值
        assert_eq!(sheet_layout(2.0, false), (1, 1, 1, 1.0));
        assert_eq!(sheet_layout(45.0, false), (1, 1, 1, 2.0 / 45.0));
        // 100s → 理想 3 帧 → 向下取 2（2×1 横 / 1×2 竖）
        assert_eq!(sheet_layout(100.0, false), (2, 2, 1, 3.0 / 100.0));
        assert_eq!(sheet_layout(100.0, true), (2, 1, 2, 3.0 / 100.0));
        // 200s → 理想 5 → 4（2×2）；300s → 理想 7 → 6（3×2）；400s → 理想 9 → 8（4×2）
        assert_eq!(sheet_layout(200.0, false), (4, 2, 2, 5.0 / 200.0));
        assert_eq!(sheet_layout(300.0, false), (6, 3, 2, 7.0 / 300.0));
        assert_eq!(sheet_layout(300.0, true), (6, 2, 3, 7.0 / 300.0));
        assert_eq!(sheet_layout(400.0, false), (8, 4, 2, 9.0 / 400.0));
        assert_eq!(sheet_layout(400.0, true), (8, 2, 4, 9.0 / 400.0));
        // 660s → 理想 15 → 12（4×3）；700s → 理想 16 → 封顶 16（4×4）
        assert_eq!(sheet_layout(660.0, false), (12, 4, 3, 13.0 / 660.0));
        assert_eq!(sheet_layout(700.0, false), (16, 4, 4, 17.0 / 700.0));
        assert_eq!(sheet_layout(1200.0, false), (16, 4, 4, 17.0 / 1200.0));
        assert_eq!(sheet_layout(1200.0, true), (16, 4, 4, 17.0 / 1200.0));
        // 时长读不出：保底 1×1（时长按 0.1 兜底 → fps=2/0.1=20）
        assert_eq!(sheet_layout(0.0, false), (1, 1, 1, 20.0));
    }

    /// 真实端到端：100s 测试片 → 接触表应为 2×1（两帧并排，无黑空）；2s 片应为 1×1
    #[test]
    fn real_ffmpeg_contact_sheet_adapts_tile_count() {
        let Some(ffmpeg) = find_tool("ffmpeg") else {
            eprintln!("SKIP: 本机没有 ffmpeg，跳过接触表端到端");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "vv-sheet-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str, dur: u32| {
            let p = dir.join(name);
            let st = crate::proc::quiet(&ffmpeg)
                .args([
                    "-hide_banner", "-loglevel", "error", "-y",
                    "-f", "lavfi", "-i", &format!("testsrc=size=320x240:rate=15:duration={dur}"),
                    "-c:v", "libx264", "-preset", "ultrafast",
                    &p.to_string_lossy(),
                ])
                .status()
                .expect("ffmpeg 造测试片失败");
            assert!(st.success());
            p
        };
        let long = mk("long.mp4", 100);
        let sheet = dir.join("sheet-long.jpg");
        contact_sheet(&long, &sheet).unwrap();
        let meta = probe(&sheet);
        // 2×1：宽 ≈ 2×240（tile margin/padding ±16），高 ≈ 1×180
        assert!(
            (meta.width as i64 - 480).abs() <= 16 && (meta.height as i64 - 180).abs() <= 16,
            "100s 片接触表应为 2×1（约 480×180），实测 {}×{}",
            meta.width,
            meta.height
        );

        let short = mk("short.mp4", 2);
        let sheet2 = dir.join("sheet-short.jpg");
        contact_sheet(&short, &sheet2).unwrap();
        let meta = probe(&sheet2);
        assert!(
            (meta.width as i64 - 240).abs() <= 16 && (meta.height as i64 - 180).abs() <= 16,
            "2s 片接触表应为 1×1（约 240×180），实测 {}×{}",
            meta.width,
            meta.height
        );

        fs::remove_dir_all(dir).ok();
    }

    /// 真实端到端：ffmpeg 造 2 秒测试片 → 抽中间帧，产物应是有效 JPEG。
    #[test]
    fn real_ffmpeg_extracts_middle_frame() {
        let Some(ffmpeg) = find_tool("ffmpeg") else {
            eprintln!("SKIP: 本机没有 ffmpeg，跳过抽帧端到端");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "vv-cover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.mp4");
        let st = crate::proc::quiet(&ffmpeg)
            .args([
                "-hide_banner", "-loglevel", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=2",
                "-c:v", "libx264", "-preset", "ultrafast",
                &src.to_string_lossy(),
            ])
            .status()
            .expect("ffmpeg 造测试片失败");
        assert!(st.success());

        let meta = probe(&src);
        assert!(meta.duration_sec > 1.0, "测试片时长应约 2 秒");
        let out = dir.join("mid.jpg");
        extract_frame(&src, meta.duration_sec / 2.0, &out).unwrap();
        assert!(out.is_file());
        let bytes = fs::read(&out).unwrap();
        assert!(bytes.len() > 500, "JPEG 应有实质内容，实测 {} 字节", bytes.len());
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "应为 JPEG（SOI 标记）");

        // 超界时间点应报错而不是留半截文件
        let bad = dir.join("bad.jpg");
        assert!(extract_frame(&src, 999.0, &bad).is_err());

        fs::remove_dir_all(dir).ok();
    }
}
