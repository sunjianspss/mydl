pub mod engine;

use std::path::PathBuf;

use engine::{Engine, TorrentId, TorrentView};
use tauri::{Manager, State};

/// 命令统一返回 String 错误，前端直接展示。anyhow 的 `{:#}` 会带上 context 链。
fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn add_torrent(
    engine: State<'_, Engine>,
    uri: String,
    output_folder: Option<String>,
) -> Result<TorrentId, String> {
    engine.add(&uri, output_folder).await.map_err(err)
}

#[tauri::command]
fn list_torrents(engine: State<'_, Engine>) -> Vec<TorrentView> {
    engine.list()
}

#[tauri::command]
async fn pause_torrent(engine: State<'_, Engine>, id: TorrentId) -> Result<(), String> {
    engine.pause(id).await.map_err(err)
}

#[tauri::command]
async fn resume_torrent(engine: State<'_, Engine>, id: TorrentId) -> Result<(), String> {
    engine.resume(id).await.map_err(err)
}

#[tauri::command]
async fn delete_torrent(
    engine: State<'_, Engine>,
    id: TorrentId,
    delete_files: bool,
) -> Result<(), String> {
    engine.delete(id, delete_files).await.map_err(err)
}

#[tauri::command]
fn default_download_dir(engine: State<'_, Engine>) -> String {
    engine.download_dir().to_string_lossy().into_owned()
}

/// 返回磁盘路径，前端交给 opener 插件在访达里显示。
#[tauri::command]
fn reveal_path(engine: State<'_, Engine>, id: TorrentId) -> Result<String, String> {
    engine
        .output_path(id)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(err)
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
            let engine = tauri::async_runtime::block_on(Engine::new(download_dir, None))?;
            app.manage(engine);
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 退出前让 librqbit 把会话状态刷盘，否则重启会丢一截进度。
            if let tauri::RunEvent::Exit = event {
                if let Some(engine) = app.try_state::<Engine>() {
                    tauri::async_runtime::block_on(engine.shutdown());
                }
            }
        });
}
