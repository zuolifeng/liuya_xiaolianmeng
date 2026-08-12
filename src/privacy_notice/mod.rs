//! 六牙象·连萌：屏幕被查看时的隐私提示标签。
//!
//! 当本机屏幕正被远端查看（远程控制 / 摄像头查看）时，在屏幕上显示一条
//! 始终置顶的提示："屏幕正在被 XXX 查看，注意保护隐私"。
//!
//! 产品约束（来自需求）：
//!   1. 可以用鼠标拖动改变位置；
//!   2. 不提供关闭 / 隐藏入口；
//!   3. 不能被拖出屏幕（始终限制在显示器可见区域内）。
//!
//! 实现方式沿用仓库内已有的 `whiteboard` 浮层框架：独立子进程 + IPC 通道，
//! 子进程用 tao 创建一个透明、无边框、置顶、不进任务栏的小窗口，
//! 用 tiny-skia 直接绘制。这样可以避开 Flutter 侧多窗口 + 置顶 + 透明的坑。

use serde_derive::{Deserialize, Serialize};

mod client;
pub use client::*;

#[cfg(target_os = "windows")]
mod server;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use server::run;

/// 主进程 → 提示条子进程 的事件。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "t", content = "c")]
pub enum NoticeEvent {
    /// 当前正在查看本机屏幕的人（显示名列表，已按 conn_id 去重排序）。
    /// 空列表表示当前无人查看，子进程应当自行退出。
    Viewers(Vec<String>),
    /// 请求子进程退出。
    Exit,
}

/// IPC 通道名（与 `whiteboard` 的 `_whiteboard` 同级）。
pub const IPC_POSTFIX: &str = "_privacy_notice";

/// 记住上次拖动到的位置，key 存在 LocalConfig 里，形如 "x,y"（物理像素）。
pub const CONFIG_KEY_POS: &str = "privacy-notice-pos";

/// 由观看者名单拼出提示文案。
pub fn build_notice_text(viewers: &[String]) -> String {
    let who = if viewers.is_empty() {
        "他人".to_string()
    } else {
        viewers.join("、")
    };
    format!("屏幕正在被 {} 查看，注意保护隐私", who)
}
