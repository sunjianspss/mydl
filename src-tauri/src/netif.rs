//! 「BT 走哪张网卡」用到的网卡枚举。
//!
//! # 这个设置解决什么问题
//!
//! 开着全局 VPN / 规则代理（TUN 模式）时，默认路由指向 `utun*`，**BT 流量
//! 也跟着走隧道**。后果不是慢一点，是结构性的：
//!
//! - 隧道出口通常是机房 IP，大量 BT 客户端和 tracker 会屏蔽或限速数据中心
//!   IP 段 —— 表现为 TCP 连得上、握手立刻被 RST 或关闭。
//! - UPnP 的多播出不了隧道，端口映射必然失败；VPN 也不会把端口转发给你。
//!   **没有入站连接，做种就是无效劳动** —— 做种完全靠别人连进来。
//! - 出口是共享的，别人怎么用这个 IP 你控制不了，成功率会毫无规律地波动。
//!
//! 绑到物理网卡（macOS 用 `IP_BOUND_IF`，Linux 用 `SO_BINDTODEVICE`）就能
//! 绕过默认路由直出，同时**完全不动 VPN 本身** —— 浏览器照旧走隧道。
//!
//! librqbit 的 `SessionOptions::bind_device_name` 覆盖 DHT、BT-UDP、BT-TCP、
//! tracker 和 LSD，一个开关就够。
//!
//! # 为什么要列出来而不是让用户手输
//!
//! 名字写错的话 `BindDevice::new_from_name` 会失败，整个会话建不起来，App
//! 直接起不来。列出候选让界面做成下拉，打不出错字。

use serde::Serialize;

/// 一张可选的网卡。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetIf {
    /// 接口名，就是要写进设置里的那个（`en0`）。
    pub name: String,
    /// 它的 IPv4 地址，给界面显示用 —— 光看 `en0` / `en5` 分不出哪个是在用的。
    pub ipv4: Option<String>,
    /// 看着像隧道（`utun` / `ppp` / `ipsec` / `tap` / `tun`）。
    ///
    /// 界面要把这些标出来：绑到隧道上等于没绕过去，是这个设置最容易犯的错。
    pub is_tunnel: bool,
}

/// 名字看着像隧道接口。
///
/// 只按名字判断，不去猜路由表：判错了顶多是界面上少一句提示，而把物理网卡
/// 误判成隧道会让用户不敢选对的那张。
pub fn is_tunnel_name(name: &str) -> bool {
    looks_like_tunnel(name)
}

fn looks_like_tunnel(name: &str) -> bool {
    const PREFIXES: &[&str] = &["utun", "tun", "tap", "ppp", "ipsec", "gpd", "wg"];
    let lower = name.to_ascii_lowercase();
    PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// 系统自建的虚拟接口 —— **有 IPv4 也通不到外网**。
///
/// 为什么必须单独认出来：`list()` 的排序是「非隧道优先，然后按名字」，而这些
/// 名字**按字母序全排在 `en0` 前面**。它们平时没 IPv4 所以被 `is_candidate`
/// 滤掉了，可是一开「互联网共享」、起个虚拟机（`bridge100` 会拿到
/// `192.168.2.1`）或者开热点（`ap1`），它们就有地址了 —— 于是自动兜底会选中
/// 一张打不通的网卡，BT 静默变成 0 peers，症状和「没源」一模一样，极难排查。
///
/// 只影响**自动挑选**：`list()` 仍然把它们列出来，手动选是用户的自由。
fn is_virtual(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        // macOS
        "bridge", // 互联网共享 / 虚拟机桥接
        "anpi",   // Apple Silicon 内部 NIC
        "ap",     // 热点 / AP 模式
        "awdl",   // AirDrop
        "llw",    // low-latency WLAN
        // 虚拟机软件
        "vmnet", "vnic", "vbox", "veth", "docker", "virbr",
    ];
    let lower = name.to_ascii_lowercase();
    PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// 值得作为候选的网卡：有 IPv4、不是回环。
///
/// 隧道接口**保留**在列表里而不是过滤掉 —— 有人的确想绑到某条特定隧道上
/// （比如只给 BT 用的那条），只是要标出来。
fn is_candidate(name: &str, ipv4: Option<&str>) -> bool {
    if name.starts_with("lo") {
        return false;
    }
    match ipv4 {
        // 169.254.x.x 是没拿到 DHCP 时的自分配地址，绑上去必然不通。
        Some(ip) => !ip.starts_with("169.254."),
        None => false,
    }
}

/// 列出能绑的网卡。失败时返回空列表 —— 界面会退回「跟随系统」。
pub fn list() -> Vec<NetIf> {
    use network_interface::{NetworkInterface, NetworkInterfaceConfig};

    let Ok(ifaces) = NetworkInterface::show() else {
        tracing::warn!("枚举网卡失败，「BT 走哪张网卡」只能手填");
        return Vec::new();
    };

    let mut out: Vec<NetIf> = Vec::new();
    for i in ifaces {
        // 一张卡可能有多个地址，取第一个 IPv4。
        let ipv4 = i
            .addr
            .iter()
            .find_map(|a| match a.ip() {
                std::net::IpAddr::V4(v4) => Some(v4.to_string()),
                _ => None,
            });

        if !is_candidate(&i.name, ipv4.as_deref()) {
            continue;
        }
        // 同名接口可能出现多次（多个地址），只留一条。
        if out.iter().any(|e| e.name == i.name) {
            continue;
        }
        out.push(NetIf {
            is_tunnel: looks_like_tunnel(&i.name),
            name: i.name,
            ipv4,
        });
    }

    // 物理网卡排前面：那才是这个设置九成的用途。
    out.sort_by_key(|e| (e.is_tunnel, e.name.clone()));
    out
}

/// 从候选里挑一张自动绑定用的。纯函数，好测 —— 挑选规则出错的代价太大
/// （绑到打不通的网卡上，BT 静默 0 peers），不能只靠真机上跑一遍。
///
/// 规则：跳过隧道（绑了等于没绕过去）和系统虚拟接口（通不到外网），
/// 剩下的取第一个。`list()` 已经排好序，这里保持它的顺序。
fn pick_default(candidates: &[NetIf]) -> Option<&NetIf> {
    candidates
        .iter()
        .find(|i| !i.is_tunnel && !is_virtual(&i.name))
}

/// 自动绑定该用哪张网卡（`en0` / `eth0` 这类）。
///
/// 开着 VPN 时默认路由指向 `utun*`，不主动绑的话 BT 就跟着走隧道了 ——
/// 这正是这个设置要解决的。枚举失败或没有合适的返回 `None`，引擎退回
/// 跟随系统路由。
pub fn first_physical() -> Option<String> {
    pick_default(&list()).map(|i| i.name.clone())
}

/// 这张网卡现在还在不在。
///
/// 用来挡住一个会让 App 起不来的场景：配置里写着 `en5`（USB 网卡），
/// 网卡拔了之后 `if_nametoindex` 返回 0，`BindDevice::new_from_name` 失败，
/// 而 librqbit 用 `?` 把它抛成 `Session::new` 的错误 —— App 整个起不来。
pub fn exists(name: &str) -> bool {
    list().iter().any(|i| i.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_tunnel_names() {
        for n in ["utun6", "utun0", "tun0", "ppp0", "ipsec1", "wg0"] {
            assert!(looks_like_tunnel(n), "{n} 该被认成隧道");
        }
        for n in ["en0", "en5", "eth0", "bridge100", "awdl0"] {
            assert!(!looks_like_tunnel(n), "{n} 不该被认成隧道");
        }
    }

    #[test]
    fn skips_loopback_and_address_less() {
        assert!(!is_candidate("lo0", Some("127.0.0.1")));
        assert!(!is_candidate("en0", None), "没 IPv4 地址的绑上去也没用");
        assert!(is_candidate("en0", Some("172.18.13.241")));
    }

    /// 没拿到 DHCP 时系统会自分配 169.254.x.x，绑上去必然不通，
    /// 列出来只会让人选错。
    #[test]
    fn skips_link_local_autoconfig() {
        assert!(!is_candidate("en5", Some("169.254.12.9")));
    }

    /// 真机上跑一遍，确保枚举本身不 panic。具体有哪些卡因机器而异，
    /// 所以只断言「回环没混进来」这种一定成立的性质。
    #[test]
    fn listing_is_sane_on_this_machine() {
        for i in list() {
            assert!(!i.name.starts_with("lo"), "回环不该出现：{i:?}");
            assert!(i.ipv4.is_some(), "候选必须有 IPv4：{i:?}");
        }
    }

    /// 默认兜底选的必须是物理网卡，隧道不该被选中 —— 选隧道等于没绕过去。
    #[test]
    fn first_physical_is_not_a_tunnel() {
        if let Some(name) = first_physical() {
            assert!(
                !looks_like_tunnel(&name),
                "默认网卡不该是隧道：{name}"
            );
        }
    }
    fn nif(name: &str) -> NetIf {
        NetIf {
            is_tunnel: looks_like_tunnel(name),
            name: name.into(),
            ipv4: Some("10.0.0.2".into()),
        }
    }

    /// 这条是这个模块存在的全部理由。
    ///
    /// `list()` 按「非隧道优先，然后按名字」排序，而 macOS 上一堆系统虚拟
    /// 接口（`anpi*` `ap*` `awdl*` `bridge*`）**按字母序全排在 `en0` 前面**。
    /// 它们平时没 IPv4 所以看不出问题，一开互联网共享 / 虚拟机 / 热点就有了
    /// —— 那时候自动兜底会挑中一张通不到外网的卡，BT 静默变成 0 peers。
    #[test]
    fn virtual_interfaces_never_win_over_the_real_one() {
        // 顺序刻意按 list() 排好的样子给：虚拟的全在 en0 前面。
        let candidates = ["anpi0", "ap1", "awdl0", "bridge100", "en0", "en5", "utun6"]
            .map(nif)
            .to_vec();
        assert_eq!(
            pick_default(&candidates).map(|i| i.name.as_str()),
            Some("en0"),
            "自动挑选必须跳过系统虚拟接口"
        );
    }

    #[test]
    fn picks_nothing_when_only_virtual_or_tunnel() {
        let candidates = ["bridge100", "utun6", "vmnet1"].map(nif).to_vec();
        assert_eq!(
            pick_default(&candidates).map(|i| i.name.as_str()),
            None,
            "没有真网卡时该返回 None，让引擎退回跟随系统，而不是硬挑一张"
        );
    }

    #[test]
    fn recognizes_virtual_names() {
        for n in ["bridge0", "bridge100", "anpi0", "ap1", "awdl0", "llw0", "vmnet8", "vboxnet0"] {
            assert!(is_virtual(n), "{n} 该被认成虚拟接口");
        }
        // 这些是真网卡，绝不能被误杀 —— 误杀的话自动绑定会退回跟随系统，
        // 等于这个功能白做。
        for n in ["en0", "en5", "eth0", "enp3s0"] {
            assert!(!is_virtual(n), "{n} 不该被认成虚拟接口");
        }
    }

    /// `ap` 这个前缀有点短，确认它不会误伤真实网卡名。
    #[test]
    fn short_prefix_does_not_overmatch() {
        assert!(!is_virtual("en0"));
        assert!(!is_virtual("eth0"));
    }

}
