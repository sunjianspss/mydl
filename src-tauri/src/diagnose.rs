//! 「为什么不动？」—— 任务卡住时的自动取证。
//!
//! # 为什么需要它
//!
//! 一个任务不动，原因至少有六种：绑的网卡没了、BT 在走 VPN 隧道、swarm 真的
//! 没源、源太少、握手被对方拒、任务其实是暂停的。**它们的表现一模一样**：
//! 进度条不动、0 peers。用户唯一能做的就是猜。
//!
//! 这套阶梯是手工跑通过的 —— 作者机器上那次「任务挂三天」，靠它才查出真因是
//! **全部流量在走 VPN 隧道**（45 秒 0.5 MB vs 绑物理网卡 588 MB）。整个过程
//! 花了两天。有这个按钮的话是几十秒。
//!
//! # 阶梯顺序：先便宜的，先能一票否决的
//!
//! 1. 任务自己是不是暂停/出错的 —— 不查这个就会把「你按了暂停」诊断成「没源」
//! 2. 绑定的网卡还在不在 —— 拔了网卡 BT 会静默停摆
//! 3. BT 实际走哪条路 —— 走隧道的话后面全都不用看了，这就是答案
//! 4. tracker 上有没有源 —— 0 做种就是真没了，跟你的网络无关
//! 5. 能不能和这些 peer 握上手
//! 6. **对照组**：同样的握手打一个必然健康的 swarm
//!
//! 第 6 步是整套东西的关键。只测自己那个 swarm 永远分不清「这个资源不行」和
//! 「你的网络不行」—— 而这两个结论指向完全相反的行动。

use std::net::SocketAddr;
use std::time::Duration;

use serde::Serialize;

use crate::health;
use crate::netif;
use crate::settings::PUBLIC_TRACKERS;

/// BEP15 握手魔数。
const PROTOCOL_ID: u64 = 0x0417_2710_1980;

/// BEP3 的协议标识。握手包第 2~20 字节。
const PSTR: &[u8; 19] = b"BitTorrent protocol";

/// 单个网络操作的上限。诊断是交互动作，总时长要可控。
const IO_TIMEOUT: Duration = Duration::from_secs(6);

/// 最多探几个 peer。样本够judge成功率就行，不必打满。
const PROBE_PEERS: usize = 16;

/// 对照用的 swarm：Ubuntu 24.04.4 官方种子。
///
/// 选它是因为**由官方服务器做种、常年几十个 seeder、不会屏蔽任何人**。
/// 如果连它都握不上手，问题一定在本地网络而不是你要下的那个资源。
///
/// 哪天这个版本下架了，`control` 那一步会变成「跳过」而不是给出错误结论 ——
/// 见 [`Step::Skipped`]。到时候换个更新的官方种子即可。
const CONTROL_INFO_HASH: &str = "62a4d9e139f3315f8716bcccca0cc984a9809da1";

// ---------------------------------------------------------------------------
// 结果
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// 这一环没问题。
    Ok,
    /// 有问题但不致命。
    Warn,
    /// 这一环就是病灶。
    Bad,
    /// 没查成（前面已经有结论，或者依赖的数据拿不到）。
    Skipped,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// 查的是什么。
    pub name: String,
    pub outcome: Outcome,
    /// **实测到的数字**，不是解释。解释放在 verdict 里。
    pub detail: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub steps: Vec<Step>,
    /// 一句话结论。
    pub verdict: String,
    /// 该怎么办。没有明确建议时为 None —— 不硬凑。
    pub advice: Option<String>,
}

fn step(name: &str, outcome: Outcome, detail: impl Into<String>) -> Step {
    Step {
        name: name.into(),
        outcome,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// 探测：这条路由通向哪张网卡
// ---------------------------------------------------------------------------

/// 内核会用哪张网卡出去。
///
/// 不去 shell 出来解析 `netstat -rn`（macOS 和 Linux 格式不同，还得处理多条
/// default 路由）。改用一个标准技巧：UDP socket `connect()` 到一个公网地址
/// —— **不发任何数据包**，只是让内核做一次选路决策，然后从 `local_addr()`
/// 把它选的源地址读出来，再拿这个 IP 去 `netif::list()` 里反查网卡名。
///
/// 这样拿到的是「内核真正会用的那条路」，比任何猜测都准。
pub fn default_route_interface() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // 1.1.1.1 只是个选路目标，不会真的联系它。
    sock.connect("1.1.1.1:80").ok()?;
    let local = sock.local_addr().ok()?.ip().to_string();
    netif::list()
        .into_iter()
        .find(|i| i.ipv4.as_deref() == Some(local.as_str()))
        .map(|i| i.name)
}

// ---------------------------------------------------------------------------
// 探测：向 tracker 要 peer 列表
// ---------------------------------------------------------------------------

/// BEP15 announce，拿真实的 peer 地址列表。
///
/// 和 `health::scrape_many` 的区别：scrape 只给**人数**，announce 给**地址**。
/// 要测握手就必须有地址。
async fn announce(tracker: &str, info_hash: [u8; 20]) -> anyhow::Result<Vec<SocketAddr>> {
    use anyhow::Context;
    use tokio::net::UdpSocket;

    let endpoint = tracker
        .strip_prefix("udp://")
        .map(|r| &r[..r.find('/').unwrap_or(r.len())])
        .filter(|e| e.contains(':'))
        .context("不是 udp:// tracker")?;

    let addr: SocketAddr = tokio::net::lookup_host(endpoint)
        .await?
        .find(|a| a.is_ipv4())
        .context("没有 IPv4 地址")?;

    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    sock.connect(addr).await?;

    // connect
    let tid = rand::random::<u32>();
    let mut req = Vec::with_capacity(16);
    req.extend_from_slice(&PROTOCOL_ID.to_be_bytes());
    req.extend_from_slice(&0u32.to_be_bytes());
    req.extend_from_slice(&tid.to_be_bytes());
    sock.send(&req).await?;

    let mut buf = [0u8; 4096];
    let n = tokio::time::timeout(IO_TIMEOUT, sock.recv(&mut buf)).await??;
    if n < 16 || u32::from_be_bytes(buf[0..4].try_into().unwrap()) != 0 {
        anyhow::bail!("connect 应答不对");
    }
    let conn = u64::from_be_bytes(buf[8..16].try_into().unwrap());

    // announce
    let tid = rand::random::<u32>();
    let mut req = Vec::with_capacity(98);
    req.extend_from_slice(&conn.to_be_bytes());
    req.extend_from_slice(&1u32.to_be_bytes()); // action = announce
    req.extend_from_slice(&tid.to_be_bytes());
    req.extend_from_slice(&info_hash);
    // peer id 随便造一个，但要长得像正常客户端 —— 有些 tracker 会挑剔。
    req.extend_from_slice(b"-qB4650-");
    req.extend_from_slice(&rand::random::<[u8; 12]>());
    req.extend_from_slice(&0u64.to_be_bytes()); // downloaded
    req.extend_from_slice(&(1u64 << 30).to_be_bytes()); // left，非 0 才被当成下载者
    req.extend_from_slice(&0u64.to_be_bytes()); // uploaded
    req.extend_from_slice(&2u32.to_be_bytes()); // event = started
    req.extend_from_slice(&0u32.to_be_bytes()); // ip
    req.extend_from_slice(&rand::random::<u32>().to_be_bytes()); // key
    req.extend_from_slice(&(-1i32).to_be_bytes()); // num_want
    req.extend_from_slice(&6881u16.to_be_bytes());
    sock.send(&req).await?;

    let n = tokio::time::timeout(IO_TIMEOUT, sock.recv(&mut buf)).await??;
    if n < 20 || u32::from_be_bytes(buf[0..4].try_into().unwrap()) != 1 {
        anyhow::bail!("announce 应答不对");
    }

    Ok(parse_peers(&buf[20..n]))
}

/// announce 应答尾部是紧凑格式的 peer 列表：每条 4 字节 IP + 2 字节端口。
fn parse_peers(body: &[u8]) -> Vec<SocketAddr> {
    body.chunks_exact(6)
        .map(|c| {
            let ip = std::net::Ipv4Addr::new(c[0], c[1], c[2], c[3]);
            let port = u16::from_be_bytes([c[4], c[5]]);
            SocketAddr::from((ip, port))
        })
        // 端口 0 是占位，连不上。
        .filter(|a| a.port() != 0)
        .collect()
}

// ---------------------------------------------------------------------------
// 探测：握手
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handshake {
    Ok,
    /// TCP 都没连上。
    NoTcp,
    /// 连上了，但对方不回应或直接断开 —— 典型的「被拒」。
    Rejected,
}

/// 对一个 peer 走一遍 BEP3 明文握手。
///
/// 只是握手，不交换任何数据。目的是回答「对方愿不愿意跟我们说话」。
async fn handshake(peer: SocketAddr, info_hash: [u8; 20]) -> Handshake {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let Ok(Ok(mut stream)) =
        tokio::time::timeout(IO_TIMEOUT, tokio::net::TcpStream::connect(peer)).await
    else {
        return Handshake::NoTcp;
    };

    let mut req = Vec::with_capacity(68);
    req.push(19);
    req.extend_from_slice(PSTR);
    req.extend_from_slice(&(1u64 << 20).to_be_bytes()); // 扩展协议位，和 librqbit 一致
    req.extend_from_slice(&info_hash);
    req.extend_from_slice(b"-qB4650-");
    req.extend_from_slice(&rand::random::<[u8; 12]>());

    if tokio::time::timeout(IO_TIMEOUT, stream.write_all(&req))
        .await
        .map(|r| r.is_err())
        .unwrap_or(true)
    {
        return Handshake::Rejected;
    }

    let mut resp = [0u8; 68];
    match tokio::time::timeout(IO_TIMEOUT, stream.read_exact(&mut resp)).await {
        Ok(Ok(_)) if resp[1..20] == PSTR[..] => Handshake::Ok,
        _ => Handshake::Rejected,
    }
}

/// 对一批 peer 并发握手，返回 (成功, 连不上, 被拒)。
async fn probe_peers(peers: &[SocketAddr], info_hash: [u8; 20]) -> (usize, usize, usize) {
    // 用 tokio 自带的 JoinSet 而不是 futures::join_all —— futures 目前只是
    // dev-dependency，为一个并发原语把它提成正式依赖不值得。
    let mut set = tokio::task::JoinSet::new();
    for p in peers.iter().take(PROBE_PEERS) {
        let peer = *p;
        set.spawn(async move { handshake(peer, info_hash).await });
    }

    let mut counts = (0, 0, 0);
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Handshake::Ok) => counts.0 += 1,
            Ok(Handshake::NoTcp) => counts.1 += 1,
            // 任务自己 panic 也算「没连上」，不能因此丢掉整轮统计。
            Ok(Handshake::Rejected) | Err(_) => counts.2 += 1,
        }
    }
    counts
}

/// 从若干 tracker 凑一批 peer 地址。单个失败不影响其余。
async fn gather_peers(info_hash: [u8; 20]) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for tracker in PUBLIC_TRACKERS {
        if out.len() >= PROBE_PEERS {
            break;
        }
        match announce(tracker, info_hash).await {
            Ok(peers) => {
                for p in peers {
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
            Err(e) => tracing::debug!("announce {tracker} 失败：{e:#}"),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 判读
// ---------------------------------------------------------------------------

/// 判读需要的全部事实。抽出来是为了让结论逻辑是**纯函数**、能测 ——
/// 这套规则出错的代价是把人指去改完全无关的地方。
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// 任务自己的状态：paused / error / live 之类。
    pub torrent_state: String,
    pub torrent_error: Option<String>,
    pub finished: bool,
    /// 引擎当前真正维持着的 peer 数和瞬时下载速度。
    ///
    /// 单独探测的一批公开 peer 可能全都失联，但任务仍通过 DHT/PEX 连着别的
    /// peer。没有这两个事实，报告会把「只剩一个慢 peer」说成「完全连不上」。
    pub peers_live: usize,
    pub download_speed_bps: f64,
    /// mydl 自己有没有配置 SOCKS5。Windows 的系统 HTTP 代理不在这里；
    /// 原始 BT socket 不会读取它，只有 TUN/WinDivert 透明代理可能在外部接管。
    pub proxy_configured: bool,
    /// BT 绑定的网卡；None = 跟随系统路由。
    pub bind_device: Option<String>,
    /// 绑定的网卡还在不在。
    pub bind_device_up: bool,
    /// 内核默认路由用的网卡。
    pub route_interface: Option<String>,
    /// 默认路由那张网卡是不是隧道。
    pub route_is_tunnel: bool,
    /// tracker 报的做种 / 下载人数；None = 一个 tracker 都没应答。
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    /// 目标 swarm 的握手结果 (成功, 连不上, 被拒)。
    pub probe: Option<(usize, usize, usize)>,
    /// 对照组的握手结果。None = 对照组本身没跑成，不能拿来比。
    pub control: Option<(usize, usize, usize)>,
}

fn rate(counts: (usize, usize, usize)) -> Option<f64> {
    let total = counts.0 + counts.1 + counts.2;
    (total > 0).then(|| counts.0 as f64 / total as f64)
}

fn format_speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MiB/s", bps / 1024.0 / 1024.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KiB/s", bps / 1024.0)
    } else {
        format!("{bps:.0} B/s")
    }
}

fn live_activity(f: &Facts) -> Option<String> {
    match (f.peers_live, f.download_speed_bps > 0.5) {
        (0, false) => None,
        (peers, true) => Some(format!(
            "任务当前仍连着 {peers} 个 peer、正在以 {} 下载",
            format_speed(f.download_speed_bps)
        )),
        (peers, false) => Some(format!("任务当前仍连着 {peers} 个 peer")),
    }
}

fn tunnel_advice() -> String {
    if cfg!(windows) {
        "Windows 版不能在 App 内绑定网卡。如果代理客户端开了 TUN / 透明代理，\
         给 mydl.exe 配一条 DIRECT（直连）规则后重启 App；仅开启 Windows 系统代理不影响 BT。"
            .into()
    } else {
        "去设置里把「BT 走哪张网卡」改成物理网卡（比如 en0），\
         重启 App。这不影响 VPN 本身，浏览器照旧走隧道。"
            .into()
    }
}

fn network_side_advice(f: &Facts) -> String {
    // 连对照组都握不上手，而 mydl 自己配了 SOCKS5 —— 那代理就是头号嫌疑：
    // BT 的出站 TCP 全走它，代理不通或被限速的表现和「网络整体不好」一模一样。
    // 先说这条，因为它是用户自己一步能验证掉的。
    if f.proxy_configured {
        return "你在设置里配了 SOCKS5 代理，BT 的出站 TCP 全部走它 —— \
                代理不通或者被限速，表现就是这样。先清空「SOCKS5 代理」重启 App \
                再跑一次诊断：还是握不上手才轮到网络本身。"
            .into();
    }
    if cfg!(windows) {
        "常见原因：VPN 的 TUN / 透明代理接管了 BT，或者被运营商干扰。\
         在代理客户端给 mydl.exe 配一条 DIRECT（直连）规则；仅开启 Windows 系统代理不影响 BT。"
            .into()
    } else {
        "常见原因：BT 流量在走 VPN / 代理，或者被运营商干扰。\
         先确认「BT 走哪张网卡」绑的是物理网卡。"
            .into()
    }
}

/// 从事实推出结论。**纯函数**。
///
/// 顺序就是阶梯顺序：能一票否决的先说。每一条都只在**有实测数据支撑**时才
/// 下结论，数据缺失一律退回「查不出来」而不是猜。
pub fn conclude(f: &Facts) -> (String, Option<String>) {
    // 1. 任务自己就是停的。不先查这个会把「你按了暂停」诊断成「没源」。
    if f.torrent_state == "paused" {
        return (
            "这个任务是暂停状态，不是连不上。".into(),
            Some("点行内的继续按钮，或者工具栏的「全部继续」。".into()),
        );
    }
    if let Some(e) = &f.torrent_error {
        return (format!("任务本身报错了：{e}"), None);
    }
    if f.finished {
        return (
            "这个任务已经下完了，现在在做种 —— 没有「不动」这回事。".into(),
            None,
        );
    }

    // 2. 绑的网卡没了。绑定是建会话时定死的，不会自动切换。
    if let Some(dev) = &f.bind_device {
        if !f.bind_device_up {
            return (
                format!("BT 绑在网卡 {dev} 上，而这张网卡已经不在了 —— 所有连接都发不出去。"),
                Some("重启 App 会自动重选一张，或者去设置里改成别的网卡。".into()),
            );
        }
    }

    // 3. BT 在走隧道。这是最容易被误判成「没源」的一种，而且影响最大。
    if f.bind_device.is_none() && f.route_is_tunnel {
        let dev = f.route_interface.as_deref().unwrap_or("隧道");
        return (
            format!(
                "BT 流量正在走 VPN / 代理隧道（{dev}）。隧道出口通常是机房 IP，\
                 会被大量 BT 客户端屏蔽；而且 UPnP 出不了隧道，没有入站连接，做种也是无效的。"
            ),
            Some(tunnel_advice()),
        );
    }

    // 4. swarm 真的没源。
    match f.seeders {
        None => {
            return (
                "一个 tracker 都没应答，查不到这个 swarm 有没有源。".into(),
                Some("可能是网络不通，也可能这几个公共 tracker 都挂了。稍后再试。".into()),
            );
        }
        Some(0) => {
            return (
                format!(
                    "tracker 上 0 个做种（{} 个人在下）—— 这个资源已经没人做种了。",
                    f.leechers.unwrap_or(0)
                ),
                Some("等下去不会有结果，用「找更好的源」换一个。".into()),
            );
        }
        _ => {}
    }

    let seeders = f.seeders.unwrap_or(0);
    let leechers = f.leechers.unwrap_or(0);

    // 5. 握手。有源但握不上手，要靠对照组判断是谁的问题。
    let Some(probe) = f.probe else {
        return (
            format!("tracker 说有 {seeders} 个做种，但没拿到可测的 peer 地址。"),
            None,
        );
    };
    let Some(probe_rate) = rate(probe) else {
        return (
            format!("tracker 说有 {seeders} 个做种，但一个 peer 地址都没拿到。"),
            None,
        );
    };

    // 握手基本正常 -> 不是连接问题，是 swarm 太瘦或者对方不给带宽。
    if probe_rate >= 0.3 {
        if leechers >= seeders.saturating_mul(5) {
            return (
                format!(
                    "连得上（{}/{} 个 peer 握手成功），但 {seeders} 个做种要分给 \
                     {leechers} 个下载的 —— 慢是正常的，不是故障。",
                    probe.0,
                    probe.0 + probe.1 + probe.2
                ),
                Some("耐心等，或者用「找更好的源」找个做种多的。".into()),
            );
        }
        return (
            format!(
                "连得上（{}/{} 个 peer 握手成功），网络这一层没问题。\
                 慢的话多半是对方不给带宽。",
                probe.0,
                probe.0 + probe.1 + probe.2
            ),
            None,
        );
    }

    // 握手成功率很低。对照组是唯一能分清「这个资源」和「你的网络」的东西。
    //
    // 但**不能只看对照组过没过阈值**：目标 25%、对照组 37% 这种差距在噪声
    // 范围内，据此断言「不是你的网络」是过度自信。要求对照组明显更好才敢
    // 归咎于这个资源，否则老实说「两边都不理想」。
    let probe_pct = probe_rate * 100.0;
    match f.control.and_then(rate) {
        Some(control_rate) if control_rate >= 0.3 && control_rate >= probe_rate * 2.0 => {
            let measured = format!(
                "公开样本 {}/{} 握手成功，而对照组（Ubuntu 官方种子）{:.0}% 正常。",
                probe.0,
                probe.0 + probe.1 + probe.2,
                control_rate * 100.0
            );
            let verdict = match live_activity(f) {
                Some(live) => format!(
                    "{live}；但这个 swarm 的{measured}可用 peer 太少，所以很慢，不是你的网络或 VPN。"
                ),
                None => format!(
                    "只有这个 swarm 连不上：{measured}问题出在这个资源的 peer 上，不是你的网络或 VPN。"
                ),
            };
            (
                verdict,
                Some("这些公开 peer 多半已经离线或在拒绝连接。本地设置修不好，换个源更快。".into()),
            )
        }
        // 对照组也不好 —— 本地网络的问题。
        Some(control_rate) if control_rate < 0.3 => (
            format!(
                "连对照组（Ubuntu 官方种子）都握不上手（{:.0}%），\
                 问题在你的网络，不在这个资源。",
                control_rate * 100.0
            ),
            Some(network_side_advice(f)),
        ),
        // 对照组过了阈值，但没有明显好过目标 —— 差距在噪声里，不下结论。
        Some(control_rate) => (
            format!(
                "握手成功率偏低（这个 swarm {probe_pct:.0}%，对照组 {:.0}%），\
                 但两边差不多 —— 分不清是这个资源的 peer 在拒绝你，还是你的网络整体不好。",
                control_rate * 100.0
            ),
            Some("多试几次；如果每次对照组都不高，那更可能是网络这一侧。".into()),
        ),
        // 对照组没跑成就别下这个结论 —— 它是整套判断的支点。
        None => (
            format!(
                "和 peer 握手基本都失败（{}/{} 成功），但对照组没跑起来，\
                 分不清是这个资源的问题还是你的网络。",
                probe.0,
                probe.0 + probe.1 + probe.2
            ),
            Some("稍后重跑一次诊断。".into()),
        ),
    }
}

// ---------------------------------------------------------------------------
// 串起来
// ---------------------------------------------------------------------------

/// 被诊断的那个任务，调用方从引擎里一次取好传进来。
///
/// 攒成结构体而不是排成一串参数：这些字段一半是 `bool`、一半是
/// `Option<String>`，位置写反了编译器一声不吭，而诊断结论会整个跑偏。
#[derive(Debug, Clone, Default)]
pub struct Subject {
    /// 任务自己的状态：paused / error / live 之类。
    pub state: String,
    pub error: Option<String>,
    pub finished: bool,
    /// BT 绑定的网卡；None = 跟随系统路由。
    pub bind_device: Option<String>,
    /// 设置里配了 SOCKS5（空串不算）。
    pub proxy_configured: bool,
    /// 引擎当前真正维持着的 peer 数和瞬时下载速度。
    pub peers_live: usize,
    pub download_speed_bps: f64,
}

/// 跑一次完整诊断。
pub async fn run(info_hash: &str, subject: Subject) -> Report {
    let Subject {
        state: torrent_state,
        error: torrent_error,
        finished,
        bind_device,
        proxy_configured,
        peers_live,
        download_speed_bps,
    } = subject;

    let mut steps = Vec::new();
    let mut f = Facts {
        torrent_state: torrent_state.clone(),
        torrent_error: torrent_error.clone(),
        finished,
        peers_live,
        download_speed_bps,
        proxy_configured,
        bind_device: bind_device.clone(),
        bind_device_up: true,
        ..Default::default()
    };

    // 1. 任务状态
    steps.push(match (&torrent_error, torrent_state.as_str(), finished) {
        (Some(e), _, _) => step("任务状态", Outcome::Bad, format!("出错：{e}")),
        (_, "paused", _) => step("任务状态", Outcome::Bad, "已暂停"),
        (_, _, true) => step("任务状态", Outcome::Ok, "已完成，在做种"),
        (_, s, _) => step("任务状态", Outcome::Ok, s.to_string()),
    });

    // 引擎自己的实时连接比临时探测更接近「下载到底有没有动」。探测样本全挂
    // 不代表任务一个 peer 都没有：它还可能通过 DHT/PEX 找到别的地址。
    steps.push(if download_speed_bps > 0.5 {
        step(
            "当前连接",
            Outcome::Ok,
            format!(
                "{peers_live} 个实时 peer · {}",
                format_speed(download_speed_bps)
            ),
        )
    } else if peers_live > 0 {
        step(
            "当前连接",
            Outcome::Warn,
            format!("{peers_live} 个实时 peer · 当前瞬时速度为 0"),
        )
    } else {
        step("当前连接", Outcome::Warn, "0 个实时 peer")
    });

    // 2. 绑定的网卡
    match &bind_device {
        Some(dev) => {
            f.bind_device_up = netif::exists(dev);
            steps.push(if f.bind_device_up {
                step("BT 绑定的网卡", Outcome::Ok, format!("{dev}，在线"))
            } else {
                step("BT 绑定的网卡", Outcome::Bad, format!("{dev} 已经不存在"))
            });
        }
        None => steps.push(step("BT 绑定的网卡", Outcome::Warn, "没绑定，跟随系统路由")),
    }

    steps.push(if proxy_configured {
        step(
            "BT 代理设置",
            Outcome::Warn,
            "已配置 SOCKS5：出站 TCP 走代理，DHT / uTP / UDP tracker 仍直连",
        )
    } else if cfg!(windows) {
        step(
            "BT 代理设置",
            Outcome::Ok,
            "直连；不使用 Windows 系统代理（TUN / 透明代理除外）",
        )
    } else {
        step("BT 代理设置", Outcome::Ok, "未配置 SOCKS5")
    });

    // 3. 默认路由
    f.route_interface = default_route_interface();
    f.route_is_tunnel = f
        .route_interface
        .as_deref()
        .map(netif::is_tunnel_name)
        .unwrap_or(false);
    steps.push(
        match (&f.route_interface, f.route_is_tunnel, &bind_device) {
            (Some(dev), true, None) => step(
                "系统默认路由",
                Outcome::Bad,
                format!("{dev}（隧道）—— BT 正在走它"),
            ),
            (Some(dev), true, Some(bound)) => step(
                "系统默认路由",
                Outcome::Ok,
                format!("{dev}（隧道），但 BT 已绑到 {bound}，不受影响"),
            ),
            (Some(dev), false, _) => step("系统默认路由", Outcome::Ok, dev.clone()),
            (None, _, _) => step("系统默认路由", Outcome::Skipped, "查不出来"),
        },
    );

    // 已经能一票定案的就别再打扰网络了。
    let decided = steps.iter().any(|s| s.outcome == Outcome::Bad);

    let Some(raw_hash) = health::parse_info_hash(&info_hash.to_ascii_lowercase()) else {
        steps.push(step(
            "swarm 有没有源",
            Outcome::Skipped,
            "info-hash 认不出来",
        ));
        let (verdict, advice) = conclude(&f);
        return Report {
            steps,
            verdict,
            advice,
        };
    };

    // 4. tracker 上有没有源
    if decided {
        steps.push(step(
            "swarm 有没有源",
            Outcome::Skipped,
            "前面已经定位到问题",
        ));
        steps.push(step("和 peer 握手", Outcome::Skipped, "同上"));
        steps.push(step("对照组", Outcome::Skipped, "同上"));
        let (verdict, advice) = conclude(&f);
        return Report {
            steps,
            verdict,
            advice,
        };
    }

    let scraped = health::scrape_many(&[(info_hash.to_ascii_lowercase(), raw_hash)]).await;
    if let Some((s, l, ok)) = scraped.get(&info_hash.to_ascii_lowercase()) {
        if *ok > 0 {
            f.seeders = Some(*s);
            f.leechers = Some(*l);
        }
    }
    steps.push(match f.seeders {
        None => step("swarm 有没有源", Outcome::Skipped, "没有 tracker 应答"),
        Some(0) => step(
            "swarm 有没有源",
            Outcome::Bad,
            format!("0 个做种 · {} 个在下", f.leechers.unwrap_or(0)),
        ),
        Some(s) => step(
            "swarm 有没有源",
            Outcome::Ok,
            format!("{s} 个做种 · {} 个在下", f.leechers.unwrap_or(0)),
        ),
    });

    if f.seeders.unwrap_or(0) == 0 {
        steps.push(step("和 peer 握手", Outcome::Skipped, "没有源可连"));
        steps.push(step("对照组", Outcome::Skipped, "同上"));
        let (verdict, advice) = conclude(&f);
        return Report {
            steps,
            verdict,
            advice,
        };
    }

    // 5. 握手
    let peers = gather_peers(raw_hash).await;
    if peers.is_empty() {
        steps.push(step("和 peer 握手", Outcome::Skipped, "没拿到 peer 地址"));
    } else {
        let counts = probe_peers(&peers, raw_hash).await;
        f.probe = Some(counts);
        let total = counts.0 + counts.1 + counts.2;
        let detail = format!(
            "{}/{} 成功（{} 个连不上，{} 个被拒）",
            counts.0, total, counts.1, counts.2
        );
        steps.push(step(
            "和 peer 握手",
            if rate(counts).unwrap_or(0.0) >= 0.3 {
                Outcome::Ok
            } else {
                Outcome::Bad
            },
            detail,
        ));
    }

    // 6. 对照组 —— 只在目标 swarm 握手不理想时才跑，省时间。
    if f.probe.and_then(rate).unwrap_or(0.0) < 0.3 {
        if let Some(control_hash) = health::parse_info_hash(CONTROL_INFO_HASH) {
            let control_peers = gather_peers(control_hash).await;
            if control_peers.is_empty() {
                steps.push(step("对照组", Outcome::Skipped, "对照组也拿不到 peer 地址"));
            } else {
                let counts = probe_peers(&control_peers, control_hash).await;
                f.control = Some(counts);
                let total = counts.0 + counts.1 + counts.2;
                steps.push(step(
                    "对照组（Ubuntu 官方种子）",
                    if rate(counts).unwrap_or(0.0) >= 0.3 {
                        Outcome::Ok
                    } else {
                        Outcome::Bad
                    },
                    format!("{}/{} 成功", counts.0, total),
                ));
            }
        }
    } else {
        steps.push(step(
            "对照组",
            Outcome::Skipped,
            "目标 swarm 握手正常，不需要对照",
        ));
    }

    let (verdict, advice) = conclude(&f);
    Report {
        steps,
        verdict,
        advice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            torrent_state: "live".into(),
            bind_device_up: true,
            seeders: Some(20),
            leechers: Some(5),
            probe: Some((8, 2, 6)),
            ..Default::default()
        }
    }

    /// 不先查任务状态的话，会把「你自己按了暂停」诊断成「没源」——
    /// 然后用户跑去换种子，问题当然还在。
    #[test]
    fn paused_is_reported_before_anything_else() {
        let f = Facts {
            torrent_state: "paused".into(),
            // 故意把后面每一环都设成有问题，确认它们不会抢在前面。
            seeders: Some(0),
            route_is_tunnel: true,
            probe: Some((0, 8, 8)),
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("暂停"), "实际：{v}");
    }

    #[test]
    fn dead_bind_device_beats_swarm_problems() {
        let f = Facts {
            bind_device: Some("en5".into()),
            bind_device_up: false,
            seeders: Some(0),
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("en5"), "实际：{v}");
        assert!(advice.unwrap().contains("重启"));
    }

    /// 这条是整个功能的来由：两天的排查，答案就是这一句。
    #[test]
    fn tunnel_is_detected_and_explained() {
        let f = Facts {
            bind_device: None,
            route_interface: Some("utun6".into()),
            route_is_tunnel: true,
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("utun6"), "实际：{v}");
        let advice = advice.unwrap();
        if cfg!(windows) {
            assert!(
                advice.contains("mydl.exe"),
                "Windows 上该给出直连规则建议：{advice}"
            );
        } else {
            assert!(advice.contains("en0"), "该给出可操作的建议：{advice}");
        }
    }

    /// 已经绑到物理网卡的话，系统默认路由是隧道就无所谓了 —— 不该误报。
    #[test]
    fn bound_to_physical_is_not_a_tunnel_problem() {
        let f = Facts {
            bind_device: Some("en0".into()),
            route_interface: Some("utun6".into()),
            route_is_tunnel: true,
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(!v.contains("隧道"), "绑了物理网卡不该报隧道问题：{v}");
    }

    #[test]
    fn zero_seeders_says_change_source() {
        let f = Facts {
            seeders: Some(0),
            leechers: Some(30),
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("没人做种"), "实际：{v}");
        assert!(advice.unwrap().contains("找更好的源"));
    }

    /// 一个 tracker 都没应答 ≠ 0 个做种。混为一谈会把网络抖动报成资源已死。
    #[test]
    fn no_tracker_response_is_not_zero_seeders() {
        let f = Facts {
            seeders: None,
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(!v.contains("没人做种"), "不该断言资源已死：{v}");
        assert!(v.contains("查不到"), "实际：{v}");
    }

    /// 握手基本失败但对照组正常 -> 是这个资源的问题。
    #[test]
    fn bad_probe_good_control_blames_the_swarm() {
        let f = Facts {
            probe: Some((0, 4, 12)),
            control: Some((10, 2, 4)),
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("不是你的网络"), "实际：{v}");
    }

    /// 真实任务可能已经通过 DHT / PEX 连上一个慢 peer，而临时抽到的公开样本
    /// 全部失联。报告必须说「正在慢速下载」，不能自相矛盾地说完全连不上。
    #[test]
    fn live_peer_and_speed_are_reported_when_public_probe_fails() {
        let f = Facts {
            peers_live: 1,
            download_speed_bps: 94.0 * 1024.0,
            probe: Some((0, 12, 1)),
            control: Some((7, 5, 4)),
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("1 个 peer"), "实际：{v}");
        assert!(v.contains("94 KiB/s"), "实际：{v}");
        assert!(v.contains("可用 peer 太少"), "实际：{v}");
        assert!(v.contains("不是你的网络或 VPN"), "实际：{v}");
        assert!(advice.unwrap().contains("本地设置修不好"));
    }

    /// 两边都失败 -> 是本地网络的问题。方向和上一条完全相反，
    /// 而且给出的建议也必须相反。
    #[test]
    fn bad_probe_bad_control_blames_the_network() {
        let f = Facts {
            probe: Some((0, 4, 12)),
            control: Some((0, 8, 8)),
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("问题在你的网络"), "实际：{v}");
        assert!(advice.unwrap().contains("VPN"));
    }

    /// 两边都失败、而且自己配了 SOCKS5 —— 头号嫌疑是那个代理，不能笼统地
    /// 说「网络不好」让人去重启路由器。这是用户一步就能验证掉的。
    #[test]
    fn socks5_proxy_is_named_when_even_the_control_fails() {
        let f = Facts {
            proxy_configured: true,
            probe: Some((0, 4, 12)),
            control: Some((0, 8, 8)),
            ..facts()
        };
        let (v, advice) = conclude(&f);
        assert!(v.contains("问题在你的网络"), "实际：{v}");
        let advice = advice.unwrap();
        assert!(advice.contains("SOCKS5"), "该点名代理：{advice}");
        assert!(advice.contains("重启 App"), "该给出可操作的一步：{advice}");
    }

    /// 对照组是这套判断的支点。它没跑成的时候必须承认「分不清」，
    /// 而不是默认甩锅给某一边。
    #[test]
    fn missing_control_admits_uncertainty() {
        let f = Facts {
            probe: Some((0, 4, 12)),
            control: None,
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("分不清"), "实际：{v}");
        assert!(!v.contains("不是你的网络"));
        assert!(!v.contains("问题在你的网络"));
    }

    /// 僧多粥少不是故障，别让人白折腾。
    #[test]
    fn starving_swarm_is_not_a_fault() {
        let f = Facts {
            seeders: Some(2),
            leechers: Some(40),
            probe: Some((10, 2, 4)),
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("慢是正常的"), "实际：{v}");
    }

    #[test]
    fn parses_compact_peer_list() {
        // 两条：1.2.3.4:6881 和 5.6.7.8:0（端口 0 是占位，要滤掉）
        let body = [1, 2, 3, 4, 0x1a, 0xe1, 5, 6, 7, 8, 0, 0];
        let peers = parse_peers(&body);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].to_string(), "1.2.3.4:6881");
    }

    /// 真机上跑一遍选路探测，确认不 panic、并且认出来的是真实存在的网卡。
    #[test]
    fn route_interface_is_a_real_one() {
        if let Some(dev) = default_route_interface() {
            assert!(netif::exists(&dev), "选路探测给出了不存在的网卡：{dev}");
        }
    }
    /// 目标 25% vs 对照组 37% —— 这点差距在噪声范围内，不该拿来断言
    /// 「不是你的网络」。实测就出现过这一幕。
    #[test]
    fn similar_rates_do_not_blame_either_side() {
        let f = Facts {
            probe: Some((4, 0, 12)),   // 25%
            control: Some((6, 0, 10)), // 37.5%
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("差不多"), "实际：{v}");
        assert!(
            !v.contains("不是你的网络"),
            "差距在噪声里不该下这个结论：{v}"
        );
        assert!(!v.contains("问题在你的网络"));
    }

    /// 对照组明显更好（两倍以上）才敢归咎于这个资源。
    #[test]
    fn clearly_better_control_blames_the_swarm() {
        let f = Facts {
            probe: Some((1, 0, 15)),   // 6%
            control: Some((12, 0, 4)), // 75%
            ..facts()
        };
        let (v, _) = conclude(&f);
        assert!(v.contains("不是你的网络"), "实际：{v}");
    }
}
