//! 本地 HTTP 桥：只监听 127.0.0.1，给浏览器油猴脚本提交「网页收藏」用。
//!
//! 为什么不用 Tauri 的 IPC：浏览器沙箱里的脚本够不着 IPC，只能走 HTTP。
//! 只依赖 std（手写最小 HTTP），不引额外 crate，避免给安装包增肥。
//!
//! 端点：
//!   GET  /ping                 → 桥是否活着
//!   GET  /check?url=...        → 这个出处是否已在库里（防重复）
//!   POST /hook  {url,title,author,site,tags,note}
//!                              → 记下元数据，等文件入库后自动挂上
//!   GET  /stats                → 库概览，脚本面板上显示

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use crate::AppState;

pub const PORT: u16 = 17890;

pub fn spawn(state: AppState) {
    std::thread::spawn(move || {
        let Ok(listener) = TcpListener::bind(("127.0.0.1", PORT)) else {
            // 端口被占就算了：影匣本体不受影响，脚本会提示连不上
            return;
        };
        for stream in listener.incoming().flatten() {
            let st = state.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &st);
            });
        }
    });
}

fn handle(mut stream: TcpStream, state: &AppState) -> std::io::Result<()> {
    let (method, path, body) = match read_request(&mut stream) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let (status, payload) = match (method.as_str(), path.split('?').next().unwrap_or("")) {
        ("OPTIONS", _) => (204, String::new()),
        ("GET", "/ping") => (
            200,
            format!(
                r#"{{"ok":true,"app":"影匣","version":"{}","port":{}}}"#,
                env!("CARGO_PKG_VERSION"),
                PORT
            ),
        ),
        ("GET", "/check") => {
            let url = query_param(&path, "url").unwrap_or_default();
            match crate::with_library(state, |lib| crate::db::find_by_url(&lib.conn, &url)) {
                Ok(Some(v)) => (
                    200,
                    format!(
                        r#"{{"ok":true,"found":true,"video":{{"id":{},"file_name":{},"folder_no":{},"slot_no":{}}}}}"#,
                        v.id,
                        json_str(&v.file_name),
                        v.folder_no,
                        v.slot_no
                    ),
                ),
                Ok(None) => (200, r#"{"ok":true,"found":false}"#.into()),
                Err(e) => (500, format!(r#"{{"ok":false,"error":{}}}"#, json_str(&e))),
            }
        }
        ("GET", "/stats") => match crate::with_library(state, |lib| {
            let total: i64 = lib
                .conn
                .query_row("SELECT COUNT(*) FROM videos WHERE trashed=0", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let root = lib.root.to_string_lossy().to_string();
            Ok((total, root))
        }) {
            Ok((total, root)) => (
                200,
                format!(
                    r#"{{"ok":true,"videos":{},"library":{}}}"#,
                    total,
                    json_str(&root)
                ),
            ),
            Err(e) => (500, format!(r#"{{"ok":false,"error":{}}}"#, json_str(&e))),
        },
        ("POST", "/hook") => match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => {
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let (url, title, author, site, tags, note) = (
                    s("url"),
                    s("title"),
                    s("author"),
                    s("site"),
                    s("tags"),
                    s("note"),
                );
                if url.is_empty() {
                    (400, r#"{"ok":false,"error":"url 不能为空"}"#.into())
                } else {
                    match crate::with_library(state, |lib| {
                        crate::db::submit_hook(&lib.conn, &url, &title, &author, &site, &tags, &note)
                    }) {
                        Ok((id, dup)) => (
                            200,
                            format!(
                                r#"{{"ok":true,"id":{},"duplicate":{}}}"#,
                                id,
                                match dup {
                                    Some(v) => v.to_string(),
                                    None => "null".into(),
                                }
                            ),
                        ),
                        Err(e) => (500, format!(r#"{{"ok":false,"error":{}}}"#, json_str(&e))),
                    }
                }
            }
            Err(e) => (400, format!(r#"{{"ok":false,"error":{}}}"#, json_str(&e.to_string()))),
        },
        _ => (404, r#"{"ok":false,"error":"not found"}"#.into()),
    };

    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Access-Control-Allow-Private-Network: true\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        if h == "\r\n" || h == "\n" {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    // 防止异常请求把内存打爆
    let content_length = content_length.min(1 << 20);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    Ok((method, path, body))
}

fn query_param(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}
