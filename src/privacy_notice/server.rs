//! 六牙象·连萌：隐私提示标签 —— 子进程侧（`--privacy-notice` 启动）。
//!
//! 一个线程跑 IPC 监听，主线程跑 tao 事件循环（Windows 上窗口必须在建它的线程里泵消息）。

use super::NoticeEvent;
use crate::ipc::{new_listener, Connection, Data};
use hbb_common::{
    allow_err, log,
    tokio::{
        self,
        sync::mpsc::{unbounded_channel, UnboundedReceiver},
    },
    ResultType,
};
use lazy_static::lazy_static;
use std::sync::RwLock;
use tao::event_loop::EventLoopProxy;

lazy_static! {
    pub(super) static ref EVENT_PROXY: RwLock<Option<EventLoopProxy<NoticeEvent>>> =
        RwLock::new(None);
}

pub fn run() {
    let (tx_exit, rx_exit) = unbounded_channel();
    std::thread::spawn(move || {
        start_ipc(rx_exit);
    });
    if let Err(e) = super::windows::create_event_loop() {
        log::error!("Failed to create privacy notice event loop: {}", e);
        tx_exit.send(()).ok();
    }
}

#[tokio::main(flavor = "current_thread")]
pub(super) async fn start_ipc(mut rx_exit: UnboundedReceiver<()>) {
    match new_listener(super::IPC_POSTFIX).await {
        Ok(mut incoming) => loop {
            tokio::select! {
                _ = rx_exit.recv() => {
                    log::info!("privacy notice: exiting IPC");
                    break;
                }
                res = incoming.next() => match res {
                    Some(Ok(stream)) => {
                        log::debug!("privacy notice: got new connection");
                        tokio::spawn(handle_new_stream(Connection::new(stream)));
                    }
                    Some(Err(err)) => {
                        log::error!("privacy notice: couldn't get client: {:?}", err);
                    }
                    None => {
                        log::error!("privacy notice: failed to get client");
                        break;
                    }
                }
            }
        },
        Err(err) => {
            log::error!("privacy notice: failed to start ipc server: {}", err);
        }
    }
}

async fn handle_new_stream(mut conn: Connection) {
    loop {
        match conn.next().await {
            Err(err) => {
                log::info!("privacy notice ipc connection closed: {}", err);
                break;
            }
            Ok(None) => {
                log::info!("privacy notice ipc connection closed");
                break;
            }
            Ok(Some(Data::PrivacyNotice(evt))) => {
                let is_exit = matches!(evt, NoticeEvent::Exit);
                if let Some(ep) = EVENT_PROXY.read().unwrap().as_ref() {
                    allow_err!(ep.send_event(evt));
                }
                if is_exit {
                    return;
                }
            }
            Ok(Some(_)) => {}
        }
    }
    // 主进程连接断了（服务重启 / 崩溃），提示条没有存在意义，跟着退出。
    if let Some(ep) = EVENT_PROXY.read().unwrap().as_ref() {
        allow_err!(ep.send_event(NoticeEvent::Exit));
    }
}

/// 所有显示器合并出来的可见矩形（物理像素）。提示条不能被拖出这个矩形。
pub(super) fn get_displays_rect() -> ResultType<(i32, i32, u32, u32)> {
    let displays = crate::server::display_service::try_get_displays()?;
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for display in displays {
        let (x, y) = (display.origin().0 as i32, display.origin().1 as i32);
        let (w, h) = (display.width() as i32, display.height() as i32);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }
    if min_x == i32::MAX || max_x <= min_x || max_y <= min_y {
        hbb_common::bail!("no display found");
    }
    Ok((
        min_x,
        min_y,
        (max_x - min_x) as u32,
        (max_y - min_y) as u32,
    ))
}
