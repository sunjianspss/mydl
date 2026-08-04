//! 拿真实 tracker 验一遍 BEP15 scrape。
//!
//! 单元测试只覆盖了报文的编解码，覆盖不到「真 tracker 认不认这套字节」——
//! 而这正是最容易写错的地方（connection id 的生命周期、字段顺序、一个包
//! 塞多个 hash 时的对齐）。所以单独留一个打真网的用例。
//!
//! 默认跳过，手动跑：
//!
//! ```
//! cargo test --test scrape_live -- --ignored --nocapture
//! ```

use mydl_lib::engine::Engine;
use mydl_lib::health::{self, parse_info_hash, scrape_one, HealthStore, Status};
use mydl_lib::settings::{SettingsStore, PUBLIC_TRACKERS};

/// Ubuntu 24.04.4 live-server amd64。官方长期做种，做种数必然远大于 0，
/// 拿它当「协议跑通了」的判据比拿冷门资源可靠。
const UBUNTU: &str = "62a4d9e139f3315f8716bcccca0cc984a9809da1";

/// 一个几乎不可能存在的 info-hash。tracker 对它要么不返回、要么返回全 0，
/// 但**不能**返回跟 Ubuntu 一样的数字 —— 那说明我们把应答对错位了。
const NONSENSE: &str = "00000000000000000000000000000000deadbeef";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会连公共 tracker"]
async fn scrapes_real_trackers() {
    let ubuntu = parse_info_hash(UBUNTU).expect("infohash 解析失败");
    let nonsense = parse_info_hash(NONSENSE).expect("infohash 解析失败");

    let mut reachable = 0;
    let mut saw_seeders = false;

    for tracker in PUBLIC_TRACKERS {
        // 故意一个包里塞两个：单个 hash 的话对齐错了也看不出来。
        match scrape_one(tracker, &[ubuntu, nonsense]).await {
            Ok(entries) => {
                reachable += 1;
                eprintln!("[{tracker}] {entries:?}");
                assert!(!entries.is_empty(), "{tracker} 返回了空列表");

                let u = entries[0];
                if u.seeders > 0 {
                    saw_seeders = true;
                }
                // 对齐错位的典型症状：第二条（不存在的种子）也有人做种。
                if entries.len() > 1 {
                    assert_eq!(
                        entries[1].seeders, 0,
                        "{tracker} 说一个不存在的种子有人做种，多半是应答对错位了"
                    );
                }
            }
            Err(e) => eprintln!("[{tracker}] 连不上：{e:#}"),
        }
    }

    assert!(
        reachable > 0,
        "5 个公共 tracker 一个都没连上 —— 可能是网络问题，不一定是代码问题"
    );
    assert!(
        saw_seeders,
        "连上了 {reachable} 个 tracker 但 Ubuntu 官方种子做种数是 0，\
         这不可能，说明解析有问题"
    );
    eprintln!("OK：{reachable}/{} 个 tracker 可达", PUBLIC_TRACKERS.len());
}

/// 端到端：引擎里的任务 -> scrape -> 落盘 -> 读出结论。
///
/// 单测覆盖了报文编解码和判定逻辑，但没覆盖它们之间的接缝：info-hash 从
/// TorrentView 里取出来是大写还是小写、chunk 之后应答跟 hash 还对不对得上、
/// 存进去再读出来键能不能命中。这些都是「每一块都对、连起来不对」的经典位置。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会连公共 tracker 并短暂添加一个真实种子"]
async fn samples_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("mydl-health-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let engine = Engine::new(
        tmp.join("dl"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
    .await
    .expect("创建 Engine 失败");

    // 用 Ubuntu 官方种子：做种数必然远大于 0，结论应该是 Ok 而不是 Dead。
    let id = engine
        .add(
            "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso.torrent",
            None,
        )
        .await
        .expect("添加种子失败");
    // 立刻暂停，这个测试只关心健康度采样，不需要真下数据。
    let _ = engine.pause(id).await;

    let info_hash = engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .expect("任务不在列表里")
        .info_hash;

    let settings = SettingsStore::load(tmp.join("settings.json"));
    let store = HealthStore::load(tmp.join("swarm_health.json"));

    let n = health::sample_once(&engine, &settings, &store).await;

    let history = store.history(&info_hash);
    let v = health::verdict(&history);
    eprintln!("info_hash={info_hash} 覆盖 {n} 个种子，结论：{v:?}");

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(n, 1, "引擎里就一个任务，该采到一个");
    assert_eq!(history.len(), 1, "该记下一条样本");
    assert!(
        history[0].trackers_ok > 0,
        "一个 tracker 都没应答 —— 可能是网络问题，不一定是代码问题"
    );
    assert!(
        history[0].seeders > 0,
        "Ubuntu 官方种子做种数不该是 0，说明 info-hash 或应答对不上：{:?}",
        history[0]
    );
    assert_ne!(v.status, Status::Dead);
    // 只有一个样本时还判不出趋势，别硬报。
    assert_eq!(v.samples, 1);
}
