//! librqbit 会话的薄封装：把库的类型翻译成前端能直接消费的扁平结构。
//!
//! 这一层刻意不含 UI 逻辑，也不含 Tauri 类型，方便以后换界面或加 CLI。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
    SessionPersistenceConfig,
};
use serde::Serialize;

/// librqbit 把 `TorrentId` 和 `ManagedTorrentHandle` 定义在私有模块里、
/// 没在 crate 根重新导出，所以这里按其真实定义重建别名。
pub type TorrentId = usize;
type TorrentHandle = Arc<ManagedTorrent>;

/// librqbit 的 `Speed.mbps` 实际是 MiB/s，不是兆比特。统一在这里换算成字节/秒。
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

pub struct Engine {
    session: Arc<Session>,
    download_dir: PathBuf,
    /// 每个任务的自定义输出目录。librqbit 把它存在 `pub(crate)` 字段里读不到，
    /// 所以本进程自己记一份。重启后恢复的任务查不到，回退到默认目录。
    output_folders: Mutex<HashMap<TorrentId, PathBuf>>,
}

/// 单个任务在界面上需要的全部信息。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TorrentView {
    pub id: TorrentId,
    pub name: String,
    pub info_hash: String,
    /// initializing | live | paused | error
    pub state: String,
    pub error: Option<String>,
    pub finished: bool,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub uploaded_bytes: u64,
    pub download_speed_bps: f64,
    pub upload_speed_bps: f64,
    pub peers_live: usize,
    /// 已格式化的剩余时间，库里只暴露了 Display。
    pub eta: Option<String>,
}

impl Engine {
    /// `state_dir` 为 None 时用 librqbit 的系统默认配置目录；测试传临时目录，
    /// 免得跑测试把真实会话状态覆盖掉。
    pub async fn new(download_dir: PathBuf, state_dir: Option<PathBuf>) -> Result<Self> {
        std::fs::create_dir_all(&download_dir)
            .with_context(|| format!("无法创建下载目录 {}", download_dir.display()))?;

        let session = Session::new_with_opts(
            download_dir.clone(),
            SessionOptions {
                // 退出后把任务列表写盘，下次启动自动接着下。
                persistence: Some(SessionPersistenceConfig::Json { folder: state_dir }),
                // 重启后跳过全量校验。
                fastresume: true,
                // 不设这个的话 librqbit 根本不监听 TCP，只能主动连出、收不到入站 peer，
                // UPnP 映射也就没意义了。范围取 rqbit CLI 的默认值。
                listen_port_range: Some(4240..4260),
                enable_upnp_port_forwarding: true,
                ..Default::default()
            },
        )
        .await
        .context("创建 librqbit session 失败")?;

        Ok(Self {
            session,
            download_dir,
            output_folders: Mutex::new(HashMap::new()),
        })
    }

    pub fn download_dir(&self) -> &Path {
        &self.download_dir
    }

    /// 接受磁力链、种子文件的 http(s) 地址，或本地 .torrent 路径。
    ///
    /// `output_folder` 为空时下载到会话默认目录。逐个任务指定目录，
    /// 避免了改全局目录需要重建 session 的问题。
    pub async fn add(&self, uri: &str, output_folder: Option<String>) -> Result<TorrentId> {
        let uri = uri.trim();
        if uri.is_empty() {
            bail!("请输入磁力链、种子地址或本地种子文件路径");
        }

        let add = if uri.starts_with("magnet:")
            || uri.starts_with("http://")
            || uri.starts_with("https://")
        {
            AddTorrent::from_url(uri.to_owned())
        } else {
            AddTorrent::from_local_filename(uri)
                .with_context(|| format!("无法读取种子文件 {uri}"))?
        };

        let resp = self
            .session
            .add_torrent(
                add,
                Some(AddTorrentOptions {
                    // 不设的话，续传已存在的文件会报错。
                    overwrite: true,
                    output_folder: output_folder.clone(),
                    ..Default::default()
                }),
            )
            .await
            .context("添加任务失败")?;

        let id = match resp {
            AddTorrentResponse::Added(id, _) | AddTorrentResponse::AlreadyManaged(id, _) => id,
            // 只有显式设置 list_only 才会走到这里。
            AddTorrentResponse::ListOnly(_) => bail!("任务未被加入会话"),
        };

        if let Some(folder) = output_folder {
            self.output_folders
                .lock()
                .unwrap()
                .insert(id, PathBuf::from(folder));
        }

        Ok(id)
    }

    pub fn list(&self) -> Vec<TorrentView> {
        self.session
            .with_torrents(|it| it.map(|(id, handle)| view_of(id, handle)).collect())
    }

    pub async fn pause(&self, id: TorrentId) -> Result<()> {
        let handle = self.handle(id)?;
        self.session.pause(&handle).await.context("暂停失败")
    }

    pub async fn resume(&self, id: TorrentId) -> Result<()> {
        let handle = self.handle(id)?;
        self.session.unpause(&handle).await.context("继续失败")
    }

    pub async fn delete(&self, id: TorrentId, delete_files: bool) -> Result<()> {
        self.session
            .delete(id.into(), delete_files)
            .await
            .context("删除失败")?;
        self.output_folders.lock().unwrap().remove(&id);
        Ok(())
    }

    /// 任务内容在磁盘上的位置，用于「在访达中显示」。
    ///
    /// 单文件种子直接落在目录里，多文件种子会有一层同名子目录；这里返回
    /// 存在的那个，都不存在就返回目录本身。
    pub fn output_path(&self, id: TorrentId) -> Result<PathBuf> {
        let handle = self.handle(id)?;
        let base = self
            .output_folders
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.download_dir.clone());

        if let Some(name) = handle.name() {
            let candidate = base.join(name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        Ok(base)
    }

    pub async fn shutdown(&self) {
        self.session.stop().await;
    }

    fn handle(&self, id: TorrentId) -> Result<TorrentHandle> {
        self.session
            .get(id.into())
            .with_context(|| format!("找不到任务 {id}"))
    }
}

fn view_of(id: TorrentId, handle: &TorrentHandle) -> TorrentView {
    let stats = handle.stats();
    let live = stats.live.as_ref();
    // Id20 没实现 Display，as_string() 给的是十六进制。
    let info_hash = handle.info_hash().as_string();

    TorrentView {
        id,
        // 磁力链刚加进来还没拿到元信息时没有名字，先用 info hash 占位。
        name: handle.name().unwrap_or_else(|| info_hash.clone()),
        info_hash,
        state: stats.state.to_string(),
        error: stats.error.clone(),
        finished: stats.finished,
        progress_bytes: stats.progress_bytes,
        total_bytes: stats.total_bytes,
        uploaded_bytes: stats.uploaded_bytes,
        download_speed_bps: live.map_or(0.0, |l| l.download_speed.mbps * BYTES_PER_MIB),
        upload_speed_bps: live.map_or(0.0, |l| l.upload_speed.mbps * BYTES_PER_MIB),
        peers_live: live.map_or(0, |l| l.snapshot.peer_stats.live),
        eta: live.and_then(|l| l.time_remaining.as_ref().map(|t| t.to_string())),
    }
}
