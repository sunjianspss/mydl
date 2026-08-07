//! 自适应上传限速：把缺失的 LEDBAT 补回来。
//!
//! # 为什么需要
//!
//! `librqbit-utp` 0.7 用的是 CUBIC（`src/congestion/` 下只有 `cubic.rs`，
//! `lib.rs` 里还留着 `// TODO: LEDBAT congestion control`），抢带宽和普通 TCP
//! 一样凶。BEP29 那套 uTP 本该「探测到排队延迟上升就主动退让」，这半没有。
//!
//! 现在的补救是手填一个全局上传上限。问题是那个数字**必须按最坏情况填**：
//! 填 512 KB/s，那么在没人开会、没人刷网页的时候也只能跑 512。
//!
//! 这里做的就是 LEDBAT 的控制回路本身：**测排队延迟，涨了就退，稳了就进**。
//!
//! # 测什么
//!
//! 到默认网关的 RTT。它测的是本地链路 + 路由器排队 —— 上行被打满时，
//! 数据包在路由器上行队列里排队，这条 RTT 会先涨起来。
//!
//! **不是完美的探针**：如果瓶颈缓冲区在运营商侧而不是你的路由器，网关 RTT
//! 可能不涨。但它零成本、不依赖外部服务、不受 VPN 隧道干扰（隧道会伪造
//! 对公网地址的 ICMP 应答，而网关是本地的）。
//!
//! # 只在「我们是元凶」时才退让
//!
//! 延迟涨了不一定是我们造成的 —— 可能是别人在看 4K，也可能是 Wi-Fi 抖动。
//! 所以**只有当实际上传速度接近当前上限时才减速**：那时候我们确实是在
//! 顶着天花板跑，退让才有意义。否则只是把一个本来就没用满的限额调得更小。

use serde::Serialize;

/// 允许高出基线多少毫秒。
///
/// LEDBAT 规范用 100ms。这里用 60ms：家用路由器的上行缓冲通常几十毫秒就
/// 开始明显影响交互，等到 100ms 时刷网页已经卡了。
const TARGET_EXTRA_MS: f64 = 60.0;

/// 超标时乘性减。0.75 比 LEDBAT 的线性减更快 —— 卡顿是立刻能感知的，
/// 宁可退猛一点再慢慢爬回来。
const DECREASE_FACTOR: f64 = 0.75;

/// 没超标时加性增，每轮加这么多字节/秒。
const INCREASE_STEP: u32 = 48 * 1024;

/// 下限。再低下去 tit-for-tat 会让下载也一起完蛋 —— README「上传限速」
/// 那节说的「低于 32 KB/s 就比较明显了」。
const FLOOR_BPS: u32 = 48 * 1024;

/// 实际上传达到上限的这个比例才算「我们在顶着天花板跑」。
const SATURATION: f64 = 0.8;

/// 基线用多少个窗口的最小值。每个窗口独立算最小，整体取最小 ——
/// 这样一个偶然的低值最多影响一个窗口的时间就会过期。
const BASELINE_WINDOWS: usize = 6;

/// 每个基线窗口收多少个样本。
const SAMPLES_PER_WINDOW: usize = 12;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// 当前算出来的上限（字节/秒）。
    pub limit_bps: u32,
    /// 基线 RTT（毫秒）。None = 还没攒够样本。
    pub baseline_ms: Option<f64>,
    /// 最近一次 RTT。
    pub last_rtt_ms: Option<f64>,
    /// 当前排队延迟 = 最近 RTT - 基线。
    pub queue_delay_ms: Option<f64>,
}

/// AIMD 控制器。**不碰网络**，只做算术 —— 采样交给调用方，这样能测。
#[derive(Debug)]
pub struct Controller {
    /// 用户设定的天花板：自适应再怎么涨也不超过它。
    ceiling_bps: u32,
    limit_bps: u32,
    /// 每个窗口的最小 RTT。`None` 表示这个窗口还没有样本。
    windows: [Option<f64>; BASELINE_WINDOWS],
    cur: usize,
    in_window: usize,
    last_rtt: Option<f64>,
}

impl Controller {
    /// `ceiling_bps` 是上限的上限。传 0 表示不限 —— 那时用一个保守的
    /// 起点，让它自己往上爬，而不是一上来就打满把线路顶死。
    pub fn new(ceiling_bps: u32) -> Self {
        let ceiling = if ceiling_bps == 0 { u32::MAX } else { ceiling_bps };
        Self {
            ceiling_bps: ceiling,
            // 从一个温和的值起步往上爬，别一上来就冲。
            limit_bps: (512 * 1024).min(ceiling),
            windows: [None; BASELINE_WINDOWS],
            cur: 0,
            in_window: 0,
            last_rtt: None,
        }
    }

    pub fn baseline_ms(&self) -> Option<f64> {
        self.windows
            .iter()
            .flatten()
            .copied()
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))))
    }

    fn push_rtt(&mut self, rtt: f64) {
        let slot = &mut self.windows[self.cur];
        *slot = Some(slot.map_or(rtt, |m| m.min(rtt)));
        self.in_window += 1;
        if self.in_window >= SAMPLES_PER_WINDOW {
            self.in_window = 0;
            self.cur = (self.cur + 1) % BASELINE_WINDOWS;
            // 轮到的新窗口清空 —— 这就是基线的「过期」机制：
            // 一个偶然的极低值最多活 BASELINE_WINDOWS 个窗口。
            self.windows[self.cur] = None;
        }
    }

    /// 走一步。
    ///
    /// - `rtt_ms`：这一轮测到的 RTT；`None` 表示没测到（ping 失败），此时**保持不动**
    /// - `upload_bps`：当前实际上传速度，用来判断我们是不是元凶
    pub fn step(&mut self, rtt_ms: Option<f64>, upload_bps: f64) -> State {
        let Some(rtt) = rtt_ms else {
            // 测不到就别乱动。宁可维持现状，也不要凭空调整。
            return self.state();
        };
        self.last_rtt = Some(rtt);
        self.push_rtt(rtt);

        let Some(base) = self.baseline_ms() else {
            return self.state();
        };
        let delay = rtt - base;
        let saturated = upload_bps >= self.limit_bps as f64 * SATURATION;

        if delay > TARGET_EXTRA_MS && saturated {
            // 我们在顶着上限跑，而且延迟起来了 —— 退。
            let next = (self.limit_bps as f64 * DECREASE_FACTOR) as u32;
            self.limit_bps = next.max(FLOOR_BPS);
        } else if delay <= TARGET_EXTRA_MS / 2.0 {
            // 延迟回落到目标的一半以下才敢加，留出滞回区间，
            // 否则会在阈值附近来回抖。
            self.limit_bps = self
                .limit_bps
                .saturating_add(INCREASE_STEP)
                .min(self.ceiling_bps);
        }
        self.state()
    }

    pub fn state(&self) -> State {
        let base = self.baseline_ms();
        State {
            limit_bps: self.limit_bps,
            baseline_ms: base,
            last_rtt_ms: self.last_rtt,
            queue_delay_ms: match (self.last_rtt, base) {
                (Some(r), Some(b)) => Some(r - b),
                _ => None,
            },
        }
    }
}

/// 测一次到 `gateway` 的 RTT，毫秒。
///
/// 用系统的 `ping` 而不是自己开 raw socket —— 后者要 root。`ping` 是 setuid 的。
pub fn ping_once(gateway: &str) -> Option<f64> {
    let out = std::process::Command::new("ping")
        .args(["-c", "1", "-t", "2", gateway])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // `64 bytes from 172.18.13.1: icmp_seq=0 ttl=64 time=4.203 ms`
    let at = text.find("time=")? + 5;
    let rest = &text[at..];
    let end = rest.find(' ')?;
    rest[..end].parse().ok()
}

/// 默认路由用的网关。
///
/// **必须按网卡问**：开着 VPN 时 `route get default` 给的是隧道那条，
/// 而隧道没有可 ping 的网关（`gateway: link#24`）。BT 绑在物理网卡上，
/// 要测的也是物理链路。
pub fn gateway_for(interface: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("route");
    cmd.arg("-n").arg("get");
    if let Some(i) = interface {
        cmd.arg("-ifscope").arg(i);
    }
    cmd.arg("default");
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("gateway:"))
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty() && !g.starts_with("link"))
}

// ---------------------------------------------------------------------------
// 后台循环
// ---------------------------------------------------------------------------

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 采样间隔。5 秒够跟上带宽变化，又不会把 ping 打成噪声源。
const TICK: Duration = Duration::from_secs(5);

/// 供界面读的当前状态。
pub type Shared = Arc<Mutex<Option<State>>>;

pub fn spawn(
    engine: Arc<crate::engine::Engine>,
    store: Arc<crate::settings::SettingsStore>,
    shared: Shared,
) {
    std::thread::spawn(move || {
        let mut controller: Option<Controller> = None;
        let mut gateway: Option<String> = None;
        let mut last_ceiling = u32::MAX;

        loop {
            std::thread::sleep(TICK);
            let s = store.get();

            if !s.adaptive_upload_limit {
                // 关掉时把控制器丢掉，下次开启重新学基线 —— 网络环境
                // 可能已经变了，沿用旧基线会立刻误判。
                if controller.is_some() {
                    controller = None;
                    *shared.lock().unwrap() = None;
                    engine.set_upload_limit(s.upload_limit_bps());
                    tracing::info!("自适应上传限速已关闭，恢复为手动设定值");
                }
                continue;
            }

            let ceiling = s.upload_limit_bps().unwrap_or(0);
            // 用户改了手动上限就重建 —— 天花板变了，之前爬到的值可能已经越界。
            if controller.is_none() || ceiling != last_ceiling {
                controller = Some(Controller::new(ceiling));
                last_ceiling = ceiling;
                gateway = gateway_for(s.bind_device.as_deref());
                tracing::info!(网关 = ?gateway, 天花板 = ceiling, "自适应上传限速启动");
            }

            let Some(gw) = gateway.as_deref() else {
                // 网关都找不到就别装作在自适应，退回手动值。
                engine.set_upload_limit(s.upload_limit_bps());
                continue;
            };

            let rtt = ping_once(gw);
            let up: f64 = engine.list().iter().map(|t| t.upload_speed_bps).sum();

            if let Some(c) = controller.as_mut() {
                let state = c.step(rtt, up);
                engine.set_upload_limit(Some(state.limit_bps));
                *shared.lock().unwrap() = Some(state);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: u32 = 1024;

    /// 喂满一个基线窗口所需的样本数。
    fn warmup(c: &mut Controller, rtt: f64) {
        for _ in 0..SAMPLES_PER_WINDOW {
            c.step(Some(rtt), 0.0);
        }
    }

    #[test]
    fn learns_a_baseline() {
        let mut c = Controller::new(0);
        assert_eq!(c.baseline_ms(), None, "没样本时不该有基线");
        warmup(&mut c, 5.0);
        assert_eq!(c.baseline_ms(), Some(5.0));
    }

    /// 延迟起来 + 我们在顶着上限跑 -> 退让。
    #[test]
    fn backs_off_when_we_are_the_cause() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        let before = c.state().limit_bps;
        // 上传打满，延迟涨到基线 + 200ms
        let after = c.step(Some(205.0), before as f64);
        assert!(after.limit_bps < before, "该退让：{before} -> {}", after.limit_bps);
        assert_eq!(after.queue_delay_ms, Some(200.0));
    }

    /// 这条是这个控制器最要紧的克制：延迟涨了，但**我们没在满负荷上传** ——
    /// 那多半是别人在占带宽或者 Wi-Fi 抖动，跟着降只会白白限死自己。
    #[test]
    fn does_not_back_off_when_we_are_idle() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        let before = c.state().limit_bps;
        let after = c.step(Some(305.0), 0.0);
        assert!(after.limit_bps >= before, "空载时不该因为别人的流量降速");
    }

    #[test]
    fn climbs_back_when_the_line_is_quiet() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        let start = c.state().limit_bps;
        for _ in 0..5 {
            c.step(Some(5.0), 0.0);
        }
        assert!(c.state().limit_bps > start, "延迟正常时该往上爬");
    }

    /// 阈值附近要有滞回，否则会在「刚好超标」和「刚好不超标」之间来回抖。
    #[test]
    fn has_hysteresis_between_back_off_and_climb() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        let before = c.state().limit_bps;
        // 延迟在目标之下、但高于目标的一半 —— 既不退也不进
        let after = c.step(Some(5.0 + TARGET_EXTRA_MS * 0.7), before as f64);
        assert_eq!(after.limit_bps, before, "滞回区间里该保持不动");
    }

    #[test]
    fn never_goes_below_the_floor() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        for _ in 0..50 {
            let s = c.state();
            c.step(Some(1005.0), s.limit_bps as f64);
        }
        assert_eq!(c.state().limit_bps, FLOOR_BPS, "跌破下限会把下载也拖死");
    }

    #[test]
    fn never_exceeds_the_user_ceiling() {
        let ceiling = 256 * KB;
        let mut c = Controller::new(ceiling);
        for _ in 0..100 {
            c.step(Some(5.0), 0.0);
        }
        assert_eq!(c.state().limit_bps, ceiling, "不该超过用户设的天花板");
    }

    /// ping 失败时保持不动 —— 凭空调整比什么都不做更糟。
    #[test]
    fn holds_when_the_probe_fails() {
        let mut c = Controller::new(4096 * KB);
        warmup(&mut c, 5.0);
        let before = c.state().limit_bps;
        for _ in 0..10 {
            c.step(None, before as f64);
        }
        assert_eq!(c.state().limit_bps, before);
    }

    /// 基线必须会过期。一次偶然的极低 RTT 如果永久成为基线，
    /// 之后所有正常延迟都会被判成「超标」，限速会一路跌到下限。
    #[test]
    fn baseline_expires() {
        let mut c = Controller::new(4096 * KB);
        c.step(Some(1.0), 0.0); // 一个偶然的极低值
        for _ in 0..(SAMPLES_PER_WINDOW * BASELINE_WINDOWS + SAMPLES_PER_WINDOW) {
            c.step(Some(20.0), 0.0);
        }
        let base = c.baseline_ms().unwrap();
        assert!(
            (base - 20.0).abs() < 0.01,
            "偶然的低值该过期，基线应回到 20ms，实际 {base}"
        );
    }

    #[test]
    fn parses_ping_output_shape() {
        // ping_once 依赖系统命令，这里只保证解析逻辑对得上真实输出格式
        let sample = "64 bytes from 172.18.13.1: icmp_seq=0 ttl=64 time=4.203 ms";
        let at = sample.find("time=").unwrap() + 5;
        let rest = &sample[at..];
        let end = rest.find(' ').unwrap();
        assert_eq!(rest[..end].parse::<f64>().unwrap(), 4.203);
    }
}
