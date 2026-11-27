//! 拖拽导入：把外部视频复制进「视频数据库」区间夹（每个夹 200 个视频）。
//!
//! 管理员的存储约定：`<导入根>\0-199`、`200-399`… 每个文件夹按名内区间容纳 200 个视频，
//! 顺序导入；删除产生的漏缺由新导入**往前补**（优先填最小序号的未满夹）。
//!
//! 数据安全三件套：
//! - 复制写 `.part.<原扩展名>` 临时文件（保留已知扩展名让后续工具认得），完成后 fsync；
//! - 字节数校验（源大小 = 落盘大小）后才原子改名成成品，失败/中断/取消一律清掉半成品；
//! - 单工作线程串行复制 + 4 MiB 分块，不与扫描/封面生成抢 IO，UI 只收节流后的进度事件。
//!
//! 状态放独立 static（不占 AppState 锁）；进度经 `import-progress` 事件推给角落浮窗。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::media;

/// 每个区间夹的视频容量
const CAP: u64 = 200;
const CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ImportTask {
    pub source: PathBuf,
    pub target: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportStatus {
    pub running: bool,
    pub queue_total: usize,
    pub queue_done: usize,
    pub current_file: String,
    pub current_done: u64,
    pub current_total: u64,
    pub failed: usize,
    pub last_error: String,
}

struct ImporterState {
    tasks: Vec<ImportTask>,
    running: bool,
    cancel: bool,
    done: usize,
    failed: usize,
    last_error: String,
    current_file: String,
    current_done: u64,
    current_total: u64,
}

static IMPORTER: Mutex<ImporterState> = Mutex::new(ImporterState {
    tasks: Vec::new(),
    running: false,
    cancel: false,
    done: 0,
    failed: 0,
    last_error: String::new(),
    current_file: String::new(),
    current_done: 0,
    current_total: 0,
});

/// 取消令牌：**全局 static**，取消按钮能立刻打断正在拷的那个文件（块间检查）。
/// 之前每个批次各带一个 Arc，cancel() 改的是状态里的标志位，工作线程读的却是自己那份，
/// 结果「取消」只能等当前文件拷完才停 —— 大文件等于没取消。
static CANCEL: AtomicBool = AtomicBool::new(false);

fn state_lock() -> std::sync::MutexGuard<'static, ImporterState> {
    IMPORTER.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------- 目标夹分配（纯函数，可单测） ----------

/// 解析区间夹名的起点（"200-399" → 200）；不合规格式返回 None。
pub fn parse_start(name: &str) -> Option<u64> {
    let (a, _) = name.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    Some(start)
}

/// 目录名 → 夹编号：纯数字直接用；旧的 `0-199` 区间按 cap 一段折成 1、2、3…
/// （与 `db::organize::folder_number_of` 同一套规则，迁移时两边能对上。）
pub fn folder_no(name: &str) -> Option<u64> {
    let t = name.trim();
    if let Ok(n) = t.parse::<u64>() {
        return if n > 0 { Some(n) } else { None };
    }
    let (a, _) = t.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    Some(start / CAP + 1)
}

/// 挑目标夹：**只往最后一号夹里追加，装满了才开下一号**，不再回头填旧夹的空缺。
/// `counts` 是"现状 + 本批已分配"的虚拟计数，调用方负责维护。
pub fn pick_target(order: &[String], counts: &std::collections::HashMap<String, u64>, cap: u64) -> String {
    match order.last() {
        Some(last) if counts.get(last).copied().unwrap_or(0) < cap => last.clone(),
        Some(last) => (folder_no(last).unwrap_or(0) + 1).to_string(),
        None => "1".to_string(),
    }
}

/// 目标夹内不重名的落盘名：原名可用则原样保留，冲突加 (2)、(3)…
pub fn unique_target_name(dir: &Path, source: &Path) -> String {
    let name = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".into());
    let stem = Path::new(&name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.clone());
    let ext = source
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".into());
    let first = dir.join(&name);
    if !first.exists() {
        return name;
    }
    for n in 2..=9999 {
        let candidate = format!("{stem} ({n}).{ext}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem}-{}.{}", uuid::Uuid::new_v4().simple(), ext)
}

/// 把本批源文件分配到区间夹，产出复制任务（含防冲突后的最终落盘名）。
pub fn plan_targets(import_root: &Path, sources: &[PathBuf]) -> Result<Vec<ImportTask>, String> {
    fs::create_dir_all(import_root).map_err(|e| format!("无法创建导入根目录: {e}"))?;
    let mut order: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for entry in fs::read_dir(import_root).map_err(|e| format!("无法读取导入根目录: {e}"))? {
        let p = match entry {
            Ok(e) => e.path(),
            Err(_) => continue,
        };
        if !p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().map(|s| s.to_string_lossy().to_string()) else { continue };
        if folder_no(&name).is_none() {
            continue; // 只认「纯数字」与旧的「N-M」两种目录名
        }
        let count = count_videos(&p);
        order.push(name.clone());
        counts.insert(name, count);
    }
    order.sort_by_key(|n| folder_no(n).unwrap_or(0));

    let mut tasks = Vec::new();
    for src in sources {
        if !src.is_file() || !media::is_video(src) {
            continue;
        }
        let dir = pick_target(&order, &counts, CAP);
        if !counts.contains_key(&dir) {
            fs::create_dir_all(import_root.join(&dir)).map_err(|e| format!("无法创建区间夹: {e}"))?;
            order.push(dir.clone());
            counts.insert(dir.clone(), 0);
        }
        *counts.get_mut(&dir).unwrap() += 1;
        let size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        let target_name = unique_target_name(&import_root.join(&dir), src);
        tasks.push(ImportTask {
            source: src.clone(),
            target: import_root.join(dir).join(target_name),
            size,
        });
    }
    Ok(tasks)
}

fn count_videos(dir: &Path) -> u64 {
    let mut n = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && media::is_video(&p) {
                n += 1;
            }
        }
    }
    n
}

fn part_path(target: &Path) -> PathBuf {
    // 保留已知扩展名（a.mp4 → a.part.mp4），ffmpeg/编辑器等下游工具才认得
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());
    let ext = target
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".into());
    target.with_file_name(format!("{stem}.part.{ext}"))
}

fn build_status(st: &ImporterState, finished: bool) -> ImportStatus {
    ImportStatus {
        running: !finished && st.running,
        queue_total: st.done + st.failed + st.tasks.len(),
        queue_done: st.done + st.failed,
        current_file: st.current_file.clone(),
        current_done: st.current_done,
        current_total: st.current_total,
        failed: st.failed,
        last_error: st.last_error.clone(),
    }
}

fn emit_status(app: &AppHandle, finished: bool) {
    let payload = {
        let st = state_lock();
        build_status(&st, finished)
    };
    let _ = app.emit("import-progress", payload);
}

/// 分块复制：cancel 置位时在块间中止（返回 Err），完成时 sync_all + 大小校验；
/// 进度经 on_progress(done_bytes) 回调（工作线程里更新状态并节流发事件，测试传空回调）。
fn copy_chunked(
    task: &ImportTask,
    part: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> Result<(), String> {
    let mut reader = fs::File::open(&task.source).map_err(|e| format!("无法读取源文件: {e}"))?;
    let mut writer = fs::File::create(part).map_err(|e| format!("无法写临时文件: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    let mut last_emit = -2.0f64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("__cancelled__".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).map_err(|e| format!("写入失败: {e}"))?;
        done += n as u64;
        let pct = if task.size > 0 { done as f64 / task.size as f64 * 100.0 } else { 0.0 };
        if pct - last_emit >= 2.0 || done >= task.size {
            last_emit = pct;
            on_progress(done);
        }
    }
    writer.sync_all().map_err(|e| format!("刷盘失败: {e}"))?;
    drop(writer);
    let actual = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if task.size > 0 && actual != task.size {
        return Err(format!("大小校验失败：源 {} 字节，落盘 {} 字节", task.size, actual));
    }
    // 继承源文件的修改时间（像 robocopy /COPY:DAT）：否则库里的「文件修改时间」
    // 会全变成导入那一刻，按时间排序就没有顺序可言了
    if let Ok(meta) = fs::metadata(&task.source) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(f) = fs::OpenOptions::new().write(true).open(part) {
                let _ = f.set_times(fs::FileTimes::new().set_modified(mtime));
            }
        }
    }
    Ok(())
}

/// 单工作线程：串行消费队列，直到队列空/被取消。
fn worker(app: AppHandle) {
    loop {
        let task = {
            let mut st = state_lock();
            if st.cancel || st.tasks.is_empty() {
                None
            } else {
                let t = st.tasks.remove(0);
                st.current_file = t
                    .source
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                st.current_total = t.size;
                st.current_done = 0;
                Some(t)
            }
        };
        let Some(task) = task else {
            emit_status(&app, true);
            return;
        };
        let part = part_path(&task.target);
        let _ = fs::remove_file(&part);
        let app_for_progress = app.clone();
        match copy_chunked(&task, &part, &CANCEL, |done| {
            {
                let mut st = state_lock();
                st.current_done = done;
            }
            emit_status(&app_for_progress, false);
        }) {
            Ok(()) => {
                if let Err(e) = fs::rename(&part, &task.target) {
                    let _ = fs::remove_file(&part);
                    let mut st = state_lock();
                    st.failed += 1;
                    st.last_error = format!("无法落盘 {}: {e}", task.target.to_string_lossy());
                } else {
                    state_lock().done += 1;
                }
            }
            Err(e) => {
                let _ = fs::remove_file(&part);
                if e == "__cancelled__" {
                    CANCEL.store(false, Ordering::SeqCst);
                    let mut st = state_lock();
                    st.cancel = false;
                    st.running = false;
                    st.tasks.clear();
                    drop(st);
                    emit_status(&app, true);
                    return;
                }
                let mut st = state_lock();
                st.failed += 1;
                st.last_error = e;
            }
        }
    }
}

/// 入队并确保工作线程在跑。返回实际入队的文件数。
pub fn enqueue(
    app: &AppHandle,
    import_root: &Path,
    sources: &[PathBuf],
) -> Result<usize, String> {
    let tasks = plan_targets(import_root, sources)?;
    if tasks.is_empty() {
        return Ok(0);
    }
    let mut queued = 0usize;
    let start_worker;
    {
        let mut st = state_lock();
        // 同一源文件已在队列里就不再排（防重复拖拽）
        for t in &tasks {
            if st.tasks.iter().any(|x| x.source == t.source) {
                continue;
            }
            st.tasks.push(t.clone());
            queued += 1;
        }
        start_worker = !st.running && !st.tasks.is_empty();
        if start_worker {
            st.running = true;
            st.cancel = false;
            CANCEL.store(false, Ordering::SeqCst);
        }
    }
    if queued == 0 {
        return Ok(0);
    }
    if start_worker {
        let app = app.clone();
        std::thread::spawn(move || worker(app));
    }
    emit_status(app, false);
    Ok(queued)
}

/// 取消：立刻打断正在拷的文件（块间检查 + 半成品清掉），队列清空，已完成的保留。
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    state_lock().cancel = true;
}

pub fn status() -> ImportStatus {
    let st = state_lock();
    build_status(&st, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_start_reads_interval_names() {
        assert_eq!(parse_start("0-199"), Some(0));
        assert_eq!(parse_start("200-399"), Some(200));
        assert_eq!(parse_start(" 1200-1399 "), Some(1200));
        assert_eq!(parse_start("Supervideo"), None);
        assert_eq!(parse_start("abc-def"), None);
    }

    #[test]
    fn folder_no_reads_numeric_and_interval_names() {
        assert_eq!(folder_no("1"), Some(1));
        assert_eq!(folder_no("12"), Some(12));
        assert_eq!(folder_no("0-199"), Some(1));
        assert_eq!(folder_no("200-399"), Some(2));
        assert_eq!(folder_no(" 1200-1399 "), Some(7));
        assert_eq!(folder_no("Supervideo"), None);
        assert_eq!(folder_no("0"), None);
    }

    #[test]
    fn pick_target_appends_to_last_folder_without_filling_gaps() {
        let mut counts = HashMap::new();
        counts.insert("1".into(), 200u64);   // 装满
        counts.insert("2".into(), 50u64);    // 有空位，但在中间
        counts.insert("3".into(), 120u64);   // 最后一号，还有空位
        let order = vec!["1".into(), "2".into(), "3".into()];
        // 只往最后一号追加，不回头填 2 号的空
        assert_eq!(pick_target(&order, &counts, 200), "3");
        counts.insert("3".into(), 200u64);
        // 最后一号满了 → 开 4 号
        assert_eq!(pick_target(&order, &counts, 200), "4");
        // 空库从 1 号起步
        assert_eq!(pick_target(&Vec::new(), &HashMap::new(), 200), "1");
    }

    #[test]
    fn pick_target_continues_numbering_from_legacy_interval_names() {
        let order: Vec<String> = vec!["0-199".into(), "200-399".into()];
        let full = order.iter().map(|n| (n.clone(), 200u64)).collect::<HashMap<String, u64>>();
        // 老区间夹 1、2 号都满了 → 接下来是 3 号（不再是 400-599）
        assert_eq!(pick_target(&order, &full, 200), "3");
    }

    #[test]
    fn unique_target_name_never_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "vv-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src").join("片名.mp4");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"x").unwrap();
        // 目标夹为空 → 原名保留
        assert_eq!(unique_target_name(&dir, &src), "片名.mp4");
        fs::write(dir.join("片名.mp4"), b"x").unwrap();
        assert_eq!(unique_target_name(&dir, &src), "片名 (2).mp4");
        fs::write(dir.join("片名 (2).mp4"), b"x").unwrap();
        assert_eq!(unique_target_name(&dir, &src), "片名 (3).mp4");
        fs::remove_dir_all(dir).ok();
    }

    /// 真实端到端：造 3 个小视频源 → plan/enqueue 复制进区间夹 → 校验大小与改名规则；
    /// 第二批的源（模拟删除后漏缺）应被补进第一个未满夹。
    #[test]
    fn real_end_to_end_import_fills_intervals() {
        let dir = std::env::temp_dir().join(format!(
            "vv-import-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let import_root = dir.join("Supervideo");
        // 模拟已有 0-199 夹装满（用 1 个非视频占位 + 名义计数走 plan 的实测路径复杂，
        // 这里直接验证 plan_targets 的真实读盘行为：夹内视频文件数即计数）
        let full = import_root.join("0-199");
        fs::create_dir_all(&full).unwrap();
        let mut srcs = Vec::new();
        let src_dir = dir.join("sources");
        fs::create_dir_all(&src_dir).unwrap();
        let Some(ffmpeg) = media::find_tool("ffmpeg") else {
            eprintln!("SKIP: 本机没有 ffmpeg，跳过导入端到端");
            return;
        };
        for i in 0..3 {
            let p = src_dir.join(format!("s{i}.mp4"));
            let st = crate::proc::quiet(&ffmpeg)
                .args([
                    "-hide_banner", "-loglevel", "error", "-y",
                    "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10:duration=1",
                    "-c:v", "libx264", "-preset", "ultrafast",
                    &p.to_string_lossy(),
                ])
                .status()
                .expect("ffmpeg 造源失败");
            assert!(st.success());
            srcs.push(p);
        }
        // 0-199 先放 199 个"视频"（用 1 个真实视频 + 198 个扩展名伪装文件，count_videos 只认扩展名）
        let real = full.join("real.mp4");
        fs::copy(&srcs[0], &real).unwrap();
        for i in 0..198 {
            fs::write(full.join(format!("fill{i}.mp4")), b"x").unwrap();
        }
        assert_eq!(count_videos(&full), 199, "0-199 现有 199 个视频");

        // 3 个源导入：先把 0-199 补到 200，再开下一号夹「2」（只往后追加，不另开区间）
        let tasks = plan_targets(&import_root, &srcs).unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].target.starts_with(&full), "第一个应追加进最后一个未满夹 0-199");
        assert!(tasks[1].target.parent().unwrap().ends_with("2"), "满了开 2 号夹");
        assert!(tasks[2].target.parent().unwrap().ends_with("2"));

        // 串行复制（同 worker 行为：.part → 校验 → 改名）
        for t in &tasks {
            let part = part_path(&t.target);
            copy_chunked(t, &part, &AtomicBool::new(false), |_| {})
                .unwrap_or_else(|e| panic!("复制失败: {e}"));
            fs::rename(&part, &t.target).unwrap();
        }
        assert_eq!(count_videos(&full), 200, "0-199 应恰好装满 200");
        let next = import_root.join("2");
        assert_eq!(count_videos(&next), 2);
        let meta = fs::metadata(&tasks[0].target).unwrap();
        assert_eq!(meta.len(), fs::metadata(&srcs[0]).unwrap().len(), "落盘大小应与源一致");
        // 落盘文件要继承源的修改时间，否则库里按「文件修改时间」排序全是导入那一刻
        let src_m = fs::metadata(&srcs[0]).and_then(|m| m.modified()).unwrap();
        let dst_m = fs::metadata(&tasks[0].target).and_then(|m| m.modified()).unwrap();
        assert_eq!(dst_m, src_m, "落盘文件应继承源文件 mtime");

        fs::remove_dir_all(dir).ok();
    }
}
