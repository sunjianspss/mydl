//! A/B 对比：BT 流量走默认路由（VPN 隧道）vs 绑到物理网卡直出。
//!
//! 开着全局 VPN 时默认路由指向 utun*，BT 也跟着走隧道。隧道出口多半是机房
//! IP，会被大量 BT 客户端屏蔽；UPnP 也映射不上，没有入站连接。这个用例拿
//! 同一条磁力链跑两遍，直接量出差距。
//!
//! ```
//! MYDL_BIND_DEVICE=en0 \
//!   cargo test --test bind_device_live -- --ignored --nocapture
//! ```
//!
//! 两个实例都用随机端口和独立 DHT 缓存，不碰正在跑的 App。

use std::time::Instant;

use mydl_lib::engine::{Engine, SessionSetup};

/// Ubuntu 官方种子：境外服务器做种、长期存在，两边都该能连上。
/// 用它而不是热门影视资源 —— 后者的 peer 更可能屏蔽机房 IP，会放大差距，
/// 而我们想量的是「基线连通性」。
const TORRENT: &str = "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso.torrent";

/// 每边跑多久。
const WINDOW_SECS: u64 = 45;

async fn run(label: &str, bind_device: Option<String>) -> (usize, u64) {
    let tag = bind_device.clone().unwrap_or_else(|| "默认路由".into());
    let tmp = std::env::temp_dir().join(format!(
        "mydl-bind-{}-{}",
        label,
        std::process::id()
    ));
    let engine = Engine::new(
        tmp.join("dl"),
        Some(tmp.join("state")),
        tmp.join("state"),
        SessionSetup {
            bind_device,
            ..Default::default()
        },
    )
    .await
    .expect("创建 Engine 失败");

    let id = engine.add(TORRENT, None).await.expect("添加失败");

    let started = Instant::now();
    let (mut peak_peers, mut bytes) = (0usize, 0u64);
    while started.elapsed().as_secs() < WINDOW_SECS {
        if let Some(v) = engine.list().into_iter().find(|t| t.id == id) {
            peak_peers = peak_peers.max(v.peers_live);
            bytes = v.progress_bytes;
        }
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    }

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!(
        "  {tag:<12} 峰值 peer {peak_peers:>3}   下到 {:.1} MB",
        bytes as f64 / 1048576.0
    );
    (peak_peers, bytes)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会实际下载数据，跑约 100 秒"]
async fn direct_vs_tunnel() {
    let Ok(device) = std::env::var("MYDL_BIND_DEVICE") else {
        eprintln!("没设 MYDL_BIND_DEVICE（比如 en0），跳过");
        return;
    };

    eprintln!("各跑 {WINDOW_SECS} 秒：");
    let (tunnel_peers, tunnel_bytes) = run("tunnel", None).await;
    let (direct_peers, direct_bytes) = run("direct", Some(device.clone())).await;

    eprintln!(
        "\n结论：绑 {device} 峰值 peer {direct_peers} / 默认路由 {tunnel_peers}；\
         下载量 {:.1} MB / {:.1} MB",
        direct_bytes as f64 / 1048576.0,
        tunnel_bytes as f64 / 1048576.0
    );

    // 不断言谁一定更快 —— 这是台特定机器上的网络测量，结果本来就会变。
    // 只保证绑定这条路能真的跑起来，不是配了个不生效的开关。
    assert!(
        direct_peers > 0 || direct_bytes > 0,
        "绑到 {device} 之后一个 peer 都没连上、一个字节都没下到 —— \
         这个网卡多半选错了，或者绑定没生效"
    );
}
