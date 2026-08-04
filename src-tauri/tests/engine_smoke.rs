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

    let engine = Engine::new(downloads, Some(state.clone()), state, Default::default())
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
            trackers: Vec::new(),
            piece_length: Some(16 * 1024),
        },
        // v9 起要显式传 spawner，用来把哈希计算放到阻塞线程池。
        &librqbit::spawn_utils::BlockingSpawner::new(1),
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
    let engine = Engine::new(
        tmp.join("default-downloads"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
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

/// 回归测试：自定义输出目录必须活过重启。
///
/// librqbit 把每个任务的输出目录藏在 `pub(crate)` 字段里读不到，Engine 自己
/// 记了一份 —— 以前只记在内存里，重启后「在访达中显示」就退回默认下载目录，
/// 用户会以为文件丢了。现在落盘到 output_folders.json，按 info-hash 索引
/// （TorrentId 是重启后重新分配的，不能拿来当键）。
#[tokio::test(flavor = "multi_thread")]
async fn output_folder_survives_restart() {
    let tmp = std::env::temp_dir().join(format!("mydl-folder-persist-{}", std::process::id()));
    let content = tmp.join("content");
    let custom_dir = tmp.join("我选的目录");
    let state = tmp.join("state");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::create_dir_all(&custom_dir).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(content.join("a.bin"), vec![7u8; 20 * 1024]).unwrap();
    std::fs::write(content.join("b.bin"), vec![8u8; 20 * 1024]).unwrap();

    let torrent = make_local_torrent(&tmp, &content, "重启测试").await;
    let subdir = custom_dir.join("重启测试");

    let engine = Engine::new(
        tmp.join("default-downloads"),
        Some(state.clone()),
        state.clone(),
        Default::default(),
    )
    .await
    .expect("创建 Engine 失败");

    let id = engine
        .add(&torrent.to_string_lossy(), Some(custom_dir.to_string_lossy().into_owned()))
        .await
        .expect("添加种子失败");
    wait_for(&subdir, &custom_dir).await;
    let before = engine.output_path(id).expect("读输出路径失败");
    engine.shutdown().await;

    // 重启：同一个 state / data 目录，librqbit 会把任务恢复回来。
    let engine = Engine::new(
        tmp.join("default-downloads"),
        Some(state.clone()),
        state,
        Default::default(),
    )
    .await
    .expect("重启后创建 Engine 失败");

    let restored = engine.list();
    let after = restored
        .first()
        .map(|t| engine.output_path(t.id).expect("重启后读输出路径失败"));

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(before, subdir, "添加时就该指向自定义目录下的子目录");
    assert_eq!(restored.len(), 1, "重启后任务该被恢复");
    assert_eq!(
        after.as_deref(),
        Some(subdir.as_path()),
        "重启后该还记得自定义目录，而不是退回默认下载目录"
    );
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

    let engine = Engine::new(
        tmp.join("default-downloads"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
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

    let engine = Engine::new(
        tmp.join("downloads"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
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

/// 主动取消必须立刻返回，而不是干等满 120 秒的超时。
///
/// 用死磁力链：它永远解析不出来，所以只要函数返回了，就一定是取消起了作用。
/// 不联网也能跑 —— probe 照样会一直挂着，取消路径不依赖网络。
#[tokio::test(flavor = "multi_thread")]
async fn preview_can_be_cancelled() {
    use std::sync::Arc;

    let tmp = std::env::temp_dir().join(format!("mydl-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let engine = Arc::new(
        Engine::new(
            tmp.join("downloads"),
            Some(tmp.join("state")),
            tmp.join("state"),
            Default::default(),
        )
        .await
        .expect("创建 Engine 失败"),
    );

    let dead = format!("magnet:?xt=urn:btih:{:040x}", rand_hash());
    let e = engine.clone();
    let task = tokio::spawn(async move { e.preview(&dead).await });

    // 等预览真的开始，让 select 完成第一次轮询。
    tokio::time::sleep(Duration::from_millis(800)).await;

    let started = Instant::now();
    engine.cancel_preview();

    let joined = tokio::time::timeout(Duration::from_secs(15), task).await;
    let elapsed = started.elapsed();

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    let outcome = joined
        .expect("取消后 15 秒内没返回，说明没被打断")
        .expect("预览任务 panic 了");

    match outcome {
        Ok(None) => {}
        Ok(Some(_)) => panic!("死磁力链不可能解析成功"),
        Err(e) => panic!("取消不该报错，实际：{e:#}"),
    }
    // 真正的超时是 120 秒，这里必须远快于它。
    assert!(elapsed < Duration::from_secs(15), "返回太慢：{elapsed:?}");
    eprintln!("OK: 取消后 {elapsed:?} 返回");
}

/// 造一个随机 info-hash，避免误中真实种子。
fn rand_hash() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    (n.as_nanos()) ^ (std::process::id() as u128) << 64
}

/// 只下选中的文件：取消勾选后，该文件必须从「已选」里消失，
/// 且任务的总大小要相应缩小（否则进度百分比会永远到不了 100%）。
#[tokio::test(flavor = "multi_thread")]
async fn deselected_files_are_excluded() {
    let tmp = std::env::temp_dir().join(format!("mydl-onlyfiles-{}", std::process::id()));
    let content = tmp.join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("keep.bin"), vec![1u8; 40 * 1024]).unwrap();
    std::fs::write(content.join("skip.bin"), vec![2u8; 80 * 1024]).unwrap();

    let torrent = make_local_torrent(&tmp, &content, "选择测试").await;
    let engine = Engine::new(
        tmp.join("downloads"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
        .await
        .expect("创建 Engine 失败");

    let id = engine
        .add(&torrent.to_string_lossy(), None)
        .await
        .expect("添加种子失败");

    // 等初始化结束 —— 初始化中 librqbit 不允许改选择。
    let started = Instant::now();
    while engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.state == "initializing")
        .unwrap_or(true)
    {
        assert!(started.elapsed() < Duration::from_secs(30), "初始化一直没结束");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let before = engine.files(id).expect("读文件列表失败");
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|f| f.selected), "默认应该全选");
    let total_before = engine.list().into_iter().find(|t| t.id == id).unwrap().total_bytes;

    // 只保留 keep.bin。
    let keep = before.iter().find(|f| f.name.contains("keep")).unwrap().index;
    engine.set_only_files(id, vec![keep]).await.expect("修改选择失败");

    let after = engine.files(id).expect("读文件列表失败");
    let total_after = engine.list().into_iter().find(|t| t.id == id).unwrap().total_bytes;

    // 一个都不选必须被拒绝，否则任务会永远卡在 0%。
    let empty = engine.set_only_files(id, vec![]).await;

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    for f in &after {
        let want = f.name.contains("keep");
        assert_eq!(f.selected, want, "{} 的选中状态不对", f.name);
    }
    assert!(
        total_after < total_before,
        "取消勾选后总大小该变小：{total_before} -> {total_after}"
    );
    assert!(empty.is_err(), "空选择必须被拒绝");
    eprintln!("OK: 总大小 {total_before} -> {total_after}");
}

/// 预览 → 勾选 → 确认：只有勾选的文件会落到磁盘，且预览缓存用完即失效。
#[tokio::test(flavor = "multi_thread")]
async fn preview_then_add_only_selected() {
    let tmp = std::env::temp_dir().join(format!("mydl-preview-{}", std::process::id()));
    let content = tmp.join("content");
    let custom_dir = tmp.join("我选的目录");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::create_dir_all(&custom_dir).unwrap();
    std::fs::write(content.join("keep.bin"), vec![1u8; 40 * 1024]).unwrap();
    std::fs::write(content.join("skip.bin"), vec![2u8; 80 * 1024]).unwrap();

    let torrent = make_local_torrent(&tmp, &content, "预览测试").await;
    let engine = Engine::new(
        tmp.join("default-downloads"),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
        .await
        .expect("创建 Engine 失败");

    // 预览不该把任务加进列表。
    let preview = engine
        .preview(&torrent.to_string_lossy())
        .await
        .expect("预览失败")
        .expect("本地种子不该返回「已取消」");
    assert_eq!(preview.files.len(), 2, "预览该列出两个文件");
    assert_eq!(preview.name, "预览测试");
    assert!(!preview.already_added);
    assert!(engine.list().is_empty(), "预览阶段不该有任务被加入");

    let keep = preview
        .files
        .iter()
        .find(|f| f.name.contains("keep"))
        .unwrap()
        .index;

    let id = engine
        .add_previewed(
            &preview.token,
            vec![keep],
            Some(custom_dir.to_string_lossy().into_owned()),
        )
        .await
        .expect("确认添加失败");

    // 多文件种子仍然要收进子目录。
    let subdir = custom_dir.join("预览测试");
    wait_for(&subdir, &custom_dir).await;

    let files = engine.files(id).expect("读文件列表失败");
    let selected: Vec<_> = files.iter().filter(|f| f.selected).collect();

    // 同一个 token 不能重复使用。
    let reuse = engine
        .add_previewed(&preview.token, vec![keep], None)
        .await;

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);

    assert_eq!(selected.len(), 1, "应该只选中一个文件");
    assert!(selected[0].name.contains("keep"), "选中的该是 keep.bin");
    assert!(reuse.is_err(), "预览 token 用完就该失效");
}

/// 回归测试：「暂停做种」只停做种的任务，还在下的必须原样跑着。
///
/// 这个区分是有实际后果的：上传带宽是全局共享的一份预算，做种会把它吃光，
/// 下载中的任务就没有可回报给对方的上行，容易被 choke 到零速。要是这里
/// 退化成「全部暂停」，用户想腾出上行反而把下载也停了。
///
/// 不联网：做种那个任务的数据预先放在输出目录里，librqbit 校验后直接判完成；
/// 下载中那个的数据不存在，没有 tracker 也连不上 DHT 里的谁，会一直是 0%。
#[tokio::test(flavor = "multi_thread")]
async fn pause_seeding_leaves_downloads_running() {
    let tmp = std::env::temp_dir().join(format!("mydl-pauseseed-{}", std::process::id()));
    let downloads = tmp.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();

    // 已完成的那个：先把内容写进下载目录，再按它造种子。
    let done_data = downloads.join("已下完.bin");
    std::fs::write(&done_data, vec![7u8; 64 * 1024]).unwrap();
    let done_torrent = make_local_torrent(&tmp, &done_data, "已下完.bin").await;

    // 还在下的那个：内容造在别处，下载目录里没有，永远下不动。
    let pending_src = tmp.join("别处.bin");
    std::fs::write(&pending_src, vec![9u8; 64 * 1024]).unwrap();
    let pending_torrent = make_local_torrent(&tmp, &pending_src, "还在下.bin").await;
    std::fs::remove_file(&pending_src).unwrap();

    let engine = Engine::new(
        downloads.clone(),
        Some(tmp.join("state")),
        tmp.join("state"),
        Default::default(),
    )
    .await
    .expect("创建 Engine 失败");

    let done = engine
        .add(&done_torrent.to_string_lossy(), None)
        .await
        .expect("添加做种任务失败");
    let pending = engine
        .add(&pending_torrent.to_string_lossy(), None)
        .await
        .expect("添加下载任务失败");

    // 等校验跑完，两个都离开 initializing。
    let started = Instant::now();
    let ready = loop {
        let list = engine.list();
        let states: Vec<_> = list.iter().map(|t| (t.id, t.state.clone(), t.finished)).collect();
        if states.iter().all(|(_, s, _)| s != "initializing") {
            break Ok(states);
        }
        if started.elapsed() > Duration::from_secs(30) {
            break Err(format!("30s 内没初始化完：{states:?}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let outcome = match ready {
        Err(e) => Err(e),
        Ok(states) => {
            let seeding_before = states.iter().any(|(id, s, f)| *id == done && s == "live" && *f);
            if !seeding_before {
                Err(format!("预置数据的任务没被判成做种中：{states:?}"))
            } else {
                let n = engine.pause_seeding().await;
                let after = engine.list();
                let done_view = after.iter().find(|t| t.id == done).unwrap();
                let pending_view = after.iter().find(|t| t.id == pending).unwrap();
                if n != 1 {
                    // 退化成「全部暂停」时就是这条 —— 带上进度，一眼能看出
                    // 是判定写错了还是那个「还在下」的种子意外下完了。
                    Err(format!(
                        "该只暂停 1 个，实际 {n} 个。做种 {}/{}，下载中 {}/{}",
                        done_view.progress_bytes, done_view.total_bytes,
                        pending_view.progress_bytes, pending_view.total_bytes,
                    ))
                } else if done_view.state != "paused" {
                    Err(format!("做种任务没被暂停：{}", done_view.state))
                } else if pending_view.state == "paused" {
                    Err("下载中的任务被误停了".to_string())
                } else {
                    Ok(())
                }
            }
        }
    };

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    outcome.unwrap_or_else(|e| panic!("{e}"));
}
