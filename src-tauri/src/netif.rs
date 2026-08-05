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
fn looks_like_tunnel(name: &str) -> bool {
    const PREFIXES: &[&str] = &["utun", "tun", "tap", "ppp", "ipsec", "gpd", "wg"];
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
}
