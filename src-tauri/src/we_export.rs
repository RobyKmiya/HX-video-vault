//! 壁纸导出助手托管：影匣拉起并盯住 node helper.mjs，进程没了就重启。
//!
//! 为什么挪进影匣：之前助手是「开机自启 + 外部守护」的独立进程，node 路径有两套
//! 互相打架，进程反复死、端口 51789 长期没人管，WE 里点导出就报「连续 5 次没响应」。
//! 现在影匣作为唯一守护：启动时拉起 helper.mjs，之后每 15 秒探一次，死了立刻重拉。

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

pub const PORT: u16 = 51789;

/// 影匣托管的助手子进程（唯一守护，不跟外部 supervisor 抢）。
static HELPER: Mutex<Option<Child>> = Mutex::new(None);
static MONITOR_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub alive: bool,
    pub pid: Option<u32>,
    pub port: u16,
    pub node_path: String,
    pub script: String,
    pub vault_root: String,
    pub config: serde_json::Value,
    pub logs: Vec<String>,
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("影匣").join("we-export")
}
fn app_dir() -> PathBuf {
    data_dir().join("app")
}
fn helper_script() -> PathBuf {
    app_dir().join("helper.mjs")
}
fn config_path() -> PathBuf {
    app_dir().join("config.json")
}
fn addr() -> SocketAddr {
    format!("127.0.0.1:{PORT}").parse().unwrap()
}

pub fn load_config() -> serde_json::Value {
    match fs::read_to_string(config_path()) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or(serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    }
}

fn config_vault_root() -> String {
    load_config()
        .get("vaultLibrary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// 找可用的 node.exe：配置显式指定 > 系统 node > WorkBuddy 自带 node > PATH。
fn find_node() -> Option<PathBuf> {
    if let Some(c) = load_config().get("node").and_then(|v| v.as_str()) {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    let sys = PathBuf::from(r"C:\Program Files\nodejs\node.exe");
    if sys.is_file() {
        return Some(sys);
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(home)
            .join(".workbuddy")
            .join("binaries")
            .join("node")
            .join("versions");
        if let Ok(entries) = fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path().join("node.exe");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    if let Ok(out) = Command::new("where").arg("node").output() {
        if let Ok(s) = String::from_utf8(out.stdout) {
            for line in s.lines() {
                let p = PathBuf::from(line.trim());
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 最小 HTTP GET（只服务本机助手，无需引 HTTP crate），返回响应 body。
fn http_get(path: &str) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr(), Duration::from_millis(1000)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_millis(1000)));
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let _ = s.write_all(req.as_bytes());
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 64 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    text.split("\r\n\r\n").nth(1).map(|b| b.trim().to_string())
}

/// 探 /ping：读到 "ok" 才算活（避免把别的进程误当成助手）。
fn ping() -> bool {
    http_get("/ping").map(|b| b.contains("\"ok\"")).unwrap_or(false)
}

/// 助手自检（/health），测试按钮用。
pub fn health() -> Result<serde_json::Value, String> {
    match http_get("/health") {
        Some(body) => serde_json::from_str(&body).map_err(|e| format!("助手响应无法解析：{e}")),
        None => Err(format!("助手未响应（127.0.0.1:{PORT}）")),
    }
}

fn spawn_helper() -> Result<u32, String> {
    if ping() {
        return Ok(0); // 已经在跑（可能旧实例），不重复起
    }
    let node = find_node().ok_or("找不到 node.exe（未安装 Node.js）")?;
    let script = helper_script();
    if !script.is_file() {
        return Err(format!("找不到助手脚本：{}", script.display()));
    }
    let child = Command::new(&node)
        .arg(&script)
        .current_dir(app_dir())
        .spawn()
        .map_err(|e| format!("拉起助手失败：{e}"))?;
    let pid = child.id();
    *HELPER.lock().unwrap() = Some(child);
    Ok(pid)
}

/// 启动助手 + 15 秒一轮的守护（幂等：只会起一个守护线程）。
pub fn start() {
    let _ = spawn_helper();
    if MONITOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(15));
        if ping() {
            continue;
        }
        // 可能刚退出端口还没放，稍等重试一次
        std::thread::sleep(Duration::from_secs(2));
        if ping() {
            continue;
        }
        if let Err(e) = spawn_helper() {
            eprintln!("[we-export] 重启助手失败: {e}");
        }
    });
}

/// 停止助手：杀掉影匣自己拉起的子进程；顺带按 /ping 报的 pid 兜底清一次。
pub fn stop() {
    if let Some(mut c) = HELPER.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if let Some(body) = http_get("/ping") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(pid) = v.get("pid").and_then(|p| p.as_u64()) {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }
    }
}

pub fn status() -> Status {
    Status {
        alive: ping(),
        pid: HELPER.lock().unwrap().as_mut().map(|c| c.id()),
        port: PORT,
        node_path: find_node().map(|p| p.display().to_string()).unwrap_or_default(),
        script: helper_script().display().to_string(),
        vault_root: config_vault_root(),
        config: load_config(),
        logs: tail_logs(120),
    }
}

/// 只允许改这些配置项（都是 helper.mjs 认识的字段）。
pub fn set_config(key: &str, value: serde_json::Value) -> Result<(), String> {
    const ALLOWED: &[&str] = &[
        "vaultLibrary",
        "steamLibrary",
        "weRoot",
        "library",
        "projectsRoot",
        "autoScan",
        "includePreview",
        "nameFrom",
        "node",
    ];
    if !ALLOWED.contains(&key) {
        return Err(format!("不支持的配置项：{key}"));
    }
    let mut cfg = load_config();
    cfg[key] = value;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    fs::write(config_path(), raw).map_err(|e| e.to_string())?;
    Ok(())
}

/// 读最近一个 .log 的末尾 n 行。
fn tail_logs(n: usize) -> Vec<String> {
    let dir = data_dir().join("logs");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "log").unwrap_or(false) {
                if let Ok(meta) = fs::metadata(&p) {
                    if let Ok(t) = meta.modified() {
                        if newest.as_ref().map(|(t0, _)| t > *t0).unwrap_or(true) {
                            newest = Some((t, p));
                        }
                    }
                }
            }
        }
    }
    let Some((_, p)) = newest else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(&p) else {
        return Vec::new();
    };
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|s| s.to_string()).collect()
}
