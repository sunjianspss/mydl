//! Engine 的集成测试。
//!
//! 子目录相关的用例本地造种子，不联网，跟着 `cargo test` 正常跑。
//! 真连网那个被 `#[ignore]` 跳过（要外网、会下几 MB、依赖 Ubuntu 官方种子
//! 仍在做种），手动跑：
//!
//! ```
//! cargo test --test engine_smoke -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mydl_lib::engine::Engine;

/// Ubuntu 官方镜像种子，合法且长期有大量做种者。单文件。
const TORRENT_URL: &str =
    "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso.torrent";

/// 子目录行为用本地现造的种子来验证：不联网、不下数据，结果完全确定。
const LOCAL_TORRENT_NAME: &str = "我的多文件种子";

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

// ---------------------------------------------------------------------------
// 子目录行为。用本地现造的种子测，不联网、不下数据，所以不加 #[ignore]。
// ---------------------------------------------------------------------------

/// 在 `dir` 下造一个种子并返回 .torrent 文件路径。
async fn make_local_torrent(dir: &Path, content: &Path, name: &str) -> PathBuf {
    let created = librqbit::create_torrent(
        content,
        librqbit::CreateTorrentOptions {
            name: Some(name),
            piece_length: Some(16 * 1024),
        },
    )
    .await
    .expect("生成种子失败");

    let path = dir.join(format!("{name}.torrent"));
    std::fs::write(&path, created.as_bytes().expect("序列化种子失败")).unwrap();
    path
}

/// 等待路径出现，超时就带上目录实际内容报错。
async fn wait_for(path: &Path, parent: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "30s 内没出现 {path:?}，{parent:?} 实际内容：{:?}",
            list_dir(parent),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != ".DS_Store")
                .collect()
        })
        .unwrap_or_default()
}

/// 回归测试：指定自定义下载目录时，多文件种子必须收进以种子名命名的子目录。
///
/// librqbit 只在使用会话默认目录时才自动建子目录 —— session.rs 里
/// `(Some(o), None) => PathBuf::from(o)` 会把自定义目录原样拿去用，
/// 于是几十个文件直接倒进目标目录。Engine::add 现在自己补上了这一层。
#[tokio::test(flavor = "multi_thread")]
async fn multifile_torrent_gets_its_own_subfolder() {
    let tmp = std::env::temp_dir().join(format!("mydl-sub-multi-{}", std::process::id()));
    let content = tmp.join("content");
    let custom_dir = tmp.join("我选的目录");
    std::fs::create_dir_all(content.join("nested")).unwrap();
    std::fs::create_dir_all(&custom_dir).unwrap();
    std::fs::write(content.join("a.txt"), b"aaaa").unwrap();
    std::fs::write(content.join("nested/b.txt"), b"bbbb").unwrap();

    let torrent = make_local_torrent(&tmp, &content, LOCAL_TORRENT_NAME).await;

    // 会话默认目录故意设成别处，确保被测的是自定义目录这条路径。
    let engine = Engine::new(tmp.join("default-downloads"), Some(tmp.join("state")))
        .await
        .expect("创建 Engine 失败");

    engine
        .add(&torrent.to_string_lossy(), Some(custom_dir.to_string_lossy().into_owned()))
        .await
        .expect("添加种子失败");

    wait_for(&custom_dir.join(LOCAL_TORRENT_NAME), &custom_dir).await;

    // 关键断言：自定义目录下只能有那一个子目录，不能有散落文件。
    let entries = list_dir(&custom_dir);
    let outcome = if entries == vec![LOCAL_TORRENT_NAME.to_string()] {
        Ok(())
    } else {
        Err(format!("自定义目录下混进了散落内容：{entries:?}"))
    };

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    outcome.unwrap_or_else(|e| panic!("{e}"));
}

/// 反过来：单文件种子不该多套一层目录。
#[tokio::test(flavor = "multi_thread")]
async fn single_file_torrent_has_no_subfolder() {
    let tmp = std::env::temp_dir().join(format!("mydl-sub-single-{}", std::process::id()));
    let custom_dir = tmp.join("我选的目录");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(&custom_dir).unwrap();
    let content = tmp.join("solo.bin");
    std::fs::write(&content, b"0123456789").unwrap();

    let torrent = make_local_torrent(&tmp, &content, "solo.bin").await;

    let engine = Engine::new(tmp.join("default-downloads"), Some(tmp.join("state")))
        .await
        .expect("创建 Engine 失败");

    engine
        .add(&torrent.to_string_lossy(), Some(custom_dir.to_string_lossy().into_owned()))
        .await
        .expect("添加种子失败");

    wait_for(&custom_dir.join("solo.bin"), &custom_dir).await;

    let entries = list_dir(&custom_dir);
    let outcome = if entries == vec!["solo.bin".to_string()] {
        Ok(())
    } else {
        Err(format!("单文件种子不该建子目录，实际：{entries:?}"))
    };

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    outcome.unwrap_or_else(|e| panic!("{e}"));
}

/// 回归测试：加一条永远解析不出来的磁力链，必须超时报错，不能无限挂住。
///
/// librqbit 解析磁力链元信息时没有超时（session.rs 的
/// read_metainfo_from_peer_receiver），命令永不返回，界面就跟着卡死。
/// Engine::add 现在自己套了一层超时。
///
/// 这里把超时行为本身跑一遍：用一个随机 info-hash，DHT 里必然找不到源。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，且必须等满 Engine 的添加超时"]
async fn dead_magnet_times_out_instead_of_hanging() {
    let tmp = std::env::temp_dir().join(format!("mydl-deadmagnet-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let engine = Engine::new(tmp.join("downloads"), Some(tmp.join("state")))
        .await
        .expect("创建 Engine 失败");

    // 随机 40 位十六进制 info-hash：不可能对应任何真实种子。
    let dead = format!("magnet:?xt=urn:btih:{:040x}", rand_hash());

    let started = Instant::now();
    let err = engine
        .add(&dead, None)
        .await
        .expect_err("死磁力链不该添加成功");
    let elapsed = started.elapsed();

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    let msg = format!("{err:#}");
    assert!(msg.contains("超时"), "错误信息该说明是超时，实际：{msg}");
    assert!(
        msg.contains("没有 tracker") || msg.contains("不带 tracker"),
        "裸磁力链该提示缺 tracker，实际：{msg}"
    );
    // 必须真的在超时附近返回，而不是立刻失败（那说明走了别的错误路径）。
    assert!(
        elapsed >= Duration::from_secs(100),
        "返回太快（{elapsed:?}），可能没走到超时逻辑"
    );
    eprintln!("OK: {elapsed:?} 后超时返回 — {msg}");
}

/// 造一个随机 info-hash，避免误中真实种子。
fn rand_hash() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    (n.as_nanos()) ^ (std::process::id() as u128) << 64
}
