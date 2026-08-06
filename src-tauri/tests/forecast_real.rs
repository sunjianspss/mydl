//! 拿真实采样历史跑一遍预测，看它对各种数据状态说什么。
//!
//! ```
//! cargo test --test forecast_real -- --ignored --nocapture
//! ```
use mydl_lib::{forecast, health};

#[test]
#[ignore = "需要本地有 swarm_health.json"]
fn runs_on_real_history() {
    let path = format!(
        "{}/Library/Application Support/com.sun.mydl/swarm_health.json",
        std::env::var("HOME").unwrap()
    );
    let store = health::HealthStore::load(path.into());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // 从 session.json 拿 info-hash 列表
    let sess = format!(
        "{}/Library/Application Support/com.rqbit.session/session.json",
        std::env::var("HOME").unwrap()
    );
    let Ok(raw) = std::fs::read_to_string(&sess) else {
        eprintln!("读不到 session.json，跳过");
        return;
    };
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let torrents = v["torrents"].as_object().cloned().unwrap_or_default();

    let mut checked = 0;
    for (_, t) in torrents {
        let Some(hash) = t["info_hash"].as_str() else { continue };
        let history = store.history(hash);
        if history.is_empty() {
            continue;
        }
        let with_progress = history.iter().filter(|s| s.progress.is_some()).count();
        // 还剩多少不知道（session.json 里没有），拿一个典型值看输出形态
        let f = forecast::forecast(&history, 10_000_000_000, now);
        eprintln!(
            "\n{}  {} 个样本（{} 个带进度）",
            &hash[..10],
            history.len(),
            with_progress
        );
        eprintln!("  {}", f.summary);
        if let (Some(lo), Some(hi), Some(med)) = (f.seeders_min, f.seeders_max, f.seeders_median) {
            eprintln!("  做种数 {lo}~{hi}，中位 {med}");
        }
        checked += 1;
    }
    assert!(checked > 0, "一个种子的历史都没读到");
}
