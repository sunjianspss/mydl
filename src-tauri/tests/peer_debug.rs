//! 开 debug 日志跑一次预览，看连 peer 时到底发生了什么。
//!
//!   MYDL_PROBE_MAGNET='magnet:?xt=...' cargo test --test peer_debug -- --ignored --nocapture

use std::time::Instant;
use mydl_lib::engine::Engine;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "诊断用"]
async fn preview_with_peer_logs() {
    let Ok(magnet) = std::env::var("MYDL_PROBE_MAGNET") else {
        eprintln!("没设 MYDL_PROBE_MAGNET，跳过");
        return;
    };

    tracing_subscriber::fmt()
        .with_env_filter("info,librqbit=debug,librqbit_utp=info")
        .with_ansi(false)
        .init();

    let tmp = std::env::temp_dir().join(format!("mydl-peerdbg-{}", std::process::id()));
    let engine = Engine::new(tmp.join("dl"), Some(tmp.join("state")), tmp.join("state"), Vec::new())
        .await
        .expect("创建 Engine 失败");

    let t0 = Instant::now();
    match engine.preview(&magnet).await {
        Ok(Some(p)) => eprintln!(">>> 成功 用时 {:?}：{}（{} 个文件）", t0.elapsed(), p.name, p.files.len()),
        Ok(None) => eprintln!(">>> 被取消 用时 {:?}", t0.elapsed()),
        Err(e) => eprintln!(">>> 失败 用时 {:?}：{e:#}", t0.elapsed()),
    }
    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
}
