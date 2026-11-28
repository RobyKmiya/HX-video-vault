//! 出网代理探测。
//!
//! 影匣的 HTTP 层用系统 curl，但 curl **不读 WinINET 设置**：用户双击图标启动时
//! 进程环境里没有 `http_proxy`，curl 就直连，浏览器能打开的站点影匣一律超时。
//! 这里按「用户显式指定 → Windows 系统代理 → 环境变量」的顺序找一个代理并缓存，
//! 所有 curl 调用统一带上 `--proxy`。
//!
//! 注意：代理只交给 curl 子进程（命令行参数），**不改进程环境变量**——否则
//! WebView2 会把 127.0.0.1 的内置页面也丢给代理，界面直接 ERR_CONNECTION_REFUSED。

use std::process::Command;

/// 返回 curl `-x/--proxy` 接受的形式，如 `http://127.0.0.1:30808`。
pub fn proxy_url() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(detect).clone()
}

/// 给 curl 命令挂上代理参数；没有可用代理时不动命令，返回是否挂了。
pub fn apply(cmd: &mut Command) -> bool {
    match proxy_url() {
        Some(p) => {
            cmd.args(["--proxy", &p]);
            true
        }
        None => false,
    }
}

fn detect() -> Option<String> {
    // 排障开关：VIDEO_VAULT_PROXY=none 强制直连；给了值就按值用
    if let Ok(v) = std::env::var("VIDEO_VAULT_PROXY") {
        let v = v.trim();
        if v.eq_ignore_ascii_case("none") {
            return None;
        }
        if !v.is_empty() {
            return normalize(v);
        }
    }
    // 系统代理＝浏览器正在用的那一个，优先信它
    #[cfg(windows)]
    if let Some(p) = system_proxy() {
        return Some(p);
    }
    ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find_map(|v| normalize(&v))
}

#[cfg(windows)]
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

#[cfg(windows)]
fn system_proxy() -> Option<String> {
    if reg_dword("ProxyEnable").unwrap_or(0) == 0 {
        return None;
    }
    reg_string("ProxyServer").and_then(|s| normalize(&s))
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn reg_dword(name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let sub = wide(INTERNET_SETTINGS);
    let val = wide(name);
    let mut data: u32 = 0;
    let mut cb: u32 = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut cb,
        )
    };
    (rc == 0).then_some(data)
}

#[cfg(windows)]
fn reg_string(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let sub = wide(INTERNET_SETTINGS);
    let val = wide(name);
    let mut buf = [0u16; 1024];
    let mut cb: u32 = (buf.len() * std::mem::size_of::<u16>()) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut cb,
        )
    };
    if rc != 0 {
        return None;
    }
    let chars = (cb as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars]))
}

/// 规范化代理串：
/// - `127.0.0.1:30808` → `http://127.0.0.1:30808`
/// - `http=127.0.0.1:30808;https=127.0.0.1:30808`（WinINET 按协议分段）→ 取 https 段
/// - `socks5://...` 原样保留
fn normalize(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let pick = if t.contains('=') {
        let (mut http, mut https) = (None, None);
        for seg in t.split(';') {
            let Some((k, v)) = seg.split_once('=') else { continue };
            match k.trim().to_ascii_lowercase().as_str() {
                "https" => https = Some(v.trim().to_string()),
                "http" => http = Some(v.trim().to_string()),
                _ => {}
            }
        }
        https.or(http)?
    } else {
        t.to_string()
    };
    let pick = pick.trim();
    if pick.is_empty() {
        return None;
    }
    let lower = pick.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks4")
        || lower.starts_with("socks5")
    {
        Some(pick.to_string())
    } else {
        Some(format!("http://{pick}"))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalizes_proxy_forms() {
        assert_eq!(normalize("127.0.0.1:30808").as_deref(), Some("http://127.0.0.1:30808"));
        assert_eq!(
            normalize("http=127.0.0.1:30808;https=127.0.0.1:30809").as_deref(),
            Some("http://127.0.0.1:30809")
        );
        assert_eq!(normalize("socks5://127.0.0.1:1080").as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(normalize("   ").as_deref(), None);
    }
}
