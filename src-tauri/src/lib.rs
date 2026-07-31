pub mod automation;
pub mod engine;
pub mod keep_awake;
pub mod rss;
pub mod settings;
pub mod stream_server;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use engine::{Engine, FileView, TorrentId, TorrentPreview, TorrentView};
use settings::{Settings, SettingsStore};
use stream_server::StreamServer;
use tauri::{Manager, State};

/// 命令统一返回 String 错误，前端直接展示。anyhow 的 `{:#}` 会带上 context 链。
fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// 解析种子但不加入会话，让用户先看看里面有什么。
#[tauri::command]
async fn preview_torrent(
    engine: State<'_, Arc<Engine>>,
    uri: String,
) -> Result<TorrentPreview, String> {
    engine.preview(&uri).await.map_err(err)
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

#[tauri::command]
async fn pause_torrent(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<(), String> {
    engine.pause(id).await.map_err(err)
}

#[tauri::command]
async fn resume_torrent(engine: State<'_, Arc<Engine>>, id: TorrentId) -> Result<(), String> {
    engine.resume(id).await.map_err(err)
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
    engine.set_upload_limit(store.get().upload_limit_bps());
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

/// 日志目录，给界面上的「日志」入口用 —— 打包版看不到 stdout。
#[tauri::command]
fn log_dir() -> String {
    dirs_home()
        .join("Library/Logs/mydl")
        .to_string_lossy()
        .into_owned()
}

/// macOS 上常见的播放器。只返回真正装了的，界面按这个渲染按钮。
const KNOWN_PLAYERS: &[&str] = &["IINA", "VLC", "mpv", "QuickTime Player"];

#[tauri::command]
fn available_players() -> Vec<String> {
    let roots = [
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        dirs_home().join("Applications"),
    ];

    KNOWN_PLAYERS
        .iter()
        .filter(|name| {
            roots
                .iter()
                .any(|root| root.join(format!("{name}.app")).exists())
        })
        .map(|s| s.to_string())
        .collect()
}

/// 用指定播放器打开流地址。走 `open -a`，因为 http:// 交给系统默认处理会进浏览器。
#[tauri::command]
fn open_in_player(url: String, app: String) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .args(["-a", &app, &url])
        .status()
        .map_err(|e| format!("启动 {app} 失败：{e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{app} 退出码 {status}"))
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/").to_path_buf())
}

fn init_app(app: &tauri::App) -> anyhow::Result<()> {
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
    let trackers = if store.get().use_public_trackers {
        settings::PUBLIC_TRACKERS.iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    };
    tracing::info!(公共tracker = trackers.len(), "会话 tracker 配置");

    let (engine, server) = tauri::async_runtime::block_on(async {
        let engine = Arc::new(Engine::new(download_dir, None, config_dir.clone(), trackers).await?);
        let server = StreamServer::start(engine.clone()).await?;
        Ok::<_, anyhow::Error>((engine, server))
    })?;

    // 限速只存在设置里，会话本身不持久化它，所以每次启动都要重新应用。
    engine.set_upload_limit(store.get().upload_limit_bps());

    let seen = Arc::new(rss::SeenStore::load(rss::seen_path(&config_dir)));

    automation::spawn(app.handle().clone(), engine.clone(), store.clone());
    rss::spawn(engine.clone(), store.clone(), seen.clone());
    keep_awake::spawn(engine.clone(), store.clone());

    app.manage(engine);
    app.manage(server);
    app.manage(store);
    app.manage(seen);
    Ok(())
}

/// 日志同时写终端和文件。打包后 stdout 没人接，出了问题只能靠文件日志查。
///
/// 返回的 guard 必须活到进程结束，否则非阻塞写线程会被提前关掉、丢日志。
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_appender::rolling::{Builder, Rotation};
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let log_dir = dirs_home().join("Library/Logs/mydl");

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
            add_previewed,
            list_torrents,
            pause_torrent,
            resume_torrent,
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
            open_in_player,
            log_dir,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 退出前让 librqbit 把会话状态刷盘，否则重启会丢一截进度。
            if let tauri::RunEvent::Exit = event {
                if let Some(engine) = app.try_state::<Arc<Engine>>() {
                    tauri::async_runtime::block_on(engine.shutdown());
                }
            }
        });
}
