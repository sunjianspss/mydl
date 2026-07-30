pub mod engine;
pub mod stream_server;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use engine::{Engine, FileView, TorrentId, TorrentView};
use stream_server::StreamServer;
use tauri::{Manager, State};

/// 命令统一返回 String 错误，前端直接展示。anyhow 的 `{:#}` 会带上 context 链。
fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn add_torrent(
    engine: State<'_, Arc<Engine>>,
    uri: String,
    output_folder: Option<String>,
) -> Result<TorrentId, String> {
    engine.add(&uri, output_folder).await.map_err(err)
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

#[tauri::command]
fn default_download_dir(engine: State<'_, Arc<Engine>>) -> String {
    engine.download_dir().to_string_lossy().into_owned()
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,librqbit=info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let download_dir = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| PathBuf::from("."));

            // Session 启动包含读取持久化状态和绑定监听端口，必须在窗口出现前完成，
            // 否则前端第一次 list_torrents 会拿不到 State。
            let (engine, server) = tauri::async_runtime::block_on(async {
                let engine = Arc::new(Engine::new(download_dir, None).await?);
                let server = StreamServer::start(engine.clone()).await?;
                Ok::<_, anyhow::Error>((engine, server))
            })?;

            app.manage(engine);
            app.manage(server);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_torrent,
            list_torrents,
            pause_torrent,
            resume_torrent,
            delete_torrent,
            default_download_dir,
            reveal_path,
            list_files,
            stream_url,
            available_players,
            open_in_player,
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
