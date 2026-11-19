//! 子进程启动的统一入口。
//!
//! Windows 上 GUI 进程（影匣没有控制台）启动控制台程序时，系统会给每个子进程
//! 新建一个 conhost 窗口 —— ffprobe / ffmpeg / curl 一多，屏幕上就是一片黑框闪烁。
//! 所有调用点统一走 `quiet()`，加 CREATE_NO_WINDOW 静默掉。
//!
//! 注意：mpv / VLC 自己的播放窗口不受影响，这个标志只影响控制台。

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 建一个不会弹控制台窗口的 Command。
pub fn quiet<S: AsRef<OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn quiet_command_runs_without_console() {
        // 用系统自带 cmd 验证：能跑通、拿到输出，说明 creation_flags 没把进程搞坏
        let out = quiet("cmd")
            .args(["/C", "echo", "ok"])
            .output()
            .expect("cmd 应能启动");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
    }
}
