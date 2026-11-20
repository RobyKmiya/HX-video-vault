//! 视频剪辑（裁剪 / 拼接）—— ffmpeg 前端的最小封装。
//!
//! 与其它子系统共享的两条铁律在这里同样生效：
//! - 任务全程不持 AppState 锁：开始前短锁解析素材路径，之后 ffmpeg 独跑；
//! - 所有子进程走 proc::quiet，不闪 cmd 黑框。
//!
//! 输出纪律：成品写 `<库根>\剪辑输出\`，先写 `.part` 再原子改名（中断不留半截文件），
//! 完成后由前端触发一次增量扫描入库。绝不删除输出目录以外的文件。
//!
//! 两条路线：
//! - copy：`-c copy` 流复制，瞬时完成，但切割/拼接点落在关键帧上（可能有几秒偏差）；
//! - encode：libx264/aac 重编码，逐帧精确或跨参数对齐，长视频较慢。
//! 裁剪默认 copy、可选 encode；拼接全段参数一致走 copy，否则自动重编码对齐第一段。

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};

use crate::db::LibraryContext;
use crate::media::{self, MediaMeta};
use crate::models::{EditSource, EditStatus};
use crate::proc;

/// 一段已就绪的剪辑素材。
pub struct ClipInfo {
    pub path: PathBuf,
    /// 展示名（库条目用 file_name，外来文件用文件名本身），用于成品命名
    pub display: String,
    pub meta: MediaMeta,
    pub container: String,
}

pub struct ResolvedSource {
    pub path: PathBuf,
    pub display: String,
}

/// ffmpeg 参数计划。`{OUT}` 是成品路径占位符，run_plan 落地时替换。
/// 计划是纯数据（可单测），真正写盘只有 concat 的清单文件与运行期临时文件。
pub struct FfmpegPlan {
    pub args: Vec<String>,
    /// 预计输出总时长（秒），进度换算用
    pub total_sec: f64,
    pub out_ext: String,
    /// copy = 流复制；encode = 重编码
    pub mode: &'static str,
    /// 运行前生成、跑完即删的临时文件（concat 清单）
    pub temp_file: Option<PathBuf>,
}

pub enum EditJob {
    Trim {
        sources: Vec<EditSource>,
        start_sec: f64,
        end_sec: f64,
        accurate: bool,
    },
    Concat {
        sources: Vec<EditSource>,
    },
}

// ---------- 素材解析 ----------

/// 短锁阶段调用：把 EditSource 变成真实路径 + 展示名。
/// 库内条目走 player::resolve，压缩包条目会按需解到缓存。
pub fn resolve_sources(ctx: &LibraryContext, sources: &[EditSource]) -> Result<Vec<ResolvedSource>, String> {
    if sources.is_empty() {
        return Err("没有选择素材".into());
    }
    let mut out = Vec::new();
    for (i, src) in sources.iter().enumerate() {
        let (path, display) = match (&src.id, &src.path) {
            (Some(id), _) => {
                let info = crate::player::resolve(ctx, *id)?;
                if info.media_path.is_empty() {
                    return Err(format!(
                        "第 {} 个素材（{}）的文件已移动或删除",
                        i + 1,
                        info.video.file_name
                    ));
                }
                (PathBuf::from(info.media_path), info.video.file_name)
            }
            (None, Some(p)) => {
                let path = PathBuf::from(p);
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                (path, name)
            }
            _ => return Err(format!("第 {} 个素材缺少来源", i + 1)),
        };
        if !path.is_file() {
            return Err(format!("素材不存在：{}", path.to_string_lossy()));
        }
        out.push(ResolvedSource { path, display });
    }
    Ok(out)
}

/// 无锁阶段：逐个 ffprobe。任何一个读不出时长都算无效素材（剪辑离不开时长）。
pub fn probe_clips(resolved: &[ResolvedSource]) -> Result<Vec<ClipInfo>, String> {
    let mut out = Vec::new();
    for r in resolved {
        let meta = media::probe(&r.path);
        if meta.duration_sec <= 0.0 {
            return Err(format!(
                "无法读取视频时长（可能已损坏或不是视频）：{}",
                r.path.to_string_lossy()
            ));
        }
        let container = r
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        out.push(ClipInfo {
            path: r.path.clone(),
            display: r.display.clone(),
            meta,
            container,
        });
    }
    Ok(out)
}

// ---------- 计划构建（纯函数，可单测） ----------

fn fmt_secs(sec: f64) -> String {
    if sec >= 60.0 {
        format!("{:.0}分{:.0}秒", sec / 60.0, sec % 60.0)
    } else {
        format!("{sec:.1} 秒")
    }
}

pub fn plan_trim(clip: &ClipInfo, start: f64, end: f64, accurate: bool) -> Result<FfmpegPlan, String> {
    if !(start >= 0.0) {
        return Err("起点不能为负".into());
    }
    if end <= start {
        return Err("终点必须大于起点".into());
    }
    if end > clip.meta.duration_sec + 0.5 {
        return Err(format!(
            "终点 {} 超出视频时长 {}",
            fmt_secs(end),
            fmt_secs(clip.meta.duration_sec)
        ));
    }
    let dur = end - start;
    let mut args = base_args();
    args.push("-ss".into());
    args.push(format!("{start:.3}"));
    args.push("-i".into());
    args.push(clip.path.to_string_lossy().to_string());
    args.push("-t".into());
    args.push(format!("{dur:.3}"));
    if accurate {
        args.extend([
            "-c:v".to_string(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-crf".into(),
            "20".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "160k".into(),
            "-movflags".into(),
            "+faststart".into(),
            "{OUT}".into(),
        ]);
        Ok(FfmpegPlan {
            args,
            total_sec: dur,
            out_ext: "mp4".into(),
            mode: "encode",
            temp_file: None,
        })
    } else {
        args.extend([
            "-c".to_string(),
            "copy".into(),
            "-avoid_negative_ts".into(),
            "make_zero".into(),
            "{OUT}".into(),
        ]);
        Ok(FfmpegPlan {
            args,
            total_sec: dur,
            out_ext: clip.container.clone(),
            mode: "copy",
            temp_file: None,
        })
    }
}

/// 拼接计划：全段容器/编码/分辨率一致 → concat 清单 + 流复制；
/// 否则 → 重编码，全部对齐到第一段的分辨率与帧率，缺音轨的段补静音。
pub fn plan_concat(clips: &[ClipInfo], work_dir: &Path) -> Result<FfmpegPlan, String> {
    if clips.is_empty() {
        return Err("没有素材".into());
    }
    let first = &clips[0];
    let uniform = clips.len() == 1
        || clips.iter().all(|c| {
            c.container == first.container
                && c.meta.codec_video == first.meta.codec_video
                && c.meta.codec_audio == first.meta.codec_audio
                && c.meta.width == first.meta.width
                && c.meta.height == first.meta.height
        });
    let total: f64 = clips.iter().map(|c| c.meta.duration_sec).sum();

    if uniform {
        fs::create_dir_all(work_dir).map_err(|e| format!("无法创建缓存目录: {e}"))?;
        let list = work_dir.join(format!("concat-{}.txt", uuid::Uuid::new_v4()));
        write_concat_list(clips, &list)?;
        return Ok(FfmpegPlan {
            args: [
                base_args(),
                vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.to_string_lossy().to_string()],
                vec!["-c".into(), "copy".into(), "-movflags".into(), "+faststart".into(), "{OUT}".into()],
            ]
            .concat(),
            total_sec: total,
            out_ext: first.container.clone(),
            mode: "copy",
            temp_file: Some(list),
        });
    }

    // 重编码对齐第一段
    let (w, h) = (first.meta.width, first.meta.height);
    if w <= 0 || h <= 0 {
        return Err(format!("第一段（{}）分辨率未知，无法自动对齐", first.display));
    }
    let fps = if first.meta.fps > 0.0 { first.meta.fps } else { 30.0 };
    let fps_s = format!("{fps:.5}");
    let any_audio = clips.iter().any(|c| !c.meta.codec_audio.is_empty());

    let mut filter = String::new();
    for (i, c) in clips.iter().enumerate() {
        filter.push_str(&format!(
            "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps_s}[v{i}];"
        ));
        if c.meta.codec_audio.is_empty() {
            let dur = format!("{:.3}", c.meta.duration_sec);
            filter.push_str(&format!(
                "anullsrc=channel_layout=stereo:sample_rate=48000[d{i}];[d{i}]atrim=duration={dur},asetpts=PTS-STARTPTS[a{i}];"
            ));
        } else {
            filter.push_str(&format!(
                "[{i}:a]aformat=sample_rates=48000:channel_layouts=stereo[a{i}];"
            ));
        }
    }
    for i in 0..clips.len() {
        filter.push_str(&format!("[v{i}]"));
        if any_audio {
            filter.push_str(&format!("[a{i}]"));
        }
    }
    if any_audio {
        filter.push_str(&format!("concat=n={}:v=1:a=1[v][a]", clips.len()));
    } else {
        filter.push_str(&format!("concat=n={}:v=1:a=0[v]", clips.len()));
    }

    let mut args = base_args();
    for c in clips {
        args.push("-i".into());
        args.push(c.path.to_string_lossy().to_string());
    }
    args.push("-filter_complex".into());
    args.push(filter);
    if any_audio {
        args.extend(["-map".into(), "[v]".into(), "-map".into(), "[a]".into()]);
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    } else {
        args.extend(["-map".into(), "[v]".into()]);
    }
    args.extend(["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into(), "-crf".into(), "20".into()]);
    args.extend(["-movflags".into(), "+faststart".into(), "{OUT}".into()]);
    Ok(FfmpegPlan {
        args,
        total_sec: total,
        out_ext: "mp4".into(),
        mode: "encode",
        temp_file: None,
    })
}

fn base_args() -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// concat demuxer 清单：单引号包裹（内部单引号按 ffmpeg 规则转义），反斜杠原样保留。
pub fn write_concat_list(clips: &[ClipInfo], list_path: &Path) -> Result<(), String> {
    let mut body = String::new();
    for c in clips {
        let p = c.path.to_string_lossy().replace('\'', "'\\''");
        body.push_str(&format!("file '{p}'\n"));
    }
    fs::write(list_path, body).map_err(|e| format!("无法写拼接清单: {e}"))
}

// ---------- 成品命名 ----------

/// 成品名里的非法字符与压缩包条目的「合集.cbz › 第01集.mp4」形态都收拢成干净词干。
pub fn sanitize_stem(name: &str) -> String {
    let last = name.split('›').next_back().unwrap_or(name).trim();
    let cleaned: String = last
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => ch,
        })
        .collect();
    // 去掉残留扩展名
    let stemmed = Path::new(cleaned.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| cleaned.trim().to_string());
    let stemmed = stemmed.trim().trim_end_matches('.').trim().to_string();
    if stemmed.is_empty() {
        return "clip".into();
    }
    if stemmed.chars().count() > 60 {
        stemmed.chars().take(60).collect()
    } else {
        stemmed
    }
}

/// 本地（UTC+8）时分秒六位，用于成品名防重。
fn hhmmss() -> String {
    let total = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 8 * 3600;
    let s = total % 86_400;
    format!("{:02}{:02}{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 输出目录里找第一个不冲突的名字（同名自动 -2、-3…），绝不覆盖已有文件。
pub fn unique_output(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..=999 {
        let p = dir.join(format!("{stem}-{n}.{ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}.{}", uuid::Uuid::new_v4().simple(), ext))
}

// ---------- 执行 ----------

/// 跑 ffmpeg：stdout 解析 `-progress` 换算百分比，stderr 落临时文件防管道死锁。
/// 取消 = 置位 cancel，下一行进度到来时 kill 子进程并清掉半成品。
pub fn run_plan(
    ffmpeg: &Path,
    plan: &FfmpegPlan,
    out_final: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64),
) -> Result<(), String> {
    // 半成品名保留已知扩展名（trim.mp4 → trim.part.mp4），ffmpeg 才能认出封装格式；
    // 直接叫 trim.mp4.part 会因扩展名 part 报 Invalid argument
    let stem = out_final
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".into());
    let ext = out_final
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".into());
    let part = out_final.with_file_name(format!("{stem}.part.{ext}"));
    let err_path = out_final.with_file_name(format!("{stem}.part.{ext}.err"));
    let _ = fs::remove_file(&part);
    let err_file = fs::File::create(&err_path).map_err(|e| format!("无法创建错误日志: {e}"))?;
    let args: Vec<String> = plan
        .args
        .iter()
        .map(|a| {
            // ffmpeg 先写 .part 半成品，成功后原子改名 —— 中断/取消不留半截文件
            if a == "{OUT}" {
                part.to_string_lossy().to_string()
            } else {
                a.clone()
            }
        })
        .collect();
    let spawned = proc::quiet(ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(err_file))
        .spawn();
    let mut child: Child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_file(&err_path);
            return Err(format!("无法启动 ffmpeg: {e}"));
        }
    };
    let stdout = child.stdout.take().expect("ffmpeg stdout 已接管");
    let mut reader = BufReader::new(stdout);
    let mut cancelled = false;
    let mut line = String::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let t = line.trim_end();
        // out_time_us / out_time_ms 实际都是微秒
        if let Some(v) = t
            .strip_prefix("out_time_us=")
            .or_else(|| t.strip_prefix("out_time_ms="))
        {
            if let Ok(us) = v.trim().parse::<f64>() {
                let pct = if plan.total_sec > 0.0 {
                    ((us / 1_000_000.0) / plan.total_sec * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };
                on_progress(pct);
            }
        } else if t == "progress=end" {
            on_progress(100.0);
        }
    }
    let status = child.wait();
    if cancelled {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&part);
        let _ = fs::remove_file(&err_path);
        return Err("已取消".into());
    }
    let ok = status.map(|s| s.success()).unwrap_or(false);
    if !ok {
        let tail = fs::read_to_string(&err_path).unwrap_or_default();
        let msg = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("ffmpeg 执行失败")
            .to_string();
        let _ = fs::remove_file(&part);
        let _ = fs::remove_file(&err_path);
        return Err(if plan.mode == "copy" {
            format!("{msg}（流复制失败，可改用精确/重编码模式重试）")
        } else {
            msg
        });
    }
    let _ = fs::remove_file(&err_path);
    if !part.is_file() {
        return Err("ffmpeg 未产出文件".into());
    }
    fs::rename(&part, out_final).map_err(|e| format!("无法落盘成品: {e}"))?;
    on_progress(100.0);
    Ok(())
}

// ---------- 任务编排 ----------

fn push_status(
    status_arc: &Mutex<EditStatus>,
    app: &AppHandle,
    running: bool,
    phase: &str,
    percent: f64,
    message: &str,
) {
    let st = EditStatus {
        running,
        phase: phase.to_string(),
        percent,
        message: message.to_string(),
    };
    if let Ok(mut slot) = status_arc.lock() {
        *slot = st.clone();
    }
    let _ = app.emit("edit-progress", st);
}

/// 裁剪/拼接共用的工作流：短锁解析 → 无锁探测 → 出计划 → 独跑 ffmpeg。
/// 期间只有三段极短锁，扫描/播放照常可用。
pub fn run_edit_job(
    state: &crate::AppState,
    app: &AppHandle,
    job: EditJob,
) -> Result<crate::models::EditReport, String> {
    let (running, cancel, status_arc) = {
        let guard = state.lock()?;
        (
            guard.edit_running.clone(),
            guard.edit_cancel.clone(),
            guard.edit_status.clone(),
        )
    };
    if running.swap(true, Ordering::SeqCst) {
        return Err("已有剪辑任务在跑，请等它结束或先取消".into());
    }
    cancel.store(false, Ordering::SeqCst);
    // 无论正常结束、报错还是取消，都要把 running 放掉，否则按钮永远转圈
    struct Reset(Arc<AtomicBool>, Arc<Mutex<EditStatus>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
            if let Ok(mut st) = self.1.lock() {
                st.running = false;
                if st.phase == "run" || st.phase == "probe" {
                    st.phase = "idle".into();
                }
            }
        }
    }
    let _reset = Reset(running.clone(), status_arc.clone());

    let Some(ffmpeg) = media::find_tool("ffmpeg") else {
        return Err("未找到 ffmpeg，请在设置里指定 ffmpeg 路径后重试".into());
    };

    let (sources, start_sec, end_sec, accurate) = match &job {
        EditJob::Trim { sources, start_sec, end_sec, accurate } => {
            (sources.as_slice(), Some(*start_sec), Some(*end_sec), Some(*accurate))
        }
        EditJob::Concat { sources } => (sources.as_slice(), None, None, None),
    };

    push_status(&status_arc, app, true, "probe", -1.0, "解析素材…");
    // 短锁一：解析素材真实路径（压缩包条目按需解缓存）
    let (resolved, root) = {
        let guard = state.lock()?;
        let lib = guard
            .library
            .as_ref()
            .ok_or("尚未创建或打开视频库，请先在设置里选择库目录")?;
        (resolve_sources(lib, sources)?, lib.root.clone())
    };
    // 无锁：逐个 ffprobe
    let clips = probe_clips(&resolved)?;

    let work_dir = LibraryContext::vault_dir(&root).join("cache").join("edits");
    let (plan, label) = match (&job, start_sec, end_sec, accurate) {
        (EditJob::Trim { .. }, Some(s), Some(e), Some(acc)) => {
            if clips.len() != 1 {
                return Err("裁剪需要且只需要一个素材".into());
            }
            let plan = plan_trim(&clips[0], s, e, acc)?;
            let stem = sanitize_stem(&clips[0].display);
            (plan, format!("{stem}_裁剪_{}", hhmmss()))
        }
        (EditJob::Concat { .. }, _, _, _) => {
            let plan = plan_concat(&clips, &work_dir)?;
            let stem = sanitize_stem(&clips[0].display);
            let label = if clips.len() == 1 {
                format!("{stem}_拼接_{}", hhmmss())
            } else {
                format!("{stem}等{}段_拼接_{}", clips.len(), hhmmss())
            };
            (plan, label)
        }
        _ => return Err("剪辑参数不完整".into()),
    };

    let out_dir = root.join("剪辑输出");
    fs::create_dir_all(&out_dir).map_err(|e| format!("无法创建输出目录: {e}"))?;
    let out_final = unique_output(&out_dir, &label, &plan.out_ext);
    let running_msg = format!(
        "{} → {}",
        if plan.mode == "copy" { "流复制" } else { "重新编码" },
        out_final.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
    );
    push_status(&status_arc, app, true, "run", 0.0, &running_msg);

    let mut last_emit = -1.0f64;
    let status_for_emit = status_arc.clone();
    let app_for_emit = app.clone();
    let msg_for_emit = running_msg.clone();
    let result = run_plan(&ffmpeg, &plan, &out_final, &cancel, |pct| {
        // 至少变化 1% 才值得动一次锁 + 一次 IPC
        if pct - last_emit >= 1.0 || pct >= 100.0 {
            last_emit = pct;
            push_status(&status_for_emit, &app_for_emit, true, "run", pct, &msg_for_emit);
        }
    });
    if let Some(tmp) = &plan.temp_file {
        let _ = fs::remove_file(tmp);
    }

    let note = match plan.mode {
        "copy" => match &job {
            EditJob::Trim { .. } => "流复制：瞬时完成，切割点对齐关键帧（可能有几秒偏差）".to_string(),
            EditJob::Concat { .. } => "全段参数一致，流复制拼接".to_string(),
        },
        _ => match &job {
            EditJob::Trim { .. } => "已重新编码（libx264/aac → mp4），逐帧精确".to_string(),
            EditJob::Concat { .. } => format!(
                "各段参数不一致，已重新编码对齐第一段（{}×{}）",
                clips[0].meta.width, clips[0].meta.height
            ),
        },
    };

    result?;
    let out_s = out_final.to_string_lossy().to_string();
    push_status(&status_arc, app, false, "done", 100.0, &format!("完成：{out_s}"));
    Ok(crate::models::EditReport {
        output_path: out_s,
        mode: plan.mode.to_string(),
        note,
    })
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(path: &str, container: &str, cv: &str, ca: &str, w: i64, h: i64, dur: f64, fps: f64) -> ClipInfo {
        ClipInfo {
            path: PathBuf::from(path),
            display: PathBuf::from(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            meta: MediaMeta {
                duration_sec: dur,
                width: w,
                height: h,
                codec_video: cv.into(),
                codec_audio: ca.into(),
                fps,
            },
            container: container.into(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vv-edit-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn trim_copy_keeps_container_and_snapshots_keyframes() {
        let c = clip(r"E:\v\a.mp4", "mp4", "h264", "aac", 1920, 1080, 600.0, 30.0);
        let plan = plan_trim(&c, 10.0, 70.0, false).unwrap();
        assert_eq!(plan.mode, "copy");
        assert_eq!(plan.out_ext, "mp4");
        assert_eq!(plan.total_sec, 60.0);
        let joined = plan.args.join(" ");
        assert!(joined.contains("-c copy"), "{joined}");
        assert!(joined.contains("-ss 10.000"));
        assert!(joined.contains("-t 60.000"));
        assert!(plan.args.iter().any(|a| a == "{OUT}"));
    }

    #[test]
    fn trim_encode_targets_mp4() {
        let c = clip(r"E:\v\a.mkv", "mkv", "vp9", "opus", 1280, 720, 120.0, 25.0);
        let plan = plan_trim(&c, 0.0, 30.0, true).unwrap();
        assert_eq!(plan.mode, "encode");
        assert_eq!(plan.out_ext, "mp4");
        let joined = plan.args.join(" ");
        assert!(joined.contains("libx264"));
        assert!(joined.contains("aac"));
        assert!(joined.contains("-movflags +faststart"));
    }

    #[test]
    fn trim_rejects_bad_range() {
        let c = clip(r"E:\v\a.mp4", "mp4", "h264", "aac", 1920, 1080, 100.0, 30.0);
        assert!(plan_trim(&c, 10.0, 10.0, false).is_err(), "终点等于起点应拒绝");
        assert!(plan_trim(&c, 20.0, 10.0, false).is_err(), "终点小于起点应拒绝");
        assert!(plan_trim(&c, -1.0, 10.0, false).is_err(), "负起点应拒绝");
        assert!(plan_trim(&c, 10.0, 200.0, false).is_err(), "超出时长应拒绝");
        // 0.5 秒容差内的越界允许（前端四舍五入）
        assert!(plan_trim(&c, 10.0, 100.3, false).is_ok());
    }

    #[test]
    fn concat_uniform_segments_use_stream_copy() {
        let a = clip(r"E:\v\a.mp4", "mp4", "h264", "aac", 1920, 1080, 100.0, 30.0);
        let b = clip(r"E:\v\b.mp4", "mp4", "h264", "aac", 1920, 1080, 50.0, 30.0);
        let dir = temp_dir("uniform");
        let plan = plan_concat(&[a, b], &dir).unwrap();
        assert_eq!(plan.mode, "copy");
        assert_eq!(plan.out_ext, "mp4");
        assert_eq!(plan.total_sec, 150.0);
        let joined = plan.args.join(" ");
        assert!(joined.contains("-f concat"), "{joined}");
        assert!(joined.contains("-c copy"));
        let list = plan.temp_file.clone().unwrap();
        let body = fs::read_to_string(&list).unwrap();
        assert!(body.contains("file 'E:\\v\\a.mp4'"), "{body}");
        assert!(body.contains("file 'E:\\v\\b.mp4'"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn concat_mixed_segments_reencode_aligned_to_first() {
        let a = clip(r"E:\v\a.mp4", "mp4", "h264", "aac", 1280, 720, 100.0, 29.97);
        let b = clip(r"E:\v\b.mkv", "mkv", "vp9", "", 1920, 1080, 50.0, 60.0);
        let dir = temp_dir("mixed");
        let plan = plan_concat(&[a, b], &dir).unwrap();
        assert_eq!(plan.mode, "encode");
        assert_eq!(plan.out_ext, "mp4");
        let joined = plan.args.join(" ");
        assert!(joined.contains("-filter_complex"), "{joined}");
        assert!(joined.contains("concat=n=2:v=1:a=1[v][a]"));
        assert!(joined.contains("scale=1280:720"), "第二段要缩到第一段分辨率");
        assert!(joined.contains("pad=1280:720"), "比例不同要加黑边");
        assert!(joined.contains("anullsrc"), "缺音轨的段要补静音");
        assert!(joined.contains("atrim=duration=50.000"), "静音长度要对齐该段时长");
        assert!(joined.contains("fps=29.97000"));
        assert!(joined.contains("libx264"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn concat_without_any_audio_has_no_audio_map() {
        // 两段都没音轨但分辨率不同 → 必须走重编码，且音频侧应为 v=1:a=0
        let a = clip(r"E:\v\a.mp4", "mp4", "h264", "", 1280, 720, 100.0, 30.0);
        let b = clip(r"E:\v\b.mp4", "mp4", "h264", "", 640, 360, 50.0, 30.0);
        let dir = temp_dir("silent");
        let plan = plan_concat(&[a, b], &dir).unwrap();
        assert_eq!(plan.mode, "encode");
        let joined = plan.args.join(" ");
        assert!(joined.contains("concat=n=2:v=1:a=0[v]"), "{joined}");
        assert!(!joined.contains("-c:a"), "没有音轨就不该出现音频编码参数");
        assert!(!joined.contains("[v0][a0][v1][a1]"), "无音轨拼接不应引用音频流");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn concat_list_escapes_single_quotes() {
        let a = clip("E:\\v\\it's.mp4", "mp4", "h264", "aac", 1280, 720, 10.0, 30.0);
        let dir = temp_dir("escape");
        let list = dir.join("list.txt");
        write_concat_list(&[a], &list).unwrap();
        let body = fs::read_to_string(&list).unwrap();
        assert!(body.contains("file 'E:\\v\\it'\\''s.mp4'"), "{body}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sanitize_stem_handles_archive_and_illegal_chars() {
        assert_eq!(sanitize_stem("合集.cbz › 第01集.mp4"), "第01集");
        assert_eq!(sanitize_stem("a/b:c*d?.mp4"), "a_b_c_d_");
        assert_eq!(sanitize_stem(""), "clip");
        assert_eq!(sanitize_stem("影片.FINAL.v2.mp4"), "影片.FINAL.v2");
    }

    #[test]
    fn unique_output_never_overwrites() {
        let dir = temp_dir("unique");
        fs::write(dir.join("a.mp4"), b"x").unwrap();
        let p = unique_output(&dir, "a", "mp4");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "a-2.mp4");
        let q = unique_output(&dir, "b", "mp4");
        assert_eq!(q.file_name().unwrap().to_string_lossy(), "b.mp4");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn parse_fraction_reads_ffmpeg_rates() {
        assert!((media::parse_fraction("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert!((media::parse_fraction("30/1").unwrap() - 30.0).abs() < 1e-9);
        assert!(media::parse_fraction("0/0").is_none());
        assert!(media::parse_fraction("abc").is_none());
    }

    /// 真实端到端：需要本机有 ffmpeg。没有就跳过（CI/无 ffmpeg 环境不失败）。
    #[test]
    fn real_ffmpeg_trim_and_concat_roundtrip() {
        let Some(ffmpeg) = media::find_tool("ffmpeg") else {
            eprintln!("SKIP: 本机没有 ffmpeg，跳过端到端");
            return;
        };
        let dir = temp_dir("e2e");
        // 造两段 2 秒、320x240、颜色不同的测试片（testsrc 不依赖外部素材）
        let mk = |name: &str, color: &str| {
            let p = dir.join(name);
            let st = proc::quiet(&ffmpeg)
                .args([
                    "-hide_banner", "-loglevel", "error", "-y",
                    "-f", "lavfi", "-i", &format!("color=c={color}:size=320x240:rate=15:duration=2"),
                    "-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=44100",
                    "-t", "2", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac",
                    &p.to_string_lossy(),
                ])
                .status()
                .expect("ffmpeg 造测试片失败");
            assert!(st.success(), "测试片 {name} 生成失败");
            p
        };
        let a = mk("a.mp4", "red");
        let b = mk("b.mp4", "blue");

        // 裁剪 copy：取 0.5 ~ 1.5 秒。终点 1.5 必须被尊重；起点可能回吸到开头关键帧（流复制的本性）
        let ca = probe_clips(&[ResolvedSource { path: a.clone(), display: "a.mp4".into() }]).unwrap();
        let plan = plan_trim(&ca[0], 0.5, 1.5, false).unwrap();
        let out1 = dir.join("trim.mp4");
        run_plan(&ffmpeg, &plan, &out1, &AtomicBool::new(false), |_| {}).unwrap();
        let meta = media::probe(&out1);
        assert!(
            meta.duration_sec >= 0.9 && meta.duration_sec <= 1.6,
            "流复制裁剪成片时长应在 0.9~1.6 秒（关键帧吸附），实测 {}",
            meta.duration_sec
        );

        // 裁剪 encode（精确）：0.5 ~ 1.5 应逐帧精确 ≈ 1.0 秒
        let plan = plan_trim(&ca[0], 0.5, 1.5, true).unwrap();
        let out1b = dir.join("trim-accurate.mp4");
        run_plan(&ffmpeg, &plan, &out1b, &AtomicBool::new(false), |_| {}).unwrap();
        let meta = media::probe(&out1b);
        assert!(
            (meta.duration_sec - 1.0).abs() < 0.25,
            "精确裁剪成片时长应约 1 秒，实测 {}",
            meta.duration_sec
        );

        // 拼接 copy：两段同参数 → 秒拼，总时长约 4 秒
        let clips = probe_clips(&[
            ResolvedSource { path: a.clone(), display: "a.mp4".into() },
            ResolvedSource { path: b.clone(), display: "b.mp4".into() },
        ])
        .unwrap();
        let plan = plan_concat(&clips, &dir).unwrap();
        assert_eq!(plan.mode, "copy", "同参数两段应走流复制");
        let out2 = dir.join("concat.mp4");
        run_plan(&ffmpeg, &plan, &out2, &AtomicBool::new(false), |_| {}).unwrap();
        let meta = media::probe(&out2);
        assert!(
            (meta.duration_sec - 4.0).abs() < 0.5,
            "拼接成片时长应约 4 秒，实测 {}",
            meta.duration_sec
        );

        fs::remove_dir_all(dir).ok();
    }

    /// 取消：跑到一半置位 cancel，run_plan 应报「已取消」且不留半成品。
    #[test]
    fn real_ffmpeg_cancel_leaves_no_part_file() {
        let Some(ffmpeg) = media::find_tool("ffmpeg") else {
            eprintln!("SKIP: 本机没有 ffmpeg，跳过取消端到端");
            return;
        };
        let dir = temp_dir("cancel");
        let src = dir.join("long.mp4");
        let st = proc::quiet(&ffmpeg)
            .args([
                "-hide_banner", "-loglevel", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=300",
                "-c:v", "libx264", "-preset", "ultrafast",
                &src.to_string_lossy(),
            ])
            .status()
            .expect("ffmpeg 造长片失败");
        assert!(st.success());
        let clips = probe_clips(&[ResolvedSource { path: src, display: "long.mp4".into() }]).unwrap();
        let plan = plan_trim(&clips[0], 0.0, 300.0, true).unwrap(); // 重编码 300 秒，跑不完
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            cancel2.store(true, Ordering::SeqCst);
        });
        let out = dir.join("cancelled.mp4");
        let r = run_plan(&ffmpeg, &plan, &out, &cancel, |_| {});
        assert_eq!(r.unwrap_err(), "已取消");
        assert!(!out.is_file(), "不应留下成品");
        assert!(!dir.join("cancelled.part.mp4").is_file(), "不应留下半成品");
        assert!(!dir.join("cancelled.part.mp4.err").is_file(), "不应留下错误日志");
        fs::remove_dir_all(dir).ok();
    }
}
