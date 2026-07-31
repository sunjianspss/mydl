//! 有任务在下载时阻止电脑休眠，下完自动解除。
//!
//! 平台实现（macOS 的 caffeinate、Windows 的 SetThreadExecutionState）在
//! `platform.rs`，这里只管什么时候该开、什么时候该关。

use std::sync::Arc;
use std::time::Duration;

use crate::engine::{Engine, TorrentView};
use crate::platform::SleepBlocker;
use crate::settings::SettingsStore;

const POLL: Duration = Duration::from_secs(15);

/// 只有「正在下载」才需要保持唤醒。做种（已完成但仍是 live）不算 ——
/// 没道理为了给别人上传就让电脑整夜不睡。
pub fn should_stay_awake(enabled: bool, torrents: &[TorrentView]) -> bool {
    enabled && torrents.iter().any(|t| t.state == "live" && !t.finished)
}

pub fn spawn(engine: Arc<Engine>, store: Arc<SettingsStore>) {
    // 用独立的 OS 线程而不是 tokio 任务：Windows 的 SetThreadExecutionState
    // 是**按线程**记的，任务在 worker 线程之间迁移的话，解除会发生在另一个
    // 线程上，原线程那份要求就永远留着了 —— 电脑再也不会自己睡。
    //
    // 循环体本来也全是同步调用（engine.list()、store.get()），不需要 async。
    std::thread::spawn(move || {
        let mut blocker = SleepBlocker::default();

        loop {
            std::thread::sleep(POLL);
            let want = should_stay_awake(
                store.get().prevent_sleep_while_downloading,
                &engine.list(),
            );
            blocker.set(want);
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
