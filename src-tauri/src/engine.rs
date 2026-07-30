//! librqbit 会话的薄封装：把库的类型翻译成前端能直接消费的扁平结构。
//!
//! 这一层刻意不含 UI 逻辑，也不含 Tauri 类型，方便以后换界面或加 CLI。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ByteBufOwned, ListOnlyResponse,
    ManagedTorrent, Session, SessionOptions, SessionPersistenceConfig, TorrentMetaV1Info,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncSeek};

/// librqbit 把 `TorrentId` 和 `ManagedTorrentHandle` 定义在私有模块里、
/// 没在 crate 根重新导出，所以这里按其真实定义重建别名。
pub type TorrentId = usize;
type TorrentHandle = Arc<ManagedTorrent>;

/// librqbit 的 `Speed.mbps` 实际是 MiB/s，不是兆比特。统一在这里换算成字节/秒。
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

/// 起播前等待任务初始化的上限。校验已有文件可能要点时间，但不能无限等。
const INIT_WAIT: Duration = Duration::from_secs(30);

/// 添加任务的上限。librqbit 解析磁力链元信息时没有超时
/// （`session.rs` 的 `read_metainfo_from_peer_receiver`），冷门磁力链会一直挂着，
/// 命令永不返回，界面就跟着卡死。
const ADD_TIMEOUT: Duration = Duration::from_secs(120);

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

/// 种子内的单个文件。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub index: usize,
    /// 种子内的相对路径。
    pub name: String,
    pub len: u64,
    pub downloaded: u64,
    /// 是否是能边下边播的媒体格式。
    pub playable: bool,
}

/// 能拿去边下边播的容器格式。列表之外的（压缩包、镜像等）播放没有意义。
const PLAYABLE_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "ts", "m2ts", "flv", "wmv", "ogv", "mpg", "mpeg",
    "m2v", "3gp", "mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma",
];

/// 多文件种子该放进哪个子目录，单文件返回 None。
///
/// 这是在补 librqbit 的行为：它只在用默认下载目录时才做这件事。种子名是
/// 外部输入，必须挡住 `../` 之类的路径穿越。
fn subfolder_for(info: &TorrentMetaV1Info<ByteBufOwned>) -> Result<Option<PathBuf>> {
    if info.iter_file_details()?.count() < 2 {
        return Ok(None);
    }

    let name = match &info.name {
        Some(n) => String::from_utf8_lossy(n).into_owned(),
        None => return Ok(None),
    };
    if name.is_empty() {
        return Ok(None);
    }

    let pb = PathBuf::from(&name);
    if pb.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("种子名里有路径穿越：{name}");
    }
    Ok(Some(pb))
}

/// 超时原因对磁力链和普通种子完全不同，分开说清楚，别让用户干猜。
fn add_timeout_message(uri: &str) -> String {
    let secs = ADD_TIMEOUT.as_secs();
    if uri.starts_with("magnet:") {
        let has_tracker = uri.contains("&tr=") || uri.contains("?tr=");
        let hint = if has_tracker {
            "tracker 和 DHT 都没找到能提供元信息的源"
        } else {
            "这条磁力链不带 tracker，只能靠 DHT 找源"
        };
        format!(
            "解析磁力链超时（{secs} 秒）。添加磁力链必须先从其他 peer 拿到文件列表，\
             但{hint} —— 通常说明这个资源已经没人做种了。\
             如果能拿到对应的 .torrent 文件，用「打开种子…」可以直接添加。"
        )
    } else {
        format!("添加超时（{secs} 秒）。种子地址可能打不开，或者网络有问题。")
    }
}

fn is_playable(name: &str) -> bool {
    name.rsplit('.')
        .next()
        .map(|e| PLAYABLE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

impl Engine {
    /// `state_dir` 为 None 时用 librqbit 的系统默认配置目录（正常运行）；
    /// 传了目录就代表这是个隔离实例（测试），此时连 DHT 也不共用全局缓存 ——
    /// 否则不但会污染真实的 DHT 路由表，还会因为持久化里记着固定端口而
    /// 无法同时跑两个实例。
    pub async fn new(download_dir: PathBuf, state_dir: Option<PathBuf>) -> Result<Self> {
        std::fs::create_dir_all(&download_dir)
            .with_context(|| format!("无法创建下载目录 {}", download_dir.display()))?;

        let isolated = state_dir.is_some();

        let session = Session::new_with_opts(
            download_dir.clone(),
            SessionOptions {
                // 退出后把任务列表写盘，下次启动自动接着下。
                persistence: Some(SessionPersistenceConfig::Json { folder: state_dir }),
                disable_dht_persistence: isolated,
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

        // 之前这里什么都不记，任务卡住时日志里连「试过添加」都看不出来。
        tracing::info!(uri = %uri, "添加任务");

        // 超时后 future 被 drop，librqbit 那边的解析也随之取消。
        match tokio::time::timeout(ADD_TIMEOUT, self.add_inner(uri, output_folder)).await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(uri = %uri, "添加超时");
                bail!("{}", add_timeout_message(uri))
            }
        }
    }

    async fn add_inner(&self, uri: &str, output_folder: Option<String>) -> Result<TorrentId> {

        // 指定了自定义目录时，librqbit 会原样使用它、跳过自动建子目录的逻辑
        // （session.rs 里 `(Some(o), None) => PathBuf::from(o)`），多文件种子
        // 就会把几十个文件直接倒进目标目录。所以先探一次种子内容，自己把
        // 子目录拼好。探测返回的 torrent_bytes 可以直接复用，磁力链不用重解析。
        let (add, output_folder) = match &output_folder {
            None => (self.make_add_torrent(uri)?, None),
            // 和会话默认目录一致时不用自己拼：librqbit 会正确建子目录，
            // 顺便省掉一次探测。
            Some(dir) if Path::new(dir) == self.download_dir => {
                (self.make_add_torrent(uri)?, None)
            }
            Some(dir) => {
                let probe = self.probe(uri).await?;
                let folder = match subfolder_for(&probe.info)? {
                    Some(sub) => PathBuf::from(dir).join(sub),
                    None => PathBuf::from(dir),
                };
                (
                    AddTorrent::from_bytes(probe.torrent_bytes),
                    Some(folder.to_string_lossy().into_owned()),
                )
            }
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

    fn make_add_torrent<'a>(&self, uri: &'a str) -> Result<AddTorrent<'a>> {
        if uri.starts_with("magnet:") || uri.starts_with("http://") || uri.starts_with("https://") {
            Ok(AddTorrent::from_url(uri.to_owned()))
        } else {
            AddTorrent::from_local_filename(uri)
                .with_context(|| format!("无法读取种子文件 {uri}"))
        }
    }

    /// 只解析种子、不加入会话，用来提前知道它有几个文件、叫什么名字。
    async fn probe(&self, uri: &str) -> Result<ListOnlyResponse> {
        let resp = self
            .session
            .add_torrent(
                self.make_add_torrent(uri)?,
                Some(AddTorrentOptions {
                    list_only: true,
                    ..Default::default()
                }),
            )
            .await
            .context("解析种子失败")?;

        match resp {
            AddTorrentResponse::ListOnly(r) => Ok(r),
            _ => bail!("bug: list_only 却返回了非 ListOnly 结果"),
        }
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

    /// 种子内的文件列表。磁力链还没解析出元信息时返回空列表，而不是报错 ——
    /// 界面会在下一次轮询里自然拿到。
    pub fn files(&self, id: TorrentId) -> Result<Vec<FileView>> {
        let handle = self.handle(id)?;
        let progress = handle.stats().file_progress;

        let infos = match handle.with_metadata(|m| m.file_infos.clone()) {
            Ok(infos) => infos,
            Err(_) => return Ok(Vec::new()),
        };

        Ok(infos
            .into_iter()
            .enumerate()
            .map(|(index, info)| {
                let name = info.relative_filename.to_string_lossy().into_owned();
                FileView {
                    playable: is_playable(&name),
                    index,
                    len: info.len,
                    downloaded: progress.get(index).copied().unwrap_or(0),
                    name,
                }
            })
            .collect())
    }

    /// 打开一路边下边播的流。librqbit 会把这个文件的分片提到最高优先级。
    ///
    /// 返回类型写成 `impl Trait` 是因为 librqbit 的 `FileStream` 定义在私有模块里、
    /// 没在 crate 根导出，外部根本命名不了。文件长度从 [`Self::files`] 拿。
    pub async fn open_stream(
        &self,
        id: TorrentId,
        file_id: usize,
    ) -> Result<impl AsyncRead + AsyncSeek + Send + Unpin + 'static> {
        let handle = self.handle(id)?;

        // 元信息解析完之后、存储初始化完成之前，任务还是 initializing 状态，
        // 这时候 stream() 会直接失败。等它就绪 —— 但不能无限等，否则用户一点播放
        // 播放器就永远挂在那里。
        tokio::time::timeout(INIT_WAIT, handle.wait_until_initialized())
            .await
            .with_context(|| format!("等待任务 {id} 初始化超过 {}s", INIT_WAIT.as_secs()))?
            .with_context(|| format!("任务 {id} 初始化失败"))?;

        // 暂停状态下不会有新数据进来，播放器只会卡住，不如直接说清楚。
        if handle.is_paused() {
            bail!("任务已暂停，继续下载后才能播放");
        }

        handle
            .stream(file_id)
            .with_context(|| format!("无法打开任务 {id} 的文件 {file_id}"))
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
