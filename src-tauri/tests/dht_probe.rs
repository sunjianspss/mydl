//! 直接问 DHT：某个 info-hash 到底有没有 peer。
//!
//! 这能区分两件完全不同的事：
//!   A. DHT 里根本找不到这个 swarm  → 换任何客户端都一样，多半是私有种子
//!      （private 标志会禁用 DHT/PEX，必须用带 passkey 的 .torrent）或者真没人做种
//!   B. DHT 里有 peer 但我们连不上  → 是我们的连接层问题
//!
//! 用法（哈希从环境变量传，不写死在代码里）：
//!   MYDL_PROBE_HASH=<40位十六进制> cargo test --test dht_probe -- --ignored --nocapture

use std::time::Duration;

use futures::StreamExt;
use std::str::FromStr;

use librqbit::dht::{DhtConfig, DhtState, Id20};

/// Debian 官方镜像，DHT 里必然找得到，用作对照基准。
const KNOWN_GOOD: &str = "481b6e3617be4c88f96cb25e47c9d8272130071e";

const LOOKUP_SECS: u64 = 60;

async fn probe(dht: &std::sync::Arc<DhtState>, label: &str, hash_hex: &str) -> usize {
    let hash = match Id20::from_str(hash_hex) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("  [{label}] 哈希格式不对：{e}");
            return 0;
        }
    };

    let mut stream = dht.get_peers(hash, None);
    let mut peers = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(LOOKUP_SECS);

    loop {
        tokio::select! {
            item = stream.next() => match item {
                Some(addr) => {
                    if peers.len() < 5 {
                        eprintln!("  [{label}] 找到 peer {addr}");
                    }
                    peers.push(addr);
                }
                None => break,
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    eprintln!("  [{label}] {LOOKUP_SECS}s 内共找到 {} 个 peer", peers.len());
    peers.len()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "诊断用，需要外网"]
async fn dht_knows_about_hash() {
    let dht = DhtState::with_config(DhtConfig::default())
        .await
        .expect("启动 DHT 失败");

    // 等路由表填起来，否则查询会因为没节点而立刻空转。
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if dht.stats().routing_table_size > 50 {
            break;
        }
    }
    eprintln!("DHT 路由表节点数：{}", dht.stats().routing_table_size);
    eprintln!();

    let good = probe(&dht, "对照-Debian", KNOWN_GOOD).await;
    eprintln!();

    match std::env::var("MYDL_PROBE_HASH") {
        Ok(h) => {
            let target = probe(&dht, "待查", h.trim()).await;
            eprintln!();
            eprintln!("结论：对照 {good} 个 peer，待查 {target} 个 peer");
            if good > 0 && target == 0 {
                eprintln!("      DHT 正常工作，但这个 swarm 在 DHT 里找不到。");
            }
        }
        Err(_) => eprintln!("（没设 MYDL_PROBE_HASH，只跑了对照）"),
    }

    assert!(good > 0, "对照组都查不到 peer，说明 DHT 本身有问题");
}
