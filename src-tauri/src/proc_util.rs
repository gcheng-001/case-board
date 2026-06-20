//! 跨平台子进程小工具。

use tokio::process::Command;

/// Windows 下隐藏子进程的控制台窗口(`CREATE_NO_WINDOW`),避免 spawn 外部命令
/// (python / lark-cli 等)时闪一个黑色命令框。非 Windows 平台是 no-op。
///
/// 坑 #21:Windows 用户反馈「点辅助立案一直弹命令框」「飞书日历后台闪窗」,根因是
/// `Command::new` spawn 控制台子进程默认会带窗口。新增 spawn 外部命令的代码记得调一下。
pub(crate) fn hide_console_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 同 [`hide_console_window`],但作用于标准库的 [`std::process::Command`]
/// (本仓库不少 spawn 用的是 `std::process::Command` 而非 tokio 版)。
pub(crate) fn hide_console_window_std(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}
