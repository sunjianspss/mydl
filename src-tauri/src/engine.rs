//! librqbit 会话的薄封装：把库的类型翻译成前端能直接消费的扁平结构。
//!
//! 这一层刻意不含 UI 逻辑，也不含 Tauri 类型，方便以后换界面或加 CLI。

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ByteBufOwned, DhtSessionConfig,
    ListOnlyResponse, ListenerMode, ListenerOptions, ManagedTorrent, Session, SessionOptions,
    SessionPersistenceConfig, ValidatedTorrentMetaV1Info,
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
    /// 每个任务的自定义输出目录。librqbit 把它存在 `pub(crate)` 字段里读不到
    /// （`ManagedTorrentShared.options` 整个是 `pub(crate)`），所以自己记一份。
    ///
    /// 按 **info-hash** 而不是 TorrentId 索引：id 是会话重启后重新分配的，
    /// 只有 info-hash 跨重启稳定。整份内容落在 [`Self::folders_path`]。
    output_folders: Mutex<HashMap<String, PathBuf>>,
    /// `output_folders` 的落盘位置。
    folders_path: PathBuf,
    /// 预览过、但还没确认添加的种子。留着 torrent_bytes 是为了确认时不用
    /// 重新解析一遍 —— 磁力链解析一次可能要几十秒。
    previews: Mutex<HashMap<String, CachedPreview>>,
    /// 正在进行的那次预览的取消信号。同一时间只允许一个预览（界面在解析
    /// 期间会禁掉添加按钮），所以一个槽就够。
    preview_cancel: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

struct CachedPreview {
    torrent_bytes: Vec<u8>,
    /// 预览时就算好，确认时直接用。
    subfolder: Option<PathBuf>,
}

/// 预览里的单个文件。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFile {
    pub index: usize,
    pub name: String,
    pub len: u64,
    pub playable: bool,
}

/// 添加前的种子内容预览。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TorrentPreview {
    /// 确认添加时带回来，用来取回缓存的 torrent_bytes。
    pub token: String,
    pub name: String,
    pub info_hash: String,
    pub total_bytes: u64,
    pub files: Vec<PreviewFile>,
    /// 这个种子已经在任务列表里了。
    pub already_added: bool,
}

/// 最多缓存几份未确认的预览。预览是临时的，超了直接整个清掉最省事。
const MAX_PREVIEWS: usize = 8;

/// 底部状态栏要显示的会话级信息。
///
/// 只放真的拿得到的：DHT 路由表大小和实际监听端口。UPnP 映射结果 librqbit
/// 没有暴露接口，所以不显示 —— 状态栏写一个猜的结论比不写更糟。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    /// DHT 路由表里的节点数。None = DHT 没启用或还没起来。
    pub dht_nodes: Option<usize>,
    pub listen_port: Option<u16>,
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
    /// 是否在下载范围内。未选中的文件不会被请求。
    pub selected: bool,
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
fn subfolder_for(info: &ValidatedTorrentMetaV1Info<ByteBufOwned>) -> Result<Option<PathBuf>> {
    if info.iter_file_details().count() < 2 {
        return Ok(None);
    }

    let name = match info.name() {
        Some(n) => n.into_owned(),
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
    ///
    /// `data_dir` 放 Engine 自己的旁路文件（目前只有输出目录表），正常运行时
    /// 就是 App 的配置目录 —— 和 settings.json、rss_seen.json 放在一起。
    pub async fn new(
        download_dir: PathBuf,
        state_dir: Option<PathBuf>,
        data_dir: PathBuf,
        extra_trackers: Vec<String>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&download_dir)
            .with_context(|| format!("无法创建下载目录 {}", download_dir.display()))?;

        let isolated = state_dir.is_some();

        let trackers = extra_trackers
            .iter()
            .filter_map(|t| match t.parse() {
                Ok(u) => Some(u),
                Err(e) => {
                    tracing::warn!("跳过无效的 tracker 地址 {t}：{e}");
                    None
                }
            })
            .collect();

        let session = Session::new_with_opts(
            download_dir.clone(),
            SessionOptions {
                // 退出后把任务列表写盘，下次启动自动接着下。
                persistence: Some(SessionPersistenceConfig::Json { folder: state_dir }),
                // v9 里 dht: None 是「关闭 DHT」，必须显式给配置。
                // 隔离实例（测试）不共用全局 DHT 缓存，否则会抢固定端口。
                dht: Some(DhtSessionConfig {
                    persistence: if isolated { None } else { Some(Default::default()) },
                    ..Default::default()
                }),
                // 重启后跳过全量校验。
                fastresume: true,
                listen: Some(ListenerOptions {
                    // 关键：只监听 TCP 的话，连不上绝大多数家用 NAT 后的 peer ——
                    // 现代客户端默认走 uTP。8.x 根本没有 uTP，这是磁力链
                    // 老是解析不出元信息的真正原因。
                    mode: ListenerMode::TcpAndUtp,
                    // v9 用固定端口取代了 8.x 的端口范围，所以隔离实例（测试）
                    // 必须用 0 让系统随机分配，否则并行跑就会互相抢端口。
                    listen_addr: (
                        std::net::Ipv6Addr::UNSPECIFIED,
                        if isolated { 0 } else { 4240 },
                    )
                        .into(),
                    // 随机端口做 UPnP 映射没意义。
                    enable_upnp_port_forwarding: !isolated,
                    ..Default::default()
                }),
                // 会话级补充 tracker，对只有裸 info-hash 的磁力链多一条找源的路。
                trackers,
                ..Default::default()
            },
        )
        .await
        .context("创建 librqbit session 失败")?;

        let folders_path = data_dir.join("output_folders.json");
        Ok(Self {
            session,
            download_dir,
            output_folders: Mutex::new(read_folders(&folders_path)),
            folders_path,
            previews: Mutex::new(HashMap::new()),
            preview_cancel: Mutex::new(None),
        })
    }

    /// 全局上传限速，单位字节/秒；None 或 0 表示不限。
    ///
    /// 这个开关是必要的：librqbit 的 uTP 用 CUBIC 拥塞控制
    /// （librqbit-utp 0.7 的 lib.rs 里还写着 `// TODO: LEDBAT congestion
    /// control`），不会像正经 uTP 那样给其他流量让路。做种时上行打满，
    /// 同一条线路上的一切都会跟着卡。
    ///
    /// 运行时可改，不需要重建会话。
    pub fn set_upload_limit(&self, bps: Option<u32>) {
        let limit = bps.and_then(NonZeroU32::new);
        self.session.ratelimits.set_upload_bps(limit);
        match limit {
            Some(v) => tracing::info!("上传限速：{} 字节/秒", v.get()),
            None => tracing::info!("上传限速：不限"),
        }
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

        let (id, handle) = match resp {
            AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            // 只有显式设置 list_only 才会走到这里。
            AddTorrentResponse::ListOnly(_) => bail!("任务未被加入会话"),
        };

        if let Some(folder) = output_folder {
            self.remember_folder(&handle, PathBuf::from(folder));
        }

        Ok(id)
    }

    /// 解析种子但**不加入会话**，返回文件列表供用户勾选。
    ///
    /// torrent_bytes 会被缓存起来，[`Self::add_previewed`] 直接复用，
    /// 所以磁力链只解析这一次。
    /// 返回 None 表示用户主动取消了 —— 那不是错误，界面不该弹红条。
    pub async fn preview(&self, uri: &str) -> Result<Option<TorrentPreview>> {
        let uri = uri.trim();
        if uri.is_empty() {
            bail!("请输入磁力链、种子地址或本地种子文件路径");
        }

        tracing::info!(uri = %uri, "预览种子");

        let cancel = Arc::new(tokio::sync::Notify::new());
        *self.preview_cancel.lock().unwrap() = Some(cancel.clone());

        // select 的另一条分支被选中时，probe 那个 future 会被 drop —— librqbit
        // 那边的解析也就随之取消，和超时走的是同一条路。
        let outcome = tokio::select! {
            r = tokio::time::timeout(ADD_TIMEOUT, self.probe(uri)) => Some(r),
            _ = cancel.notified() => None,
        };
        self.preview_cancel.lock().unwrap().take();

        let probe = match outcome {
            None => {
                tracing::info!(uri = %uri, "预览已被用户取消");
                return Ok(None);
            }
            Some(Ok(r)) => r?,
            Some(Err(_)) => {
                tracing::warn!(uri = %uri, "预览超时");
                bail!("{}", add_timeout_message(uri))
            }
        };

        let files: Vec<PreviewFile> = probe
            .info
            .iter_file_details()
            .enumerate()
            .map(|(index, fd)| {
                // v9 里 to_pathbuf() 已经不返回 Result 了。
                let name = fd.filename.to_pathbuf().to_string_lossy().into_owned();
                PreviewFile {
                    index,
                    playable: is_playable(&name),
                    name,
                    len: fd.len,
                }
            })
            .collect();

        let info_hash = probe.info_hash.as_string();
        let already_added = self.session.get(probe.info_hash.into()).is_some();

        let token = format!("{:032x}", rand::random::<u128>());
        {
            let mut cache = self.previews.lock().unwrap();
            if cache.len() >= MAX_PREVIEWS {
                cache.clear();
            }
            cache.insert(
                token.clone(),
                CachedPreview {
                    torrent_bytes: probe.torrent_bytes.to_vec(),
                    subfolder: subfolder_for(&probe.info)?,
                },
            );
        }

        Ok(Some(TorrentPreview {
            token,
            name: probe
                .info
                .name()
                .map(|n| n.into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| info_hash.clone()),
            info_hash,
            total_bytes: files.iter().map(|f| f.len).sum(),
            files,
            already_added,
        }))
    }

    /// 打断正在进行的预览。没有正在进行的就什么也不做。
    ///
    /// 用 `notify_one` 而不是 `notify_waiters`：前者在还没有人等待时会把这次
    /// 通知存下来，后者会直接丢掉 —— 用户手快、在 select 第一次轮询之前就
    /// 点了取消的话，信号会丢。
    pub fn cancel_preview(&self) {
        if let Some(n) = self.preview_cancel.lock().unwrap().as_ref() {
            n.notify_one();
        }
    }

    /// 确认添加之前预览过的种子，只下 `only_files` 里的文件。
    pub async fn add_previewed(
        &self,
        token: &str,
        only_files: Vec<usize>,
        output_folder: Option<String>,
    ) -> Result<TorrentId> {
        if only_files.is_empty() {
            bail!("至少要选一个文件");
        }

        let cached = self
            .previews
            .lock()
            .unwrap()
            .remove(token)
            .context("这份预览已失效，请重新添加")?;

        let folder = match &output_folder {
            None => None,
            // 和会话默认目录一致时交给 librqbit 自己建子目录。
            Some(dir) if Path::new(dir) == self.download_dir => None,
            Some(dir) => Some(match &cached.subfolder {
                Some(sub) => PathBuf::from(dir).join(sub),
                None => PathBuf::from(dir),
            }),
        };

        let resp = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(cached.torrent_bytes),
                Some(AddTorrentOptions {
                    overwrite: true,
                    output_folder: folder.as_ref().map(|f| f.to_string_lossy().into_owned()),
                    only_files: Some(only_files),
                    ..Default::default()
                }),
            )
            .await
            .context("添加任务失败")?;

        let (id, handle) = match resp {
            AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            AddTorrentResponse::ListOnly(_) => bail!("任务未被加入会话"),
        };

        if let Some(folder) = folder {
            self.remember_folder(&handle, folder);
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

    pub fn session_status(&self) -> SessionStatus {
        SessionStatus {
            dht_nodes: self.session.get_dht().map(|d| d.stats().routing_table_size),
            listen_port: self.session.listen_addr().map(|a| a.port()),
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
        // 删完就拿不到 handle 了，info-hash 得先取出来。
        let info_hash = self.handle(id).ok().map(|h| h.info_hash().as_string());

        self.session
            .delete(id.into(), delete_files)
            .await
            .context("删除失败")?;

        if let Some(hash) = info_hash {
            let mut map = self.output_folders.lock().unwrap();
            if map.remove(&hash).is_some() {
                self.save_folders(&map);
            }
        }
        Ok(())
    }

    /// 记下某个任务的自定义输出目录，并立刻落盘 —— 写不进去只警告，
    /// 不能因为一份「在访达中显示」用的备忘录失败就让添加任务失败。
    fn remember_folder(&self, handle: &TorrentHandle, folder: PathBuf) {
        let mut map = self.output_folders.lock().unwrap();
        map.insert(handle.info_hash().as_string(), folder);
        self.save_folders(&map);
    }

    fn save_folders(&self, map: &HashMap<String, PathBuf>) {
        if let Err(e) = write_folders(&self.folders_path, map) {
            tracing::warn!("保存任务目录记录失败：{e:#}");
        }
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
            .get(&handle.info_hash().as_string())
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

        // None 表示没有做过筛选，也就是全选。
        let only = handle.only_files();

        Ok(infos
            .into_iter()
            .enumerate()
            .map(|(index, info)| {
                let name = info.relative_filename.to_string_lossy().into_owned();
                FileView {
                    playable: is_playable(&name),
                    selected: only.as_ref().is_none_or(|v| v.contains(&index)),
                    index,
                    len: info.len,
                    downloaded: progress.get(index).copied().unwrap_or(0),
                    name,
                }
            })
            .collect())
    }

    /// 设置只下载哪些文件。未选中的文件不再被请求，但**已经下好的数据不会删** ——
    /// librqbit 只改 chunk tracker，不动磁盘。
    pub async fn set_only_files(&self, id: TorrentId, files: Vec<usize>) -> Result<()> {
        if files.is_empty() {
            bail!("至少要选一个文件；一个都不要的话请直接删除任务");
        }

        let handle = self.handle(id)?;
        let total = self.files(id)?.len();
        if let Some(bad) = files.iter().find(|i| **i >= total) {
            bail!("文件序号 {bad} 超出范围（共 {total} 个）");
        }

        let selection: HashSet<usize> = files.into_iter().collect();
        self.session
            .update_only_files(&handle, &selection)
            .await
            // 初始化中改不了，librqbit 会直接报错，这里给句人话。
            .with_context(|| {
                if handle.stats().state.to_string() == "initializing" {
                    "任务还在初始化，等它开始下载后再改".to_string()
                } else {
                    format!("无法修改任务 {id} 的文件选择")
                }
            })
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
            .await
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

/// 读回 info-hash → 输出目录 的映射。文件不存在或坏了都当空表继续 ——
/// 大不了「在访达中显示」退回默认下载目录，不值得让 App 起不来。
fn read_folders(path: &Path) -> HashMap<String, PathBuf> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        // 首次启动没有这个文件，是正常情况。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            tracing::warn!("读取任务目录记录失败：{e}");
            return HashMap::new();
        }
    };

    match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("任务目录记录不是合法 JSON，忽略：{e}");
            HashMap::new()
        }
    }
}

/// 先写临时文件再 rename，免得写到一半崩了留下半份坏文件。
fn write_folders(path: &Path, map: &HashMap<String, PathBuf>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录 {}", parent.display()))?;
    }

    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(map).context("序列化任务目录记录失败")?;
    std::fs::write(&tmp, json).with_context(|| format!("无法写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("无法替换 {}", path.display()))
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
        // v9 把这个字段从 usize 改成了 u32。
        peers_live: live.map_or(0, |l| l.snapshot.peer_stats.live as usize),
        eta: live.and_then(|l| l.time_remaining.as_ref().map(|t| t.to_string())),
    }
}
