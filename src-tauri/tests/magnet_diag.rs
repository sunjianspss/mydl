//! 诊断：对一个确定健在的 swarm（Debian 官方镜像），分别用
//! 「带 tracker 的磁力链」和「裸 info-hash 磁力链」走一遍真实添加路径。
//!
//! 这能区分三种情况：两个都成 = 用户的链接确实没人做种；
//! 只有带 tracker 的成 = 我们的 DHT 解析有问题；两个都失败 = 磁力链路径整个坏了。

use std::time::Instant;
use mydl_lib::engine::Engine;

const IH: &str = "481b6e3617be4c88f96cb25e47c9d8272130071e";

async fn try_magnet(tag: &str, magnet: &str) -> bool {
    let tmp = std::env::temp_dir().join(format!("mydl-diag-{}-{tag}", std::process::id()));
    let engine = Engine::new(tmp.join("dl"), Some(tmp.join("state")))
        .await
        .expect("创建 Engine 失败");

    let t0 = Instant::now();
    let result = engine.preview(magnet).await;
    let elapsed = t0.elapsed();
    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    match result {
        Ok(p) => {
            eprintln!("  [{tag}] 成功 用时 {elapsed:?} — {} ({} 个文件)", p.name, p.files.len());
            true
        }
        Err(e) => {
            eprintln!("  [{tag}] 失败 用时 {elapsed:?} — {e:#}");
            false
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "诊断用，需要外网，最长跑 4 分钟"]
async fn magnet_with_and_without_trackers() {
    let full = format!(
        "magnet:?xt=urn:btih:{IH}&dn=debian-13.6.0-amd64-netinst.iso\
         &tr=http%3A//bttracker.debian.org%3A6969/announce"
    );
    let bare = format!("magnet:?xt=urn:btih:{IH}");

    eprintln!("对照实验：同一个 Debian 官方种子（swarm 确定健在）");
    let with_tr = try_magnet("带tracker", &full).await;
    let no_tr = try_magnet("裸infohash", &bare).await;

    eprintln!();
    eprintln!("结论：带 tracker = {with_tr}，裸 info-hash = {no_tr}");
}
