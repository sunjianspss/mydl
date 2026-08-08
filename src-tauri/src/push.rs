//! 把任务列表推给界面，而不是让界面每秒来问一次。
//!
//! # 为什么不是「前端定时 invoke」
//!
//! 之前是前端每 1 秒调两个命令（`list_torrents` + `session_status`）。它其实
//! 已经不傻了 —— 窗口失焦就停、全都停着就降到 5 秒 —— 但有个改不掉的毛病：
//! **哪怕一个字节都没变，也照样来回搬一趟数据、照样触发一次 React 重渲染**。
//! 一屋子任务全在暂停时，那就是纯粹的空转。
//!
//! 换成推送之后，轮询还在，只是挪到了 Rust 这边（librqbit 没有变更回调，
//! 总得有人去问），但**只有内容真的变了才发**。
//!
//! # 说清楚它买到了什么、没买到什么
//!
//! 买到的是「静止时彻底安静」：全暂停 / 只做种没速度的会话，一个消息都不发，
//! 前端一次都不重渲染。
//!
//! **没买到的是「下载中省 CPU」**：正在下的时候速度和已下字节每秒都在变，
//! 该发还是每秒发一条。这条路省不掉，也不该假装省得掉。
//!
//! # 失焦时照样不干活
//!
//! 这条纪律是从前端搬过来的，不能丢：没人在看的时候，连 `engine.list()`
//! 都不该调。重新拿到焦点时把「上次发了什么」清掉，保证立刻补一条完整的。

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::{Engine, SessionStatus, TorrentView};

/// 有任务在跑时的采样间隔。速度数字要跟手。
const TICK_ACTIVE: Duration = Duration::from_secs(1);
/// 全都停着时的间隔。没什么可看的，不用那么勤。
const TICK_IDLE: Duration = Duration::from_secs(5);

/// 事件名。前端 `listen` 的就是它。
pub const EVENT: &str = "torrents";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub torrents: Vec<TorrentView>,
    pub status: SessionStatus,
}

pub fn spawn(app: AppHandle, engine: Arc<Engine>) {
    tauri::async_runtime::spawn(async move {
        // 上一次发出去的内容。用序列化后的字符串比，省得给每个 View 结构
        // 都加 PartialEq —— 反正一会儿也要序列化着发出去。
        let mut last: Option<String> = None;

        loop {
            let focused = app
                .get_webview_window("main")
                .and_then(|w| w.is_focused().ok())
                .unwrap_or(false);

            if !focused {
                // 清掉记录：切回来时哪怕内容没变也要补发一条，否则界面
                // 停在离开前的那一帧，等下一次真的有变化才醒过来。
                last = None;
                tokio::time::sleep(TICK_IDLE).await;
                continue;
            }

            let torrents = engine.list();
            let active = torrents.iter().any(|t| t.state == "live");
            let snapshot = Snapshot {
                torrents,
                status: engine.session_status(),
            };

            match serde_json::to_string(&snapshot) {
                Ok(json) if last.as_deref() == Some(json.as_str()) => {}
                Ok(json) => {
                    if let Err(e) = app.emit(EVENT, &snapshot) {
                        tracing::warn!("推送任务列表失败：{e}");
                    }
                    last = Some(json);
                }
                // 序列化都失败的话比就没法比了，那就照发不误。
                Err(e) => {
                    tracing::warn!("序列化任务列表失败：{e}");
                    let _ = app.emit(EVENT, &snapshot);
                    last = None;
                }
            }

            tokio::time::sleep(if active { TICK_ACTIVE } else { TICK_IDLE }).await;
        }
    });
}
