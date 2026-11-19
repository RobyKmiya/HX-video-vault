//! mpv JSON IPC 播放器控制层。
//!
//! 设计取舍：
//! - mpv 用独立进程运行，通过 `--input-ipc-server` 暴露 JSON IPC；
//! - 进度/时长用 `observe_property` 事件主动推给前端，不靠轮询文件；
//! - 所有命令都是 fire-and-forget 的 JSON 行，前端按事件更新 UI；
//! - 本机没装 mpv 时 `find_mpv()` 返回 None，前端自动降级到内置播放器/VLC。

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

static REQ_ID: AtomicI64 = AtomicI64::new(1);

#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineInfo {
    pub name: String,
    pub available: bool,
    pub path: String,
    pub note: String,
}

/// 探测 mpv：显式配置 → PATH → 常见目录。
pub fn find_mpv(configured: Option<&str>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = configured.filter(|s| !s.trim().is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            candidates.push(dir.join("mpv.exe"));
            candidates.push(dir.join("mpv"));
        }
    }
    candidates.extend([
        PathBuf::from(r"C:\Program Files\mpv\mpv.exe"),
        PathBuf::from(r"C:\Program Files (x86)\mpv\mpv.exe"),
        PathBuf::from(r"D:\TOOLS\mpv\mpv.exe"),
        PathBuf::from(r"D:\Program Files\mpv\mpv.exe"),
    ]);
    candidates.into_iter().find(|p| p.is_file())
}

pub fn engines(configured_mpv: Option<&str>) -> Vec<EngineInfo> {
    let mpv = find_mpv(configured_mpv);
    let vlc = crate::player::find_vlc_public();
    vec![
        EngineInfo {
            name: "mpv".into(),
            available: mpv.is_some(),
            path: mpv.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            note: if mpv.is_some() {
                "JSON IPC · 可外挂字幕/截图/倍速".into()
            } else {
                "未安装 mpv，可下载便携版后在设置里指定路径".into()
            },
        },
        EngineInfo {
            name: "vlc".into(),
            available: vlc.is_some(),
            path: vlc.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            note: "本机已有 · 冷门格式兜底，从上次位置续播".into(),
        },
        EngineInfo {
            name: "builtin".into(),
            available: true,
            path: String::new(),
            note: "内置播放器 · mp4/webm/mov 直接播".into(),
        },
    ]
}

struct IpcState {
    writer: Mutex<BufWriter<std::fs::File>>,
    child: Mutex<Option<Child>>,
}

impl IpcState {
    fn send(&self, command: Vec<Value>) -> Result<(), String> {
        let id = REQ_ID.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "command": command, "request_id": id });
        let mut w = self.writer.lock().map_err(|_| "IPC 写入锁损坏")?;
        serde_json::to_writer(&mut *w, &msg).map_err(|e| e.to_string())?;
        w.write_all(b"\n").map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())
    }
}

/// IPC 会话句柄。前台只保留一个实例，切换视频时复用同一 mpv 进程。
pub struct MpvSession {
    inner: Arc<IpcState>,
}

impl MpvSession {
    /// 启动 mpv 并连接 IPC。pipe 名带进程号，避免多实例冲突。
    pub fn start(
        app: AppHandle,
        mpv_path: &Path,
        media: &Path,
        start_sec: f64,
        wid: Option<String>,
    ) -> Result<Self, String> {
        let pipe = format!(r"\\.\pipe\video-vault-{}", std::process::id());
        let mut args: Vec<String> = vec![
            format!("--input-ipc-server={pipe}"),
            "--force-window=yes".into(),
            "--keep-open=yes".into(),
            format!("--start={start_sec}"),
            "--no-terminal".into(),
        ];
        if let Some(w) = wid.filter(|s| !s.is_empty()) {
            args.push(format!("--wid={w}"));
        }
        args.push(media.to_string_lossy().to_string());

        let child = crate::proc::quiet(mpv_path)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("启动 mpv 失败: {e}"))?;

        // named pipe 可能要等 mpv 几百毫秒才创建，重试 5 秒
        let mut file = None;
        for _ in 0..50 {
            match OpenOptions::new().read(true).write(true).open(&pipe) {
                Ok(f) => { file = Some(f); break; }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let file = file.ok_or("连接 mpv IPC 超时（mpv 可能启动失败）")?;
        let state = Arc::new(IpcState {
            writer: Mutex::new(BufWriter::new(file.try_clone().map_err(|e| e.to_string())?)),
            child: Mutex::new(Some(child)),
        });

        // 事件读取线程：property-change → 前端事件
        let reader_state = Arc::clone(&state);
        let app_events = app.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(file);
            for line in reader.lines().flatten() {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue; };
                if let Some(prop) = v.get("name").and_then(Value::as_str) {
                    let data = v.get("data").cloned().unwrap_or(Value::Null);
                    let _ = app_events.emit("mpv-property", json!({ "name": prop, "data": data }));
                }
                // request_id 的响应也转发，前端可据此确认命令生效
                if v.get("error").is_some() {
                    let _ = app_events.emit("mpv-reply", v);
                }
            }
            let _ = reader_state;
        });

        let session = Self { inner: state };
        // 订阅进度/状态属性
        session.send(vec!["observe_property".into(), json!(1), json!("time-pos")])?;
        session.send(vec!["observe_property".into(), json!(2), json!("duration")])?;
        session.send(vec!["observe_property".into(), json!(3), json!("pause")])?;
        session.send(vec!["observe_property".into(), json!(4), json!("eof-reached")])?;
        Ok(session)
    }

    pub fn send(&self, command: Vec<Value>) -> Result<(), String> {
        self.inner.send(command)
    }

    pub fn stop(&self) {
        let _ = self.inner.send(vec!["quit".into()]);
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(mut guard) = self.inner.child.lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            *guard = None;
        }
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 把 IPC 命令名映射到 mpv 命令数组（供前端统一调用）。
pub fn command_from(action: &str, value: Option<Value>) -> Result<Vec<Value>, String> {
    match action {
        "pause" => Ok(vec!["set_property".into(), json!("pause"), json!(true)]),
        "resume" => Ok(vec!["set_property".into(), json!("pause"), json!(false)]),
        "toggle" => Ok(vec!["cycle".into(), json!("pause")]),
        "seek" => {
            let v = value.ok_or("seek 需要秒数")?;
            let sec = v.as_f64().ok_or("seek 参数必须是数字")?;
            Ok(vec!["seek".into(), json!(sec), json!("absolute")])
        }
        "seek_relative" => {
            let v = value.ok_or("seek_relative 需要秒数")?;
            let sec = v.as_f64().ok_or("seek_relative 参数必须是数字")?;
            Ok(vec!["seek".into(), json!(sec), json!("relative")])
        }
        "speed" => {
            let v = value.ok_or("speed 需要倍率")?;
            Ok(vec!["set_property".into(), json!("speed"), v])
        }
        "volume" => {
            let v = value.ok_or("volume 需要音量")?;
            Ok(vec!["set_property".into(), json!("volume"), v])
        }
        "subtitle" => {
            let v = value.ok_or("subtitle 需要路径")?;
            Ok(vec!["sub-add".into(), v, json!("select")])
        }
        "screenshot" => {
            let v = value.ok_or("screenshot 需要输出路径")?;
            Ok(vec!["screenshot-to-file".into(), v, json!("video")])
        }
        "get_time" => Ok(vec!["get_property".into(), json!("time-pos")]),
        "get_duration" => Ok(vec!["get_property".into(), json!("duration")]),
        _ => Err(format!("未知 mpv 命令: {action}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_mapping_is_exact() {
        assert_eq!(
            command_from("toggle", None).unwrap(),
            vec![Value::String("cycle".into()), json!("pause")]
        );
        assert_eq!(
            command_from("seek", Some(json!(12.5))).unwrap(),
            vec![Value::String("seek".into()), json!(12.5), json!("absolute")]
        );
        assert!(command_from("unknown", None).is_err());
    }

    #[test]
    fn engines_always_include_builtin() {
        let list = engines(None);
        assert!(list.iter().any(|e| e.name == "builtin" && e.available));
    }
}
