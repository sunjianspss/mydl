//! 真连网的冒烟测试：确认协议栈端到端能跑通（解析种子 → tracker/DHT 找 peer → 收到数据）。
//!
//! 默认被 `#[ignore]` 跳过，因为它需要外网、会真的下载几 MB 数据，而且依赖
//! Ubuntu 官方种子仍在做种。手动跑：
//!
//! ```
//! cargo test --test engine_smoke -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use mydl_lib::engine::Engine;

/// Ubuntu 官方镜像种子，合法且长期有大量做种者。
const TORRENT_URL: &str =
    "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso.torrent";

/// 收到这么多字节就算协议栈跑通了，不必下完整个 ISO。
const ENOUGH_BYTES: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会实际下载约 1MB 数据"]
async fn downloads_real_torrent() {
    let tmp = std::env::temp_dir().join(format!("mydl-smoke-{}", std::process::id()));
    let downloads = tmp.join("downloads");
    let state = tmp.join("state");
    std::fs::create_dir_all(&state).unwrap();

    let engine = Engine::new(downloads, Some(state))
        .await
        .expect("创建 Engine 失败");

    let id = engine.add(TORRENT_URL, None).await.expect("添加任务失败");

    let started = Instant::now();
    let mut last_report = Instant::now();
    let outcome = loop {
        let view = engine
            .list()
            .into_iter()
            .find(|t| t.id == id)
            .expect("任务从列表里消失了");

        if let Some(e) = &view.error {
            break Err(format!("任务报错：{e}"));
        }
        if view.progress_bytes >= ENOUGH_BYTES {
            break Ok(view);
        }
        if started.elapsed() > TIMEOUT {
            break Err(format!(
                "{}s 内只下到 {} 字节，peers={}，state={}",
                TIMEOUT.as_secs(),
                view.progress_bytes,
                view.peers_live,
                view.state
            ));
        }
        if last_report.elapsed() > Duration::from_secs(5) {
            eprintln!(
                "  [{:>3}s] {} bytes, {} peers, {}",
                started.elapsed().as_secs(),
                view.progress_bytes,
                view.peers_live,
                view.state
            );
            last_report = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    let view = outcome.unwrap_or_else(|e| panic!("{e}"));
    assert!(view.total_bytes > 0, "没拿到种子元信息");
    assert!(view.peers_live > 0, "没连上任何 peer");
    eprintln!(
        "OK: {} — {} / {} bytes, {} peers",
        view.name, view.progress_bytes, view.total_bytes, view.peers_live
    );
}
