//! API key 存系统钥匙串，不落在 settings.json 里。
//!
//! settings.json 是明文的，还会被用户随手发出来排查问题（README 的「分发」
//! 一节就在教人去翻那个目录）。key 混在里面迟早泄露。macOS 走钥匙串、
//! Windows 走凭据管理器，都由 `keyring` 抹平。

use anyhow::{Context, Result};

const SERVICE: &str = "com.sun.mydl";
const ACCOUNT: &str = "ai-api-key";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("打不开系统钥匙串")
}

/// 存一条 key。传空串等于删除 —— 界面上清空输入框就是这个意思。
pub fn set_ai_key(key: &str) -> Result<()> {
    let e = entry()?;
    if key.is_empty() {
        // 本来就没有的时候删会报 NoEntry，那不算失败。
        return match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err).context("从钥匙串删除失败"),
        };
    }
    e.set_password(key).context("写入钥匙串失败")
}

/// 读 key。没存过返回 None。
///
/// 读不出来（比如用户在钥匙串里点了拒绝）只记一条日志，当作没配 —— 搜索
/// 会退化成不排序，比整个功能报错好。
pub fn ai_key() -> Option<String> {
    match entry().and_then(|e| e.get_password().context("读取钥匙串失败")) {
        Ok(k) if !k.is_empty() => Some(k),
        Ok(_) => None,
        Err(e) => {
            // 只在真的出错时提一句；「没存过」是常态，不该刷屏。
            if !format!("{e:#}").contains("No matching entry") {
                tracing::debug!("读取 API key 失败：{e:#}");
            }
            None
        }
    }
}

pub fn has_ai_key() -> bool {
    ai_key().is_some()
}
