//! 六牙象·连萌：隐私提示标签 —— 主进程（被控端 server 进程）侧。
//!
//! 职责：
//!   - 维护"当前有谁在看我的屏幕"的名单（conn_id -> 显示名）；
//!   - 名单从空变非空时拉起提示条子进程，从非空变空时让它退出；
//!   - 名单变化时把最新名单推给子进程。

use super::NoticeEvent;
#[cfg(target_os = "windows")]
use crate::ipc::{self, Data};
use hbb_common::log;
#[cfg(target_os = "windows")]
use hbb_common::{
    allow_err, anyhow::anyhow, bail, sleep,
    tokio::{
        self,
        sync::mpsc::{unbounded_channel, UnboundedSender},
    },
    ResultType,
};
use lazy_static::lazy_static;
use std::collections::BTreeMap;
use std::sync::RwLock;

lazy_static! {
    /// conn_id -> 观看者显示名。用 BTreeMap 保证名单顺序稳定，避免文案抖动。
    static ref VIEWERS: RwLock<BTreeMap<i32, String>> = Default::default();
}

#[cfg(target_os = "windows")]
lazy_static! {
    static ref TX_NOTICE: RwLock<Option<UnboundedSender<NoticeEvent>>> = RwLock::new(None);
}

/// 观看者名字为空时的兜底显示（例如对端没设置昵称）。
fn normalize_name(name: &str, peer_id: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let peer_id = peer_id.trim();
    if !peer_id.is_empty() {
        return peer_id.to_string();
    }
    "他人".to_string()
}

/// 有人开始查看本机屏幕。
pub fn register_viewer(conn_id: i32, name: &str, peer_id: &str) {
    let display = normalize_name(name, peer_id);
    {
        let mut viewers = VIEWERS.write().unwrap();
        if viewers.get(&conn_id).map(|v| v.as_str()) == Some(display.as_str()) {
            return;
        }
        viewers.insert(conn_id, display);
    }
    log::info!("privacy notice: viewer {} registered", conn_id);
    ensure_started();
    broadcast();
}

/// 某个查看者断开。
pub fn unregister_viewer(conn_id: i32) {
    let is_empty = {
        let mut viewers = VIEWERS.write().unwrap();
        if viewers.remove(&conn_id).is_none() {
            return;
        }
        viewers.is_empty()
    };
    log::info!("privacy notice: viewer {} unregistered", conn_id);
    if is_empty {
        stop();
    } else {
        broadcast();
    }
}

fn current_viewers() -> Vec<String> {
    VIEWERS.read().unwrap().values().cloned().collect()
}

// ---------------------------------------------------------------------------
// Windows 实现
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn ensure_started() {
    if TX_NOTICE.read().unwrap().is_some() {
        return;
    }
    std::thread::spawn(|| {
        if let Err(e) = start_notice_() {
            log::error!("Failed to start privacy notice: {}", e);
        }
    });
}

#[cfg(target_os = "windows")]
fn broadcast() {
    let viewers = current_viewers();
    if let Some(tx) = TX_NOTICE.read().unwrap().as_ref() {
        allow_err!(tx.send(NoticeEvent::Viewers(viewers)));
    }
}

#[cfg(target_os = "windows")]
fn stop() {
    std::thread::spawn(|| {
        let mut tx_notice = TX_NOTICE.write().unwrap();
        if let Some(tx) = tx_notice.as_ref() {
            allow_err!(tx.send(NoticeEvent::Exit));
            // 简单等待子进程退出，和 whiteboard 的做法保持一致。
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        tx_notice.take();
    });
}

#[cfg(target_os = "windows")]
#[tokio::main(flavor = "current_thread")]
async fn start_notice_() -> ResultType<()> {
    let mut tx_guard = TX_NOTICE.write().unwrap();
    if tx_guard.is_some() {
        log::warn!("privacy notice already started");
        return Ok(());
    }

    // 登录界面（winlogon 桌面）下不弹提示条，等进入用户桌面。
    loop {
        if !crate::platform::is_prelogin() {
            break;
        }
        sleep(1.).await;
    }

    let mut stream = None;
    if let Ok(s) = ipc::connect(1000, super::IPC_POSTFIX).await {
        stream = Some(s);
    } else {
        let args = vec!["--privacy-notice"];
        let mut run_done = false;
        if crate::platform::is_root() {
            // 被控端主进程通常是 SYSTEM 服务，必须切到当前登录用户的会话里才看得见窗口。
            let mut res = Ok(None);
            for _ in 0..10 {
                log::debug!("Start privacy notice");
                res = crate::platform::run_as_user(args.clone());
                if res.is_ok() {
                    break;
                }
                log::error!("Failed to run privacy notice: {res:?}");
                sleep(1.).await;
            }
            if let Some(task) = res? {
                crate::CHILD_PROCESS.lock().unwrap().push(task);
            }
            run_done = true;
        }
        if !run_done {
            log::debug!("Start privacy notice");
            crate::CHILD_PROCESS
                .lock()
                .unwrap()
                .push(crate::run_me(args)?);
        }
        for _ in 0..20 {
            sleep(0.3).await;
            if let Ok(s) = ipc::connect(1000, super::IPC_POSTFIX).await {
                stream = Some(s);
                break;
            }
        }
        if stream.is_none() {
            bail!("Failed to connect to privacy notice process");
        }
    }

    let mut stream = stream.ok_or(anyhow!("none stream"))?;
    let (tx, mut rx) = unbounded_channel();
    tx_guard.replace(tx);
    drop(tx_guard);
    let _call_on_ret = crate::common::SimpleCallOnReturn {
        b: true,
        f: Box::new(move || {
            let _ = TX_NOTICE.write().unwrap().take();
        }),
    };

    // 建连后先把当前名单同步过去，避免子进程停留在默认文案。
    allow_err!(stream
        .send(&Data::PrivacyNotice(NoticeEvent::Viewers(current_viewers())))
        .await);

    loop {
        tokio::select! {
            Some(evt) = rx.recv() => {
                let is_exit = matches!(evt, NoticeEvent::Exit);
                if let Err(e) = stream.send(&Data::PrivacyNotice(evt)).await {
                    log::error!("privacy notice ipc send error: {}", e);
                    break;
                }
                if is_exit {
                    break;
                }
            }
            res = stream.next() => {
                match res {
                    Err(e) => {
                        log::info!("privacy notice ipc closed: {}", e);
                        break;
                    }
                    Ok(None) => {
                        log::info!("privacy notice ipc closed");
                        break;
                    }
                    Ok(Some(_)) => {}
                }
            }
        }
    }

    // 子进程意外退出（例如被任务管理器强杀）而此时仍有人在看 —— 重新拉起。
    // 这是"不可隐藏"的兜底：关掉它，它会自己回来。
    if !VIEWERS.read().unwrap().is_empty() {
        log::warn!("privacy notice exited unexpectedly while being viewed, restarting");
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(800));
            ensure_started();
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 其它平台：暂不实现（Android 学生端另有方案），保持调用点无需加 cfg。
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
fn ensure_started() {}

#[cfg(not(target_os = "windows"))]
fn broadcast() {}

#[cfg(not(target_os = "windows"))]
fn stop() {}
