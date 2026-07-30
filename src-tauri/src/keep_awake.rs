//! 有任务在下载时阻止电脑休眠，下完自动解除。
//!
//! 走 `caffeinate` 子进程而不是直接调 IOKit：少一层 FFI，行为也和系统自带
//! 工具完全一致，`pmset -g assertions` 里能看到是谁在阻止休眠。
//!
//! 关键是 `-w <自己的 pid>`：万一 App 被强杀、来不及 kill 子进程，
//! caffeinate 也会跟着退出，不会留一个进程让电脑永远睡不着。

use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use crate::engine::{Engine, TorrentView};
use crate::settings::SettingsStore;

const POLL: Duration = Duration::from_secs(15);

/// 只有「正在下载」才需要保持唤醒。做种（已完成但仍是 live）不算 ——
/// 没道理为了给别人上传就让电脑整夜不睡。
pub fn should_stay_awake(enabled: bool, torrents: &[TorrentView]) -> bool {
    enabled && torrents.iter().any(|t| t.state == "live" && !t.finished)
}

fn start() -> std::io::Result<Child> {
    Command::new("/usr/bin/caffeinate")
        // -i 禁止闲置休眠，-m 禁止磁盘休眠，-s 禁止系统休眠（仅接电源时有效）
        .args(["-i", "-m", "-s", "-w", &std::process::id().to_string()])
        .spawn()
}

pub fn spawn(engine: Arc<Engine>, store: Arc<SettingsStore>) {
    tauri::async_runtime::spawn(async move {
        let mut guard: Option<Child> = None;

        loop {
            tokio::time::sleep(POLL).await;

            let want = should_stay_awake(
                store.get().prevent_sleep_while_downloading,
                &engine.list(),
            );

            match (want, guard.as_mut()) {
                (true, None) => match start() {
                    Ok(child) => {
                        tracing::info!(pid = child.id(), "有任务在下载，已阻止休眠");
                        guard = Some(child);
                    }
                    Err(e) => tracing::warn!("启动 caffeinate 失败：{e}"),
                },
                (false, Some(child)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::info!("没有正在下载的任务，已解除阻止休眠");
                    guard = None;
                }
                // 子进程意外没了（比如被人手动 kill），下一轮会重新拉起。
                (true, Some(child)) => {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        tracing::warn!("caffeinate 意外退出，将重新启动");
                        guard = None;
                    }
                }
                (false, None) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(state: &str, finished: bool) -> TorrentView {
        TorrentView {
            id: 0,
            name: "t".into(),
            info_hash: "h".into(),
            state: state.into(),
            error: None,
            finished,
            progress_bytes: 0,
            total_bytes: 100,
            uploaded_bytes: 0,
            download_speed_bps: 0.0,
            upload_speed_bps: 0.0,
            peers_live: 0,
            eta: None,
        }
    }

    #[test]
    fn stays_awake_only_while_downloading() {
        assert!(should_stay_awake(true, &[torrent("live", false)]));

        // 做种不该阻止休眠
        assert!(!should_stay_awake(true, &[torrent("live", true)]));
        assert!(!should_stay_awake(true, &[torrent("paused", false)]));
        assert!(!should_stay_awake(true, &[]));

        // 多任务里只要有一个在下就够
        assert!(should_stay_awake(
            true,
            &[torrent("live", true), torrent("live", false)]
        ));
    }

    #[test]
    fn respects_the_setting() {
        assert!(!should_stay_awake(false, &[torrent("live", false)]));
    }
}
