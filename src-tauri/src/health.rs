//! Swarm 健康度采样。
//!
//! 定期向公共 tracker 做 BEP15 scrape，把每个任务的做种/下载人数存成时间
//! 序列，用来回答两个现有客户端都答不了的问题：**这个 swarm 在变好还是
//! 变差**，以及**还值不值得等下去**。
//!
//! 为什么是 scrape 而不是别的：
//!
//! - DHT 只回答「谁有这个 info-hash」，给不出总量，而且要为每个种子跑一次
//!   完整的 lookup，很贵。
//! - BEP15 的 scrape **一个 UDP 包能查 74 个种子**，一轮下来全部任务只要
//!   几个包，是现成且极省的数据源。
//! - 单个 tracker 会撒谎、会数据陈旧、会连不上（实测 5 个公共 tracker 里
//!   有 2 个从某些网络根本够不着），所以多个交叉验证，取最大值。
//!
//! 刻意不做的事：不预测「还要下多久」。那取决于你能连上几个 peer、对方给
//! 不给你带宽，不是 swarm 规模能决定的，硬报一个数字只会误导。这里只说
//! swarm 本身的状态和趋势。

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use crate::engine::Engine;
use crate::settings::{SettingsStore, PUBLIC_TRACKERS};

/// BEP15 握手用的魔数。
const PROTOCOL_ID: u64 = 0x0417_2710_1980;

/// 一个 scrape 请求最多带几个 info-hash。BEP15 的说法是「约 74 个」——
/// 16 字节头 + 74×20 = 1496，刚好压在常见 MTU 以内，再多就会分片。
const MAX_PER_SCRAPE: usize = 74;

/// 单个 tracker 的往返上限。采样是后台行为，慢一点无所谓，但不能吊着。
const TRACKER_TIMEOUT: Duration = Duration::from_secs(6);

/// 每个种子最多留多少个样本。半小时一次的话约等于 10 天。
const MAX_SAMPLES: usize = 480;

/// 判「已死」至少要这么多个有效样本 —— 一次采样全部 tracker 都超时也会
/// 得到 0 做种，不能凭一次就下结论。
const MIN_SAMPLES_FOR_DEAD: usize = 3;

/// 下载人数是做种人数的多少倍算「僧多粥少」。
const STARVING_RATIO: u32 = 5;

/// 判趋势至少要几个样本。
const MIN_SAMPLES_FOR_TREND: usize = 4;

/// 启动后多久采第一轮。见 [`spawn`]。
const STARTUP_DELAY: Duration = Duration::from_secs(20);

/// 趋势的判定阈值：前后两半的均值差超过这个比例才算涨/跌。
const TREND_THRESHOLD: f64 = 0.2;

// ---------------------------------------------------------------------------
// 数据
// ---------------------------------------------------------------------------

/// 一轮采样对一个种子的合并结果。
///
/// 存合并值而不是每个 tracker 一条：一个种子挂十天就是 480 条，再乘 5 个
/// tracker 就没必要了，而且下判断时用的本来就是合并值。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Sample {
    /// Unix 秒。
    pub ts: i64,
    /// 各 tracker 里的最大值。每个 tracker 只知道向**它**汇报过的那部分
    /// peer，取最大比取平均更接近真实规模。
    pub seeders: u32,
    pub leechers: u32,
    /// 这轮有几个 tracker 应答了。0 表示全都没通，这个样本不可信 ——
    /// 判定时会跳过，否则一次断网就会被误判成「没源了」。
    pub trackers_ok: u8,

    /// 采样时已下到的字节数。
    ///
    /// **这才是回答「还要多久」的那个信号。** swarm 规模答不了这个问题 ——
    /// 实测做种数的变异系数 34%~151%，拿它外推等于编数字。而两次采样之间
    /// 进度涨了多少，是实打实测出来的吞吐量。
    ///
    /// `Option` 是因为这个字段是后加的，老样本里没有（`serde(default)`）。
    #[serde(default)]
    pub progress: Option<u64>,

    /// 采样时的总字节数。任务改过文件选择的话会变，所以要跟着存。
    #[serde(default)]
    pub total: Option<u64>,
}

impl Sample {
    fn trustworthy(&self) -> bool {
        self.trackers_ok > 0
    }
}

/// swarm 当前处在什么状态。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// 样本还不够，别下结论。
    Unknown,
    /// 连续多轮所有 tracker 都报 0 做种。
    Dead,
    /// 有做种，但下载的人多太多，分不到带宽。
    Starving,
    Ok,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Trend {
    Unknown,
    Rising,
    Falling,
    Flat,
}

/// 给界面看的结论。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub status: Status,
    pub trend: Trend,
    /// 最近一个**可信**样本；一个都没有就是 None。
    pub latest: Option<Sample>,
    /// 可信样本的个数。界面用它决定要不要显示「数据还不够」。
    pub samples: usize,
    /// 一句话结论。措辞在 Rust 这边定死，不交给模型 —— 这些数字是要拿来
    /// 做决定的，不能有一点发挥空间。
    pub summary: String,
    /// 做种人数曲线，按时间先后。给界面画迷你走势图用。
    pub seeders_series: Vec<u32>,
}

// ---------------------------------------------------------------------------
// 持久化
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct HealthData {
    /// info-hash（小写十六进制）-> 样本，按时间先后。
    torrents: HashMap<String, VecDeque<Sample>>,
}

pub struct HealthStore {
    path: PathBuf,
    data: Mutex<HealthData>,
}

impl HealthStore {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    fn record(&self, info_hash: &str, sample: Sample) {
        let mut data = self.data.lock().unwrap();
        let q = data.torrents.entry(info_hash.to_ascii_lowercase()).or_default();
        q.push_back(sample);
        while q.len() > MAX_SAMPLES {
            q.pop_front();
        }
    }

    pub fn history(&self, info_hash: &str) -> Vec<Sample> {
        self.data
            .lock()
            .unwrap()
            .torrents
            .get(&info_hash.to_ascii_lowercase())
            .map(|q| q.iter().copied().collect())
            .unwrap_or_default()
    }

    /// 任务被删掉之后它的历史就没意义了，顺手清掉，免得文件无限长。
    fn retain(&self, keep: &HashSet<String>) {
        let mut data = self.data.lock().unwrap();
        data.torrents.retain(|k, _| keep.contains(k));
    }

    /// 写盘失败只会丢掉历史，不影响下载，所以不往上抛。
    fn save(&self) {
        let data = self.data.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string(&*data) {
            Ok(json) => {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &self.path);
                }
            }
            Err(e) => tracing::warn!("序列化健康度记录失败：{e:#}"),
        }
    }
}

pub fn health_path(config_dir: &Path) -> PathBuf {
    config_dir.join("swarm_health.json")
}

// ---------------------------------------------------------------------------
// 判定
// ---------------------------------------------------------------------------

/// 从历史样本里读出结论。纯函数，好测。
pub fn verdict(history: &[Sample]) -> Verdict {
    let valid: Vec<Sample> = history.iter().copied().filter(Sample::trustworthy).collect();
    let series: Vec<u32> = valid.iter().map(|s| s.seeders).collect();

    let Some(latest) = valid.last().copied() else {
        return Verdict {
            status: Status::Unknown,
            trend: Trend::Unknown,
            latest: None,
            samples: 0,
            summary: "还没采到 swarm 数据".into(),
            seeders_series: series,
        };
    };

    let trend = trend_of(&series);

    // 「已死」要连续多轮都是 0，避免被一次网络抖动骗到。
    let tail_all_zero = valid.len() >= MIN_SAMPLES_FOR_DEAD
        && valid
            .iter()
            .rev()
            .take(MIN_SAMPLES_FOR_DEAD)
            .all(|s| s.seeders == 0);

    let (status, summary) = if tail_all_zero {
        (
            Status::Dead,
            format!(
                "连续 {} 轮采样都是 0 做种，这个种子已经没人做了 —— 换一个源比等有用",
                MIN_SAMPLES_FOR_DEAD
            ),
        )
    } else if latest.seeders == 0 {
        (
            Status::Unknown,
            format!(
                "这轮 {} 个 tracker 都报 0 做种，再采几轮才能确认是不是真没了",
                latest.trackers_ok
            ),
        )
    } else if latest.leechers >= latest.seeders.saturating_mul(STARVING_RATIO) {
        (
            Status::Starving,
            format!(
                "{} 个做种要分给 {} 个下载的，慢是正常的{}",
                latest.seeders,
                latest.leechers,
                trend_suffix(trend)
            ),
        )
    } else {
        (
            Status::Ok,
            format!(
                "{} 个做种 · {} 个在下{}",
                latest.seeders,
                latest.leechers,
                trend_suffix(trend)
            ),
        )
    };

    Verdict {
        status,
        trend,
        latest: Some(latest),
        samples: valid.len(),
        summary,
        seeders_series: series,
    }
}

fn trend_suffix(trend: Trend) -> &'static str {
    match trend {
        Trend::Rising => "，做种人数在涨",
        Trend::Falling => "，做种人数在掉",
        _ => "",
    }
}

/// 前一半和后一半的均值比，超过阈值才算有趋势。
///
/// 用两半均值而不是「最新 vs 最老」：做种人数抖得厉害，取两个点很容易
/// 得出相反的结论。
fn trend_of(series: &[u32]) -> Trend {
    if series.len() < MIN_SAMPLES_FOR_TREND {
        return Trend::Unknown;
    }
    let mid = series.len() / 2;
    let mean = |xs: &[u32]| xs.iter().map(|v| *v as f64).sum::<f64>() / xs.len() as f64;
    let older = mean(&series[..mid]);
    let newer = mean(&series[mid..]);

    // 从 0 涨上来没法算比例，单独判。
    if older == 0.0 {
        return if newer > 0.0 { Trend::Rising } else { Trend::Flat };
    }
    let change = (newer - older) / older;
    if change > TREND_THRESHOLD {
        Trend::Rising
    } else if change < -TREND_THRESHOLD {
        Trend::Falling
    } else {
        Trend::Flat
    }
}

// ---------------------------------------------------------------------------
// BEP15 scrape
// ---------------------------------------------------------------------------

/// scrape 回来的一条。字段顺序按 BEP15：seeders, completed, leechers。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrapeEntry {
    pub seeders: u32,
    pub completed: u32,
    pub leechers: u32,
}

/// 拼一个 connect 请求。抽出来是为了能单测。
fn build_connect(transaction_id: u32) -> [u8; 16] {
    let mut buf = [0u8; 16];
    buf[..8].copy_from_slice(&PROTOCOL_ID.to_be_bytes());
    buf[8..12].copy_from_slice(&0u32.to_be_bytes()); // action = connect
    buf[12..].copy_from_slice(&transaction_id.to_be_bytes());
    buf
}

fn parse_connect(resp: &[u8], transaction_id: u32) -> Result<u64> {
    if resp.len() < 16 {
        bail!("connect 应答只有 {} 字节", resp.len());
    }
    let action = u32::from_be_bytes(resp[0..4].try_into().unwrap());
    let tid = u32::from_be_bytes(resp[4..8].try_into().unwrap());
    if action != 0 {
        bail!("connect 应答的 action 是 {action}，不是 0");
    }
    if tid != transaction_id {
        bail!("connect 应答的 transaction id 对不上");
    }
    Ok(u64::from_be_bytes(resp[8..16].try_into().unwrap()))
}

fn build_scrape(connection_id: u64, transaction_id: u32, hashes: &[[u8; 20]]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + hashes.len() * 20);
    buf.extend_from_slice(&connection_id.to_be_bytes());
    buf.extend_from_slice(&2u32.to_be_bytes()); // action = scrape
    buf.extend_from_slice(&transaction_id.to_be_bytes());
    for h in hashes {
        buf.extend_from_slice(h);
    }
    buf
}

fn parse_scrape(resp: &[u8], transaction_id: u32, want: usize) -> Result<Vec<ScrapeEntry>> {
    if resp.len() < 8 {
        bail!("scrape 应答只有 {} 字节", resp.len());
    }
    let action = u32::from_be_bytes(resp[0..4].try_into().unwrap());
    let tid = u32::from_be_bytes(resp[4..8].try_into().unwrap());
    if tid != transaction_id {
        bail!("scrape 应答的 transaction id 对不上");
    }
    if action == 3 {
        let msg = String::from_utf8_lossy(&resp[8..]);
        bail!("tracker 返回错误：{}", msg.trim());
    }
    if action != 2 {
        bail!("scrape 应答的 action 是 {action}，不是 2");
    }

    let body = &resp[8..];
    // tracker 可能只回一部分（它不认识的 info-hash 会被跳过），按实际长度读。
    let n = (body.len() / 12).min(want);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let c = &body[i * 12..i * 12 + 12];
        out.push(ScrapeEntry {
            seeders: u32::from_be_bytes(c[0..4].try_into().unwrap()),
            completed: u32::from_be_bytes(c[4..8].try_into().unwrap()),
            leechers: u32::from_be_bytes(c[8..12].try_into().unwrap()),
        });
    }
    Ok(out)
}

/// `udp://host:port/announce` -> `host:port`。只认 udp，HTTP tracker 的
/// scrape 是另一套协议，这里不做。
fn udp_endpoint(tracker: &str) -> Option<&str> {
    let rest = tracker.strip_prefix("udp://")?;
    let end = rest.find('/').unwrap_or(rest.len());
    let hostport = &rest[..end];
    if hostport.contains(':') {
        Some(hostport)
    } else {
        None
    }
}

/// 对单个 tracker 查一批 info-hash。`pub` 是为了能被集成测试直接打真实
/// tracker —— 单测只能覆盖报文编解码，覆盖不到「对方到底认不认」。
pub async fn scrape_one(tracker: &str, hashes: &[[u8; 20]]) -> Result<Vec<ScrapeEntry>> {
    let endpoint = udp_endpoint(tracker).with_context(|| format!("看不懂的 tracker 地址 {tracker}"))?;
    let addr: SocketAddr = tokio::net::lookup_host(endpoint)
        .await
        .with_context(|| format!("解析 {endpoint} 失败"))?
        .find(|a| a.is_ipv4())
        .with_context(|| format!("{endpoint} 没有 IPv4 地址"))?;

    let sock = UdpSocket::bind("0.0.0.0:0").await.context("绑定 UDP 端口失败")?;
    sock.connect(addr).await.with_context(|| format!("连接 {addr} 失败"))?;

    let tid = rand::random::<u32>();
    sock.send(&build_connect(tid)).await.context("发 connect 失败")?;
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(TRACKER_TIMEOUT, sock.recv(&mut buf))
        .await
        .context("等 connect 应答超时")?
        .context("读 connect 应答失败")?;
    let connection_id = parse_connect(&buf[..n], tid)?;

    let tid = rand::random::<u32>();
    sock.send(&build_scrape(connection_id, tid, hashes))
        .await
        .context("发 scrape 失败")?;
    let n = tokio::time::timeout(TRACKER_TIMEOUT, sock.recv(&mut buf))
        .await
        .context("等 scrape 应答超时")?
        .context("读 scrape 应答失败")?;
    parse_scrape(&buf[..n], tid, hashes.len())
}

// ---------------------------------------------------------------------------
// 采样
// ---------------------------------------------------------------------------

/// 40 个十六进制字符 -> 20 字节。`pub` 同上，给集成测试用。
pub fn parse_info_hash(hex: &str) -> Option<[u8; 20]> {
    if hex.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}


/// 向所有公共 tracker 查一批 info-hash，按 info-hash 合并成
/// `(最大做种, 最大下载, 应答过的 tracker 数)`。
///
/// 取最大值而不是平均：每个 tracker 只知道向**它**汇报过的那部分 peer，
/// 取平均会把没数据的那几个算进去，系统性偏低。
///
/// 单个 tracker 失败绝不放弃整轮 —— 实测 5 个公共 tracker 里从某些网络
/// 只有 2~3 个可达。
///
/// `hashes` 是 `(小写十六进制, 20 字节)` 的列表；返回的键就是前者。
pub async fn scrape_many(hashes: &[(String, [u8; 20])]) -> HashMap<String, (u32, u32, u8)> {
    let mut merged: HashMap<String, (u32, u32, u8)> =
        hashes.iter().map(|(k, _)| (k.clone(), (0, 0, 0))).collect();
    if hashes.is_empty() {
        return merged;
    }

    for tracker in PUBLIC_TRACKERS {
        let mut ok = false;
        for chunk in hashes.chunks(MAX_PER_SCRAPE) {
            let raw: Vec<[u8; 20]> = chunk.iter().map(|(_, h)| *h).collect();
            match scrape_one(tracker, &raw).await {
                Ok(entries) => {
                    ok = true;
                    for ((key, _), e) in chunk.iter().zip(entries) {
                        let slot = merged.get_mut(key).expect("键来自同一份表");
                        slot.0 = slot.0.max(e.seeders);
                        slot.1 = slot.1.max(e.leechers);
                    }
                }
                Err(e) => {
                    // 公共 tracker 连不上是常态，不值得上升到 warn。
                    tracing::debug!("scrape {tracker} 失败：{e:#}");
                }
            }
        }
        if ok {
            for slot in merged.values_mut() {
                slot.2 = slot.2.saturating_add(1);
            }
        }
    }
    merged
}

/// 跑一轮采样，返回记下了几个种子。
///
/// 单个 tracker 挂掉不影响其余 —— 实测公共 tracker 里总有一两个是连不上的，
/// 一个失败就整轮放弃的话基本采不到数据。
pub async fn sample_once(
    engine: &Engine,
    store: &SettingsStore,
    health: &HealthStore,
    stats: Option<&crate::stats::StatsStore>,
) -> usize {
    if !store.get().swarm_health_check {
        return 0;
    }

    let torrents = engine.list();

    let mut hashes = Vec::new();
    let mut keep = HashSet::new();
    // info-hash -> (已下字节, 总字节)，记进样本里给 forecast 用。
    let mut progress: HashMap<String, (u64, u64)> = HashMap::new();
    for t in &torrents {
        progress.insert(
            t.info_hash.to_ascii_lowercase(),
            (t.progress_bytes, t.total_bytes),
        );
        let lower = t.info_hash.to_ascii_lowercase();
        if let Some(h) = parse_info_hash(&lower) {
            hashes.push((lower.clone(), h));
        }
        keep.insert(lower);
    }
    health.retain(&keep);
    if hashes.is_empty() {
        return 0;
    }

    // 手动「现在查一次」也顺带记一笔统计。定时循环那条路在调用这里之前
    // 已经记过了，重复调用是安全的 —— 增量按种子算，第二次算出来是 0。
    if let Some(stats) = stats {
        record_daily(engine, stats);
    }

    let merged = scrape_many(&hashes).await;

    let ts = now_secs();
    for (key, (seeders, leechers, trackers_ok)) in &merged {
        health.record(
            key,
            Sample {
                ts,
                seeders: *seeders,
                leechers: *leechers,
                trackers_ok: *trackers_ok,
                progress: progress.get(key).map(|(p, _)| *p),
                total: progress.get(key).map(|(_, t)| *t),
            },
        );
    }
    health.save();

    let responded = merged.values().filter(|v| v.2 > 0).count();
    tracing::info!(
        种子数 = merged.len(),
        采到数据 = responded,
        "swarm 健康度采样完成"
    );
    merged.len()
}

/// 把这一轮的进度/上传量交给每日统计。
///
/// 和 swarm 采样分开：**统计不该受「记录 swarm 健康度」那个开关影响** ——
/// 那个开关管的是「要不要把 info-hash 发给公共 tracker」，是隐私问题；
/// 而每日统计纯本地，一个包都不发。
pub fn record_daily(engine: &Engine, stats: &crate::stats::StatsStore) {
    // 用会话级真实传输计数器，不能用任务 progress_bytes：后者在启动校验期间
    // 会先回退再恢复，把磁盘上早已存在的文件误算成新下载。
    let current = engine.session_transfer_totals();
    let (down, up) = stats.record(current, now_secs());
    if down > 0 || up > 0 {
        tracing::debug!(新增下载 = down, 新增上传 = up, "每日统计");
    }
}

pub fn spawn(
    engine: Arc<Engine>,
    store: Arc<SettingsStore>,
    health: Arc<HealthStore>,
    stats: Arc<crate::stats::StatsStore>,
) {
    tauri::async_runtime::spawn(async move {
        // 先采一轮再进循环，而不是上来就睡半小时 —— 否则新装的用户展开任务
        // 只会看到「还没采到数据」，得等到下一个整点才有东西看。
        // 但也不能立刻采：会话刚起来时任务可能还在 initializing，而且没必要
        // 跟启动时那一堆 DHT/tracker 流量挤在一起。
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            // 统计先记，且无条件记 —— swarm 采样可能被开关关掉，
            // 但每日统计是纯本地的，不该跟着一起停。
            record_daily(&engine, &stats);
            sample_once(&engine, &store, &health, Some(&stats)).await;
            // 每轮重读间隔，改了设置不用重启。下限 10 分钟：scrape 很便宜，
            // 但没必要比 tracker 自己给的 announce interval 还勤。
            let minutes = store.get().swarm_health_interval_minutes.max(10);
            tokio::time::sleep(Duration::from_secs(minutes * 60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: i64, seeders: u32, leechers: u32) -> Sample {
        Sample {
            ts,
            seeders,
            leechers,
            trackers_ok: 3,
            ..Default::default()
        }
    }

    // ---- BEP15 报文 ----

    #[test]
    fn connect_roundtrip() {
        let req = build_connect(0xdead_beef);
        assert_eq!(u64::from_be_bytes(req[..8].try_into().unwrap()), PROTOCOL_ID);
        assert_eq!(u32::from_be_bytes(req[8..12].try_into().unwrap()), 0);

        let mut resp = [0u8; 16];
        resp[0..4].copy_from_slice(&0u32.to_be_bytes());
        resp[4..8].copy_from_slice(&0xdead_beefu32.to_be_bytes());
        resp[8..16].copy_from_slice(&0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(parse_connect(&resp, 0xdead_beef).unwrap(), 0x0102_0304_0506_0708);
    }

    /// transaction id 是防串包的，对不上必须拒绝 —— UDP 上什么都可能收到。
    #[test]
    fn connect_rejects_wrong_transaction_id() {
        let mut resp = [0u8; 16];
        resp[4..8].copy_from_slice(&1u32.to_be_bytes());
        assert!(parse_connect(&resp, 2).is_err());
    }

    #[test]
    fn scrape_request_layout() {
        let h = [[7u8; 20], [9u8; 20]];
        let req = build_scrape(0x1122_3344_5566_7788, 42, &h);
        // 16 字节头 + 每个 hash 20 字节
        assert_eq!(req.len(), 16 + 40);
        assert_eq!(u32::from_be_bytes(req[8..12].try_into().unwrap()), 2);
        assert_eq!(&req[16..36], &[7u8; 20]);
        assert_eq!(&req[36..56], &[9u8; 20]);
    }

    /// 字段顺序错了会让「做种」和「完成过」对调，是最容易犯又最难发现的错。
    #[test]
    fn scrape_response_field_order_is_seeders_completed_leechers() {
        let mut resp = Vec::new();
        resp.extend_from_slice(&2u32.to_be_bytes());
        resp.extend_from_slice(&77u32.to_be_bytes());
        resp.extend_from_slice(&11u32.to_be_bytes()); // seeders
        resp.extend_from_slice(&22u32.to_be_bytes()); // completed
        resp.extend_from_slice(&33u32.to_be_bytes()); // leechers

        let got = parse_scrape(&resp, 77, 1).unwrap();
        assert_eq!(
            got[0],
            ScrapeEntry {
                seeders: 11,
                completed: 22,
                leechers: 33
            }
        );
    }

    #[test]
    fn scrape_surfaces_tracker_error() {
        let mut resp = Vec::new();
        resp.extend_from_slice(&3u32.to_be_bytes());
        resp.extend_from_slice(&5u32.to_be_bytes());
        resp.extend_from_slice(b"torrent not found");
        let err = parse_scrape(&resp, 5, 1).unwrap_err().to_string();
        assert!(err.contains("torrent not found"), "实际：{err}");
    }

    /// tracker 只认识一部分 hash 时会少回几条，不能越界读。
    #[test]
    fn scrape_tolerates_short_response() {
        let mut resp = Vec::new();
        resp.extend_from_slice(&2u32.to_be_bytes());
        resp.extend_from_slice(&1u32.to_be_bytes());
        resp.extend_from_slice(&1u32.to_be_bytes());
        resp.extend_from_slice(&2u32.to_be_bytes());
        resp.extend_from_slice(&3u32.to_be_bytes());
        assert_eq!(parse_scrape(&resp, 1, 5).unwrap().len(), 1);
    }

    // ---- 地址解析 ----

    #[test]
    fn parses_udp_tracker_urls() {
        assert_eq!(
            udp_endpoint("udp://tracker.opentrackr.org:1337/announce"),
            Some("tracker.opentrackr.org:1337")
        );
        assert_eq!(udp_endpoint("udp://open.stealth.si:80"), Some("open.stealth.si:80"));
        // HTTP tracker 的 scrape 是另一套协议，这里必须拒绝而不是猜。
        assert_eq!(udp_endpoint("http://tracker.example.com/announce"), None);
        // 没端口就没法连，也不该瞎补默认值。
        assert_eq!(udp_endpoint("udp://tracker.example.com/announce"), None);
    }

    #[test]
    fn parses_info_hash_hex() {
        let h = parse_info_hash("a57b7f3548bcab81116f5bd282dce952b4215853").unwrap();
        assert_eq!(h[0], 0xa5);
        assert_eq!(h[19], 0x53);
        assert!(parse_info_hash("abc").is_none());
        assert!(parse_info_hash("zz7b7f3548bcab81116f5bd282dce952b4215853").is_none());
    }

    // ---- 判定 ----

    #[test]
    fn no_samples_is_unknown() {
        let v = verdict(&[]);
        assert_eq!(v.status, Status::Unknown);
        assert_eq!(v.samples, 0);
    }

    /// 一次断网会让所有 tracker 都不应答，得到 0 做种。要是把它当成真数据，
    /// 一断网所有任务都会被判死。
    #[test]
    fn untrustworthy_samples_are_ignored() {
        let history = vec![
            s(1, 5, 10),
            Sample {
                ts: 2,
                seeders: 0,
                leechers: 0,
                trackers_ok: 0,
                ..Default::default()
            },
        ];
        let v = verdict(&history);
        assert_eq!(v.samples, 1, "trackers_ok=0 的样本不该算数");
        assert_eq!(v.latest.unwrap().seeders, 5);
        assert_ne!(v.status, Status::Dead);
    }

    #[test]
    fn one_zero_sample_is_not_enough_to_call_it_dead() {
        let v = verdict(&[s(1, 3, 4), s(2, 0, 4)]);
        assert_eq!(v.status, Status::Unknown, "只有一轮 0 做种还不能下结论");
    }

    #[test]
    fn sustained_zero_seeders_is_dead() {
        let v = verdict(&[s(1, 2, 4), s(2, 0, 4), s(3, 0, 4), s(4, 0, 3)]);
        assert_eq!(v.status, Status::Dead);
        assert!(v.summary.contains("没人做"), "实际：{}", v.summary);
    }

    /// 今天碟中谍8 的真实数字：2 个做种对 16 个下载。
    #[test]
    fn few_seeders_many_leechers_is_starving() {
        let v = verdict(&[s(1, 2, 16)]);
        assert_eq!(v.status, Status::Starving);
        assert!(v.summary.contains("慢是正常的"), "实际：{}", v.summary);
    }

    #[test]
    fn healthy_swarm_is_ok() {
        let v = verdict(&[s(1, 40, 8)]);
        assert_eq!(v.status, Status::Ok);
    }

    #[test]
    fn trend_needs_enough_samples() {
        assert_eq!(trend_of(&[1, 9]), Trend::Unknown);
        assert_eq!(trend_of(&[1, 1, 9, 9]), Trend::Rising);
    }

    #[test]
    fn trend_detects_decline_and_stability() {
        assert_eq!(trend_of(&[10, 10, 2, 2]), Trend::Falling);
        assert_eq!(trend_of(&[10, 11, 10, 11]), Trend::Flat);
    }

    /// 从 0 涨上来算不了比例，不能除零。
    #[test]
    fn trend_handles_zero_baseline() {
        assert_eq!(trend_of(&[0, 0, 3, 4]), Trend::Rising);
        assert_eq!(trend_of(&[0, 0, 0, 0]), Trend::Flat);
    }

    // ---- 存储 ----

    #[test]
    fn store_caps_history_and_survives_reload() {
        let path = std::env::temp_dir().join(format!("mydl-health-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = HealthStore::load(path.clone());
        for i in 0..(MAX_SAMPLES + 10) {
            store.record("AABB", s(i as i64, 1, 1));
        }
        store.save();

        let reloaded = HealthStore::load(path.clone());
        // 大小写不该影响命中。
        let h = reloaded.history("aabb");
        assert_eq!(h.len(), MAX_SAMPLES, "应该按上限截断");
        assert_eq!(h[0].ts, 10, "截掉的该是最老的");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn store_forgets_deleted_torrents() {
        let path = std::env::temp_dir().join(format!("mydl-health-r-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = HealthStore::load(path.clone());
        store.record("aa", s(1, 1, 1));
        store.record("bb", s(1, 1, 1));
        store.retain(&HashSet::from(["aa".to_string()]));

        assert_eq!(store.history("aa").len(), 1);
        assert!(store.history("bb").is_empty(), "删掉的任务该被清理");

        let _ = std::fs::remove_file(&path);
    }
}
