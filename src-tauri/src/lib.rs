pub mod ai;
pub mod automation;
pub mod congestion;
pub mod diagnose;
pub mod engine;
pub mod forecast;
pub mod health;
pub mod keep_awake;
pub mod media;
pub mod netif;
pub mod platform;
pub mod push;
pub mod rarity;
pub mod ratio;
pub mod release;
pub mod rss;
pub mod search;
pub mod secrets;
pub mod settings;
pub mod verify;
pub mod stats;
pub mod stream_server;

use std::path::PathBuf;
use std::sync::Arc;

use engine::{Engine, FileView, SessionSetup, TorrentId, TorrentPreview, TorrentView};
use settings::{Settings, SettingsStore};
use stream_server::StreamServer;
use tauri::{Manager, State};

/// 命令统一返回 String 错误，前端直接展示。anyhow 的 `{:#}` 会带上 context 链。
fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// 解析种子但不加入会话，让用户先看看里面有什么。
///
/// 返回 null 表示用户点了取消 —— 不是错误，界面不该弹红条。
#[tauri::command]
async fn preview_torrent(
    engine: State<'_, Arc<Engine>>,
    uri: String,
) -> Result<Option<TorrentPreview>, String> {
    engine.preview(&uri).await.map_err(err)
}

/// 打断正在进行的预览。磁力链解析最长要等 2 分钟，不该只能干等。
#[tauri::command]
fn cancel_preview(engine: State<'_, Arc<Engine>>) {
    engine.cancel_preview();
}

/// 确认添加预览过的种子，只下勾选的文件。
#[tauri::command]
async fn add_previewed(
    engine: State<'_, Arc<Engine>>,
    token: String,
    files: Vec<usize>,
    output_folder: Option<String>,
) -> Result<TorrentId, String> {
    engine
        .add_previewed(&token, files, output_folder)
        .await
        .map_err(err)
}

#[tauri::command]
fn list_torrents(engine: State<'_, Arc<Engine>>) -> Vec<TorrentView> {
    engine.list()
}

/// 按关键词搜索种子。
///
/// 结果里的链接**全部来自索引器**，我们不生成任何 info-hash。开了 AI 排序的话
/// 模型也只能对这个列表重排，见 `ai.rs`。搜到的东西加进任务列表前仍然要走
/// `preview_torrent` 真实探测一次，编造的 hash 在那一步必然暴露。
#[tauri::command]
async fn search_torrents(
    store: State<'_, Arc<SettingsStore>>,
    query: String,
) -> Result<Vec<search::SearchResult>, String> {
    let s = store.get();
    // key 在系统钥匙串里，不在 settings.json。
    let cfg = secrets::ai_key().map(|api_key| ai::AiConfig {
        base_url: s.ai_base_url.clone(),
        api_key,
        model: s.ai_model.clone(),
    });

    let configured = s.search_url.as_deref().is_some_and(|u| !u.trim().is_empty());

    // 有索引器就用索引器，它给的 info-hash 可信。没有才退而求其次让模型上网找 ——
    // 那条路返回的每条都带 unverified 标记，界面会标出来。
    if !configured {
        let Some(cfg) = cfg else {
            return Err("还没配置索引器地址，也没填 API key。至少要有一个".into());
        };
        return ai::search_web(&cfg, &query).await.map_err(err);
    }

    let results = search::search(s.search_url.as_deref().unwrap_or_default(), &query)
        .await
        .map_err(err)?;

    match (s.ai_rank, cfg) {
        (true, Some(cfg)) => Ok(ai::rank(&cfg, &query, results).await),
        _ => Ok(results),
    }
}

/// 把 API key 写进系统钥匙串。传空串等于删除。
///
/// 只写不读：界面上没有「显示 key」的入口，存进去就拿不回来了 —— 想换就重填。
#[tauri::command]
fn set_ai_key(key: String) -> Result<(), String> {
    secrets::set_ai_key(key.trim()).map_err(err)
}

/// 界面用来决定输入框显示「已保存」还是空。
#[tauri::command]
fn has_ai_key() -> bool {
    secrets::has_ai_key()
}

/// 底部状态栏用的会话信息（DHT 节点数、监听端口）。
#[tauri::command]
fn session_status(engine: State<'_, Arc<Engine>>) -> engine::SessionStatus {
    engine.session_status()
}

#[tauri::command]
async fn pause_torrent(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<(), String> {
    engine.pause(id).await.map_err(err)
}

#[tauri::command]
async fn resume_torrent(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<(), String> {
    engine.resume(id).await.map_err(err)
}

/// 暂停所有正在跑的任务，返回操作了几个。
#[tauri::command]
async fn pause_all(engine: State<'_, Arc<Engine>>) -> Result<usize, String> {
    Ok(engine.pause_all().await)
}

/// 每个做种任务的稀缺度。给界面标「全网仅存 N 份」用。
#[tauri::command]
fn seeding_rarity(
    engine: State<'_, Arc<Engine>>,
    health: State<'_, Arc<health::HealthStore>>,
) -> Vec<(TorrentId, rarity::Verdict)> {
    engine
        .list()
        .into_iter()
        .filter(|t| t.finished)
        .map(|t| (t.id, rarity::judge(&health.history(&t.info_hash))))
        .collect()
}

/// 只暂停「不稀缺」的做种任务，把全局上传预算让给稀有的那些。
///
/// 为什么是暂停而不是分配带宽：librqbit v9 的 per-torrent `ratelimits` 在
/// `ManagedTorrentOptions` 里，整个结构是 `pub(crate)`，外部够不着 ——
/// 只有全局那一个。粗，但这是现有 API 唯一能做到的分配。
///
/// **判不出稀缺度的一律不动。** 数据不够就保守，不能因为「还没采够」
/// 就把人家的做种停了。
#[tauri::command]
async fn pause_common_seeding(
    engine: State<'_, Arc<Engine>>,
    health: State<'_, Arc<health::HealthStore>>,
) -> Result<usize, String> {
    let targets: Vec<TorrentId> = engine
        .list()
        .into_iter()
        .filter(|t| t.finished && t.state == "live")
        .filter(|t| rarity::judge(&health.history(&t.info_hash)).rarity == rarity::Rarity::Common)
        .map(|t| t.id)
        .collect();

    let mut n = 0;
    for id in targets {
        match engine.pause(id).await {
            Ok(()) => n += 1,
            Err(e) => tracing::warn!(id, "暂停失败：{e:#}"),
        }
    }
    tracing::info!(暂停 = n, "只留稀有的做种");
    Ok(n)
}

/// 只暂停做种中的任务，返回操作了几个。下载中的不动。
#[tauri::command]
async fn pause_seeding(engine: State<'_, Arc<Engine>>) -> Result<usize, String> {
    Ok(engine.pause_seeding().await)
}

/// 某个任务的 swarm 健康度结论。纯读本地历史，不发包，所以可以随便调。
#[tauri::command]
fn torrent_health(
    health: State<'_, Arc<health::HealthStore>>,
    info_hash: String,
) -> health::Verdict {
    health::verdict(&health.history(&info_hash))
}

/// 按实测吞吐量算「还要多久」。见 `forecast.rs`。
///
/// 纯读本地采样历史，不发包。**不出概率** —— 见那个模块的文档，
/// 做种数的噪声太大，报百分比是编造精度。
#[tauri::command]
fn torrent_forecast(
    engine: State<'_, Arc<Engine>>,
    health: State<'_, Arc<health::HealthStore>>,
    id: TorrentId,
) -> Result<forecast::Forecast, String> {
    let t = engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("找不到任务 {id}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(forecast::forecast(
        &health.history(&t.info_hash),
        t.total_bytes.saturating_sub(t.progress_bytes),
        now,
    ))
}

/// 立刻采一轮，不等定时器。返回覆盖了几个种子。
#[tauri::command]
async fn check_health_now(
    engine: State<'_, Arc<Engine>>,
    store: State<'_, Arc<SettingsStore>>,
    health: State<'_, Arc<health::HealthStore>>,
) -> Result<usize, String> {
    Ok(health::sample_once(&engine, &store, &health, None).await)
}

/// 给一个已有任务找替代源的结果。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FoundSources {
    /// 实际拿去搜的词。界面要显示出来并允许改 —— 中文压制名解析不可能全对，
    /// 让用户一眼看出「它搜错了」比默默返回坏结果强。
    query: String,
    /// 从任务名解析出的完整标题，用来打分的那个。
    full_title: String,
    candidates: Vec<release::Candidate>,
}

/// 给任务 `id` 找别的源。
///
/// `query` 传 null 就用从任务名解析出来的词；传了就用传的，这样界面上改词
/// 能立刻重搜。
///
/// **不做任何自动切换**，只返回候选。换不换、什么时候换是有代价的决定
/// （换源意味着已下的字节全部作废），必须由人来做。
///
/// 这里刻意不走 AI 排序：`relevance` 是确定性的，而模型这条路要多等最多
/// 60 秒、实测还超时过。找替代源是个交互动作，不能让人干等。
#[tauri::command]
async fn find_sources(
    engine: State<'_, Arc<Engine>>,
    store: State<'_, Arc<SettingsStore>>,
    id: TorrentId,
    query: Option<String>,
) -> Result<FoundSources, String> {
    let torrent = engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("找不到任务 {id}"))?;

    let parsed = release::parse(&torrent.name);
    let query = query
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .unwrap_or(parsed.search_query);

    let s = store.get();
    let base = s.search_url.clone().unwrap_or_default();
    if base.trim().is_empty() {
        return Err("还没配置索引器地址。在设置里填 Prowlarr 或 Jackett 的 Torznab 地址".into());
    }

    let results = search::search(&base, &query).await.map_err(err)?;
    let mut candidates = release::rank(results, &parsed.full_title, &torrent.info_hash);

    // 索引器给的做种数不能信：实测某些中文索引器给**所有**条目都填
    // `seeders=1, size=0.01GB` 这种占位值，照着它挑源等于抛硬币。
    // 所以拿候选的 info-hash 去真 tracker 实查一遍。
    // 一个 UDP 包能带 74 个 hash，二十来个候选就是一个包，很便宜。
    let hashes: Vec<(String, [u8; 20])> = candidates
        .iter()
        .filter_map(|c| c.magnet.as_deref().and_then(release::info_hash_of))
        .filter_map(|h| health::parse_info_hash(&h).map(|raw| (h, raw)))
        .collect();

    if !hashes.is_empty() {
        let live = health::scrape_many(&hashes).await;
        for c in &mut candidates {
            let Some(h) = c.magnet.as_deref().and_then(release::info_hash_of) else {
                continue;
            };
            // trackers_ok 为 0 表示这轮一个 tracker 都没应答，那就是「没查到」，
            // 不能当成「0 个做种」——否则一次网络抖动会让所有候选都显示成死的。
            if let Some((s, l, ok)) = live.get(&h) {
                if *ok > 0 {
                    c.live_seeders = Some(*s);
                    c.live_leechers = Some(*l);
                }
            }
        }
        // 实查到的做种数才是有意义的排序依据，重排一次。相关度仍然优先。
        candidates.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.live_seeders.unwrap_or(0).cmp(&a.live_seeders.unwrap_or(0)))
                .then(b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
        });
    }

    tracing::info!(
        任务 = %torrent.name,
        查询 = %query,
        候选 = candidates.len(),
        实查到做种数 = candidates.iter().filter(|c| c.live_seeders.is_some()).count(),
        "找替代源"
    );

    Ok(FoundSources {
        query,
        full_title: parsed.full_title,
        candidates,
    })
}

/// 可以绑定的网卡列表。给设置里的下拉用。
#[tauri::command]
fn network_interfaces() -> Vec<netif::NetIf> {
    netif::list()
}

/// 诊断一个任务为什么不动。见 `diagnose.rs`。
///
/// 会真的发包（tracker announce + 对若干 peer 试握手 + 打一个对照 swarm），
/// 最长几十秒。所以只在用户点了才跑，不做后台巡检。
#[tauri::command]
async fn diagnose_torrent(
    engine: State<'_, Arc<Engine>>,
    id: TorrentId,
) -> Result<diagnose::Report, String> {
    let t = engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("找不到任务 {id}"))?;

    Ok(diagnose::run(
        &t.info_hash,
        t.state,
        t.error,
        t.finished,
        engine.session_status().bind_device,
    )
    .await)
}

/// 验一验这个任务是不是名字说的那个东西。见 `verify.rs`。
///
/// 只读文件头（必要时加文件尾）几 MB，不等整个下完。会把这些分片提到最高
/// 优先级，所以对正在下的任务来说等于插了个队。
#[tauri::command]
async fn verify_torrent(
    engine: State<'_, Arc<Engine>>,
    id: TorrentId,
) -> Result<verify::VerifyReport, String> {
    verify::verify_torrent(&engine, id).await.map_err(err)
}

/// 自适应上传限速的当前状态。None = 没开或还没起来。
#[tauri::command]
fn congestion_state(state: State<'_, congestion::Shared>) -> Option<congestion::State> {
    *state.lock().unwrap()
}

/// 每日下载/上传统计。纯读本地记录，不发包。
#[tauri::command]
fn download_stats(stats: State<'_, Arc<stats::StatsStore>>) -> stats::Report {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    stats.snapshot(now)
}

/// 继续所有暂停的任务。
///
/// 注意它会把「被并发上限自动暂停」的也一起放出来 —— 用户明确点了「全部继续」，
/// 那就该听他的；下一轮轮询会重新按上限收敛。
#[tauri::command]
async fn resume_all(engine: State<'_, Arc<Engine>>) -> Result<usize, String> {
    Ok(engine.resume_all().await)
}

#[tauri::command]
async fn delete_torrent(
    engine: State<'_, Arc<Engine>>,
    id: TorrentId,
    delete_files: bool,
) -> Result<(), String> {
    engine.delete(id, delete_files).await.map_err(err)
}

/// 当前实际生效的下载目录（会话默认值）。
#[tauri::command]
fn default_download_dir(engine: State<'_, Arc<Engine>>) -> String {
    engine.download_dir().to_string_lossy().into_owned()
}

#[tauri::command]
fn get_settings(store: State<'_, Arc<SettingsStore>>) -> Settings {
    store.get()
}

/// `dir` 传 null 表示恢复成系统默认下载文件夹。
///
/// 立即对之后添加的任务生效；下次启动时会直接作为会话默认目录。
#[tauri::command]
fn set_download_dir(store: State<'_, Arc<SettingsStore>>, dir: Option<String>) -> Result<(), String> {
    store.set_download_dir(dir).map_err(err)
}

/// 整份保存设置。下载目录不走这里 —— 它只在启动时读，见 `SettingsStore::update`。
///
/// 上传限速立刻生效（librqbit 的限速器是运行时可改的），不用重启。
#[tauri::command]
fn save_settings(
    store: State<'_, Arc<SettingsStore>>,
    engine: State<'_, Arc<Engine>>,
    settings: Settings,
) -> Result<(), String> {
    store.update(settings).map_err(err)?;
    let s = store.get();
    engine.set_upload_limit(s.upload_limit_bps());
    engine.set_download_limit(s.download_limit_bps());
    Ok(())
}

/// 返回磁盘路径，前端交给 opener 插件在访达里显示。
#[tauri::command]
fn reveal_path(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<String, String> {
    engine
        .output_path(id)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(err)
}

#[tauri::command]
fn list_files(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<Vec<FileView>, String> {
    engine.files(id).map_err(err)
}

/// 只下载 `files` 里的这些文件（按序号）。
#[tauri::command]
async fn set_only_files(
    engine: State<'_, Arc<Engine>>,
    id: TorrentId,
    files: Vec<usize>,
) -> Result<(), String> {
    engine.set_only_files(id, files).await.map_err(err)
}

/// 边下边播用的本地 URL。可以直接丢给播放器，也可以复制到别处用。
#[tauri::command]
fn stream_url(
    engine: State<'_, Arc<Engine>>,
    server: State<'_, StreamServer>,
    id: TorrentId,
    file_id: usize,
) -> Result<String, String> {
    let files = engine.files(id).map_err(err)?;
    let file = files
        .get(file_id)
        .ok_or_else(|| format!("任务 {id} 里没有第 {file_id} 个文件"))?;
    Ok(server.url_for(id, file_id, &file.name))
}

/// 立刻检查一遍所有启用的订阅，返回每条订阅的结果。
#[tauri::command]
async fn check_rss_now(
    engine: State<'_, Arc<Engine>>,
    store: State<'_, Arc<SettingsStore>>,
    seen: State<'_, Arc<rss::SeenStore>>,
) -> Result<Vec<rss::CheckReport>, String> {
    Ok(rss::check_all(&engine, &store, &seen).await)
}

/// 重启 App，让「改了要重启才生效」的那几项设置生效。
///
/// 那几项（公共 tracker、绑定网卡、代理、黑名单、peer 上限）都是**建会话时
/// 才读**的，运行中改不了。真要热生效得把整个 BT 会话拆了重建：监听端口是
/// 独占的、DHT 状态要重新持久化、所有任务得重新加一遍 —— 为几个开关冒这个
/// 险不划算。让用户少走一步「自己关掉再打开」，收益的大头就到手了。
///
/// **`restart()` 必须回到主线程上调用。** 它在非主线程上只是「请求」退出，
/// 然后 `loop { sleep(Duration::MAX) }` 把调用线程永久挂起，等事件循环把
/// `Exit` 送回来再真正重启（见 tauri 2.11 的 `app.rs`）。而 Tauri 命令跑在
/// tokio 工作线程上 —— 实测那个 `Exit` 等不到，界面就一直卡在「重启中…」。
/// 回到主线程走的是另一条分支：`cleanup_before_exit()` + 直接重启，不绕
/// 事件循环。
///
/// 也正因为不绕事件循环，**`RunEvent::Exit` 那个回调不会跑**，该刷的盘得在
/// 这里自己刷完。
#[tauri::command]
async fn restart_app(
    app: tauri::AppHandle,
    engine: State<'_, Arc<Engine>>,
    ratio: State<'_, Arc<ratio::RatioStore>>,
    stats: State<'_, Arc<stats::StatsStore>>,
) -> Result<(), String> {
    ratio.save();
    health::record_daily(&engine, &stats);
    engine.shutdown().await;
    tracing::info!("按用户要求重启");

    let handle = app.clone();
    app.run_on_main_thread(move || handle.restart())
        .map_err(|e| format!("回到主线程重启失败：{e}"))
}

/// 日志目录，给界面上的「日志」入口用 —— 打包版看不到 stdout。
#[tauri::command]
fn log_dir() -> String {
    platform::log_dir().to_string_lossy().into_owned()
}

/// 设置里的「试听」用。不然要等一个任务真的下完才知道声音是什么。
#[tauri::command]
fn play_done_sound() {
    platform::play_done_sound();
}

/// 界面上的一个播放器按钮。
///
/// `path` 有值表示这是用户在设置里手填的那条路径，启动时直接用它，不再去猜
/// 安装位置 —— 也因此不怕两个同名播放器分不清：分派看的是 path，不是 name。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PlayerEntry {
    name: String,
    path: Option<String>,
    /// 这个播放器认得的容器扩展名，`None` = 什么都能放。界面按它决定
    /// 给哪些文件出按钮，理由见 [`platform::player_containers`]。
    plays: Option<&'static [&'static str]>,
}

/// 按钮上显示什么：路径的文件名去掉扩展名。
/// `/Applications/IINA.app` → IINA，`/opt/homebrew/bin/mpv` → mpv。
fn player_label(path: &str) -> String {
    PathBuf::from(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// 已装的播放器，界面按这个渲染按钮。自动扫出来的 + 用户手填的那个。
#[tauri::command]
fn available_players(store: State<'_, Arc<SettingsStore>>) -> Vec<PlayerEntry> {
    let mut list: Vec<PlayerEntry> = platform::available_players()
        .into_iter()
        .map(|name| PlayerEntry {
            plays: platform::player_containers(&name),
            name,
            path: None,
        })
        .collect();

    let custom = store
        .get()
        .custom_player
        .filter(|p| !p.trim().is_empty());
    if let Some(path) = custom {
        let name = player_label(&path);
        // 同名的自动检测项让位：用户明确指了路径，那就是他要的那一个，
        // 留着两个一模一样的按钮只会让人不知道该点哪个。
        list.retain(|p| p.name != name);
        list.push(PlayerEntry {
            // 手填的路径同样过一遍容器表：填的要是 QuickTime，该限制还得限制。
            // 认不出的名字一律当全能 —— 我们不知道那个可执行文件能放什么，
            // 而用户是自己指的路径，猜错不如放行。
            plays: platform::player_containers(&name),
            name,
            path: Some(path),
        });
    }
    list
}

/// 用指定播放器打开流地址。
///
/// 给了 `path` 就直接用它，否则按名字去各平台的已知安装位置里找。
#[tauri::command]
fn open_in_player(url: String, app: String, path: Option<String>) -> Result<(), String> {
    match path.as_deref() {
        Some(p) => platform::open_path(&url, p).map_err(err),
        None => platform::open_in_player(&url, &app).map_err(err),
    }
}

fn init_app(app: &tauri::App) -> anyhow::Result<()> {
    // 必须在建会话之前：librqbit 一起来就会开一堆 socket。
    platform::raise_file_limit();

    let config_dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let store = Arc::new(SettingsStore::load(config_dir.join("settings.json")));

    // 用户选过目录就直接拿它当会话默认目录 —— 这样常规添加走的是
    // librqbit 自己那条路径，不需要额外探测种子。
    let download_dir = store
        .get()
        .download_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            app.path()
                .download_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
        });

    tracing::info!(
        "下载目录：{}（{}）",
        download_dir.display(),
        if store.get().download_dir.is_some() {
            "来自设置"
        } else {
            "系统默认"
        }
    );

    // Session 启动包含读取持久化状态和绑定监听端口，必须在窗口出现前完成，
    // 否则前端第一次 list_torrents 会拿不到 State。
    let s = store.get();
    let setup = SessionSetup {
        extra_trackers: if s.use_public_trackers {
            settings::PUBLIC_TRACKERS.iter().map(|t| t.to_string()).collect()
        } else {
            Vec::new()
        },
        proxy_url: s.proxy_url.clone().filter(|u| !u.trim().is_empty()),
        blocklist_url: s.blocklist_url.clone().filter(|u| !u.trim().is_empty()),
        peer_limit: s.peer_limit,
        bind_device: s.bind_device.clone().filter(|d| !d.trim().is_empty()),
    };
    tracing::info!(
        网卡 = setup.bind_device.as_deref().unwrap_or("跟随系统"),
        公共tracker = setup.extra_trackers.len(),
        代理 = setup.proxy_url.is_some(),
        黑名单 = setup.blocklist_url.is_some(),
        peer上限 = ?setup.peer_limit,
        "会话配置"
    );

    let (engine, server) = tauri::async_runtime::block_on(async {
        let engine = Arc::new(Engine::new(download_dir, None, config_dir.clone(), setup).await?);
        let server = StreamServer::start(engine.clone()).await?;
        Ok::<_, anyhow::Error>((engine, server))
    })?;

    // 限速只存在设置里，会话本身不持久化它，所以每次启动都要重新应用。
    engine.set_upload_limit(store.get().upload_limit_bps());
    engine.set_download_limit(store.get().download_limit_bps());

    let seen = Arc::new(rss::SeenStore::load(rss::seen_path(&config_dir)));
    let health = Arc::new(health::HealthStore::load(health::health_path(&config_dir)));
    let stats = Arc::new(stats::StatsStore::load(stats::stats_path(&config_dir)));
    let queue = Arc::new(automation::QueueStore::load(automation::queue_path(
        &config_dir,
    )));
    let ratio = Arc::new(ratio::RatioStore::load(ratio::ratio_path(&config_dir)));
    let congestion: congestion::Shared = Default::default();

    automation::spawn(
        app.handle().clone(),
        engine.clone(),
        store.clone(),
        queue,
        ratio.clone(),
    );
    rss::spawn(engine.clone(), store.clone(), seen.clone());
    keep_awake::spawn(engine.clone(), store.clone());
    health::spawn(engine.clone(), store.clone(), health.clone(), stats.clone());
    congestion::spawn(engine.clone(), store.clone(), congestion.clone());
    push::spawn(app.handle().clone(), engine.clone());

    app.manage(engine);
    app.manage(server);
    app.manage(store);
    app.manage(seen);
    app.manage(health);
    app.manage(stats);
    app.manage(ratio);
    app.manage(congestion);
    Ok(())
}

/// 日志同时写终端和文件。打包后 stdout 没人接，出了问题只能靠文件日志查。
///
/// 返回的 guard 必须活到进程结束，否则非阻塞写线程会被提前关掉、丢日志。
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_appender::rolling::{Builder, Rotation};
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let log_dir = platform::log_dir();

    // 日志坏了也不能影响 App 启动，所以每一步失败都只是退化成「只打终端」。
    let (file_layer, guard) = match std::fs::create_dir_all(&log_dir).ok().and_then(|_| {
        Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix("mydl")
            .filename_suffix("log")
            // 留一周，免得无限长大。
            .max_log_files(7)
            .build(&log_dir)
            .ok()
    }) {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(fmt::layer().with_ansi(false).with_writer(writer)), Some(guard))
        }
        None => {
            eprintln!("警告：无法写入日志目录 {}，只输出到终端", log_dir.display());
            (None, None)
        }
    };

    // 两层都关掉 ANSI。span 字段的格式化结果按 field-formatter 类型缓存在
    // span extensions 里，两层共用 DefaultFields 就会共用同一份缓存 ——
    // 只在文件层 with_ansi(false) 没用，终端层先写进去的带色版本会被直接复用。
    // 打包版没有终端，dev 模式 stdout 也基本都重定向到文件，颜色没有价值。
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,librqbit=info".into()))
        .with(fmt::layer().with_ansi(false))
        .with(file_layer)
        .init();

    if guard.is_some() {
        tracing::info!("日志目录：{}", log_dir.display());
    }
    guard
}

/// 启动失败时给出一句人话再退出，而不是让 Tauri panic 成 SIGABRT。
fn fatal(app: &tauri::App, message: &str) -> ! {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    tracing::error!("启动失败：{message}");
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Error)
        .title("mydl 无法启动")
        .blocking_show();

    std::process::exit(1);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 必须绑在变量上活到 run() 返回：guard 一 drop，缓冲的日志就没了。
    let _log_guard = init_logging();

    tauri::Builder::default()
        // 必须第一个注册。BT 会话独占监听端口和持久化状态，跑两份既起不来
        // 也会互相写坏 session；第二次启动改成把已有窗口拉到前面。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // 不能把错误往上抛：Tauri 会直接 panic!，用户看到的是系统的
            // 「意外退出」崩溃报告，完全看不出发生了什么。
            if let Err(e) = init_app(app) {
                fatal(app, &format!("{e:#}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            preview_torrent,
            cancel_preview,
            add_previewed,
            list_torrents,
            session_status,
            search_torrents,
            set_ai_key,
            has_ai_key,
            pause_torrent,
            resume_torrent,
            pause_all,
            pause_seeding,
            seeding_rarity,
            pause_common_seeding,
            torrent_health,
            check_health_now,
            torrent_forecast,
            download_stats,
            congestion_state,
            find_sources,
            network_interfaces,
            diagnose_torrent,
            verify_torrent,
            resume_all,
            delete_torrent,
            default_download_dir,
            get_settings,
            set_download_dir,
            save_settings,
            check_rss_now,
            reveal_path,
            list_files,
            set_only_files,
            stream_url,
            available_players,
            play_done_sound,
            open_in_player,
            restart_app,
            log_dir,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 退出前让 librqbit 把会话状态刷盘，否则重启会丢一截进度。
            if let tauri::RunEvent::Exit = event {
                // 累计上传量平时最多一分钟才写一次盘，退出前补一次，
                // 否则每次正常关闭都要丢掉最后那截。
                if let Some(ratio) = app.try_state::<Arc<ratio::RatioStore>>() {
                    ratio.save();
                }
                if let Some(engine) = app.try_state::<Arc<Engine>>() {
                    // 每日统计同理，而且它丢起来更狠：会话计数器随进程归零，
                    // 上一次采样（默认半小时一轮）到现在的流量补不回来。
                    // 必须赶在 shutdown 之前 —— 之后会话就停了。
                    if let Some(stats) = app.try_state::<Arc<stats::StatsStore>>() {
                        health::record_daily(&engine, &stats);
                    }
                    tauri::async_runtime::block_on(engine.shutdown());
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_label_strips_bundle_and_exe_suffix() {
        assert_eq!(player_label("/Applications/IINA.app"), "IINA");
        // brew 装的那种裸可执行文件 —— 正是自动检测认不出、需要手填的情况
        assert_eq!(player_label("/opt/homebrew/bin/mpv"), "mpv");
        assert_eq!(player_label("/x/vlc.exe"), "vlc");

        // 反斜杠只在 Windows 上算路径分隔符，别的平台上整条都是「文件名」。
        // 路径本来就来自各平台自己的文件选择器，所以只在 Windows 上断言。
        #[cfg(windows)]
        assert_eq!(player_label(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), "vlc");
    }
}
