//! 应用内文件编辑：重命名 / 移动 / 资源管理器定位。
//!
//! 设计取舍（为「快」服务）：
//! - 同盘改名/移动走 `fs::rename`，是元数据操作（毫秒级），不碰文件内容；
//! - 封面与接触表缓存按 **视频 id** 命名（`cover-{id}.jpg` / `mid-{id}.jpg` / `sheet-{id}`），
//!   所以改名、换目录之后缓存照样命中，不需要重抽 —— 这是「不卡顿」的关键；
//! - 只有跨卷（不同盘）才退化成流式复制 + 删源，并且保留源文件 mtime；
//! - 目标重名不覆盖：自动加 ` (2)`、` (3)`，跟资源管理器的手感一致。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const COPY_CHUNK: usize = 4 * 1024 * 1024;
/// Windows: ERROR_NOT_SAME_DEVICE
const ERR_NOT_SAME_DEVICE: i32 = 17;

/// 校验用户输入的文件名：不允许路径分隔符与 Windows 保留字符，保留扩展名。
pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("文件名不能为空".into());
    }
    if name.contains(['\\', '/']) {
        return Err("文件名里不能带路径分隔符".into());
    }
    if name.chars().any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("文件名不能包含 : * ? \" < > | 这些字符".into());
    }
    if name == "." || name == ".." {
        return Err("文件名不合法".into());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("文件名不能以点或空格结尾".into());
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("「{stem}」是 Windows 保留名，换一个"));
    }
    Ok(name.to_string())
}

/// 拆分文件名与扩展名（保留点，扩展名含点；无扩展名时返回空串）。
pub fn split_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// 目标目录里不重名的落盘名（同名自动 ` (2)`、` (3)`…）。
pub fn unique_path(dir: &Path, want: &str) -> PathBuf {
    let first = dir.join(want);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = split_ext(want);
    for n in 2..=9999 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}{ext}", std::process::id()))
}

/// 同目录改名。保持扩展名不变由前端保证（UI 只让改主名），这里只做校验。
pub fn rename_in_place(src: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = validate_name(new_name)?;
    let parent = src.parent().ok_or("取不到文件所在目录")?;
    let dest = parent.join(&name);
    if dest == src {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(format!("同目录下已经有「{name}」了"));
    }
    fs::rename(src, &dest).map_err(|e| format!("重命名失败：{e}"))?;
    Ok(dest)
}

/// 移动到目标目录：同卷 rename 秒成，跨卷退回复制 + 删源。
pub fn move_to(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    if !dest_dir.is_dir() {
        fs::create_dir_all(dest_dir).map_err(|e| format!("无法创建目标目录：{e}"))?;
    }
    // 已经在同一个目录里：什么都不做（否则 unique_path 会把自己当成重名，改出个 (2)）
    let same_dir = match (
        src.parent().and_then(|p| fs::canonicalize(p).ok()),
        fs::canonicalize(dest_dir).ok(),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    if same_dir {
        return Ok(src.to_path_buf());
    }
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or("取不到文件名")?;
    let dest = unique_path(dest_dir, &name);
    if dest == src {
        return Ok(dest);
    }
    match fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(e) if e.raw_os_error() == Some(ERR_NOT_SAME_DEVICE) => {
            copy_then_remove(src, &dest)?;
            Ok(dest)
        }
        Err(e) => Err(format!("移动失败：{e}")),
    }
}

/// 跨卷兜底：分块复制 → 校验大小 → 继承 mtime → 删源。任何一步失败都清掉半成品。
fn copy_then_remove(src: &Path, dest: &Path) -> Result<(), String> {
    let part = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    ));
    let _ = fs::remove_file(&part);
    let result = (|| -> Result<(), String> {
        let mut reader = fs::File::open(src).map_err(|e| format!("读源文件失败：{e}"))?;
        let mut writer = fs::File::create(&part).map_err(|e| format!("写目标文件失败：{e}"))?;
        let mut buf = vec![0u8; COPY_CHUNK];
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("读取中断：{e}"))?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).map_err(|e| format!("写入中断：{e}"))?;
        }
        writer.sync_all().map_err(|e| format!("刷盘失败：{e}"))?;
        drop(writer);
        let src_len = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let dst_len = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if src_len != dst_len {
            return Err(format!("复制校验失败：源 {src_len} 字节，落盘 {dst_len} 字节"));
        }
        // 保留源文件修改时间，库里按「文件修改时间」排序才不乱
        if let Ok(meta) = fs::metadata(src) {
            if let Ok(mtime) = meta.modified() {
                if let Ok(f) = fs::OpenOptions::new().write(true).open(&part) {
                    let _ = f.set_times(fs::FileTimes::new().set_modified(mtime));
                }
            }
        }
        fs::rename(&part, dest).map_err(|e| format!("落盘改名失败：{e}"))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            fs::remove_file(src).map_err(|e| format!("已复制到新位置，但删不掉源文件：{e}"))?;
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

/// 资源管理器里定位并选中该文件（不打开播放器）。
pub fn reveal(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("文件已不在原位，先扫描库".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打不开资源管理器：{e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("当前平台未实现".into())
    }
}

/// 文件当前 mtime（秒）；取不到返回 0。改名/移动后写回库，保持排序真实。
pub fn mtime_secs(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vv-fileops-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_name_rejects_illegal_input() {
        assert!(validate_name("  ").is_err());
        assert!(validate_name("a/b.mp4").is_err());
        assert!(validate_name("a\\b.mp4").is_err());
        assert!(validate_name("a:b.mp4").is_err());
        assert!(validate_name("结尾点.").is_err());
        assert!(validate_name("con.mp4").is_err(), "Windows 保留名");
        assert_eq!(validate_name("  正常名字.mp4  ").unwrap(), "正常名字.mp4");
    }

    #[test]
    fn split_ext_keeps_dot_and_handles_no_ext() {
        assert_eq!(split_ext("片名.mp4"), ("片名".into(), ".mp4".into()));
        assert_eq!(split_ext("a.b.c.mkv"), ("a.b.c".into(), ".mkv".into()));
        assert_eq!(split_ext("无扩展"), ("无扩展".into(), String::new()));
    }

    #[test]
    fn rename_in_place_moves_name_without_touching_bytes() {
        let dir = tmpdir("rename");
        let src = dir.join("旧名.mp4");
        fs::write(&src, b"payload").unwrap();
        let dest = rename_in_place(&src, "新名.mp4").unwrap();
        assert_eq!(dest, dir.join("新名.mp4"));
        assert!(!src.exists(), "旧文件应已消失");
        assert_eq!(fs::read(&dest).unwrap(), b"payload", "内容不能变");
        // 同目录撞名要拒绝，不能静默覆盖
        fs::write(dir.join("占位.mp4"), b"x").unwrap();
        let err = rename_in_place(&dest, "占位.mp4").unwrap_err();
        assert!(err.contains("已经有"), "{err}");
        // 改成同名 = 空操作
        assert_eq!(rename_in_place(&dest, "新名.mp4").unwrap(), dest);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn move_to_keeps_name_and_avoids_overwrite() {
        let root = tmpdir("move");
        let a = root.join("0-199");
        let b = root.join("200-399");
        fs::create_dir_all(&a).unwrap();
        let src = a.join("片.mp4");
        fs::write(&src, b"abc").unwrap();

        let moved = move_to(&src, &b).unwrap();
        assert_eq!(moved, b.join("片.mp4"));
        assert!(src.exists() == false);
        assert_eq!(fs::metadata(&moved).unwrap().len(), 3);

        // 目标已有同名：自动加 (2)，不覆盖
        fs::write(&src, b"new").unwrap();
        let again = move_to(&src, &b).unwrap();
        assert_eq!(again, b.join("片 (2).mp4"));
        assert_eq!(fs::read(b.join("片.mp4")).unwrap(), b"abc", "旧文件不能被覆盖");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn move_to_same_dir_is_noop() {
        let dir = tmpdir("noop");
        let src = dir.join("x.mp4");
        fs::write(&src, b"x").unwrap();
        let same = move_to(&src, &dir).unwrap();
        assert_eq!(same, src);
        assert!(src.exists());
        fs::remove_dir_all(dir).ok();
    }
}
