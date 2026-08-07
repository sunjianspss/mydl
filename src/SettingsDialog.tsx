import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { NetIf, Settings } from "./types";
import { IS_MAC } from "./platform";

/// 对应 settings.rs 的 FOLLOW_SYSTEM_ROUTE。真实网卡名里不会有尖括号。
const FOLLOW_SYSTEM_ROUTE = "<system>";

// 勾上「限制上传速度」时的初始值，KB/s。约 1 Mbps —— 基本任何家用上行都
// 感觉不到，同时不至于低到让 tit-for-tat 把下载速度也拖下去。
const DEFAULT_UPLOAD_LIMIT = 128;

interface Props {
  initial: Settings;
  onSaved: (settings: Settings) => void;
  onClose: () => void;
  onError: (message: string) => void;
}

export default function SettingsDialog({ initial, onSaved, onClose, onError }: Props) {
  const [draft, setDraft] = useState<Settings>(initial);
  const [saving, setSaving] = useState(false);
  // key 存在系统钥匙串里，读不回来 —— 只能知道有没有。空串 = 不改动。
  const [keyInput, setKeyInput] = useState("");
  const [hasKey, setHasKey] = useState(false);
  // 可绑定的网卡。名字写错会让整个会话建不起来，所以做成下拉不让手输。
  const [ifaces, setIfaces] = useState<NetIf[]>([]);

  useEffect(() => {
    // Windows 上 librqbit 的 BindDevice 直接报错，列出来也没用。
    if (!IS_MAC) return;
    invoke<NetIf[]>("network_interfaces").then(setIfaces).catch(() => {});
  }, []);

  useEffect(() => {
    invoke<boolean>("has_ai_key").then(setHasKey).catch(() => {});
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !saving) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [saving, onClose]);

  const patch = (p: Partial<Settings>) => setDraft((d) => ({ ...d, ...p }));

  async function pickMoveTo() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") patch({ moveTo: picked });
  }

  async function save() {
    setSaving(true);
    try {
      await invoke("save_settings", { settings: draft });
      // 输入框留空表示不动原来的；要清掉 key 得点「清除」。
      if (keyInput) await invoke("set_ai_key", { key: keyInput });
      onSaved(draft);
      onClose();
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="overlay" onClick={() => !saving && onClose()}>
      <div className="dialog settings-dialog" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">设置</h2>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.preventSleepWhileDownloading}
            onChange={(e) => patch({ preventSleepWhileDownloading: e.target.checked })}
          />
          <span>
            <b>下载期间不让电脑休眠</b>
            <em>
              只在有任务<b>正在下载</b>时生效，下完自动解除；单纯做种不会阻止休眠。
              合盖仍然会睡 —— 想挂整夜的话别合盖。
            </em>
          </span>
        </label>

        <div className="setting setting-block">
          <span>
            <b>搜索：索引器地址</b>
            <em>
              搜索结果来自你自己搭的 <b>Prowlarr 或 Jackett</b>，我们只是个客户端。
              把它们界面上那条 Torznab 地址整条粘进来（含 apikey）。留空则搜索不可用。
            </em>
            <input
              className="setting-input"
              type="text"
              spellCheck={false}
              placeholder="http://localhost:9696/api/v1/indexer/all/results/torznab/api?apikey=…"
              value={draft.searchUrl ?? ""}
              onChange={(e) => patch({ searchUrl: e.target.value.trim() || null })}
            />
          </span>
        </div>

        <div className="setting setting-block">
          <span>
            <b>搜索：AI 排序</b>
            <em>
              让模型把结果按你的意图重排并给出理由。<b>模型只能对索引器给的列表重排，
              不能产出链接</b> —— 磁力链的 hash 是内容摘要，模型只会编。API key
              <b>明文存在 settings.json</b> 里，介意就别填。
            </em>
            {/* key 存在系统钥匙串里、读不回来，所以输入框永远是空的。
                光靠灰色占位符太容易被当成「没保存」，这里给一行明确状态。 */}
            <div className="key-status">
              {hasKey ? (
                <>
                  <span className="key-ok">● 已保存到系统钥匙串</span>
                  <button
                    className="act-text"
                    disabled={saving}
                    onClick={async () => {
                      await invoke("set_ai_key", { key: "" });
                      setHasKey(false);
                      setKeyInput("");
                    }}
                  >
                    清除
                  </button>
                </>
              ) : (
                <span className="key-none">尚未保存</span>
              )}
            </div>
            <input
              className="setting-input"
              type="password"
              spellCheck={false}
              placeholder={hasKey ? "要更换就填新的，留空表示不动" : "DeepSeek API key"}
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
            />
            <input
              className="setting-input"
              type="text"
              spellCheck={false}
              placeholder="模型 id"
              value={draft.aiModel}
              onChange={(e) => patch({ aiModel: e.target.value })}
            />
          </span>
        </div>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.watchClipboard}
            onChange={(e) => patch({ watchClipboard: e.target.checked })}
          />
          <span>
            <b>切回窗口时检查剪贴板</b>
            <em>
              剪贴板里有磁力链就显示一个横幅问要不要添加，<b>绝不自动添加</b>。
              只在窗口重新获得焦点时读一次，不在后台轮询 —— 常驻读剪贴板既让人
              不安，macOS 15 起还会弹「某某读取了剪贴板」的系统提示。
            </em>
          </span>
        </label>

        <label className="setting setting-block">
          <span>
            <b>BT 走哪张网卡</b>
            <em>
              开着全局 VPN / 规则代理（TUN 模式）时，默认路由指向 <code>utun*</code>，
              <b>BT 流量也会跟着走隧道</b>。后果不是慢一点，是结构性的：隧道出口
              多半是机房 IP，会被大量 BT 客户端和 tracker 屏蔽（表现为连得上、
              握手立刻被断）；UPnP 的多播出不了隧道，端口映射必然失败，
              <b>没有入站连接，做种就是无效劳动</b>。
              <br />
              绑到物理网卡就能绕过默认路由直出，而<b>完全不动 VPN 本身</b>
              —— 浏览器照旧走隧道。<b>默认就绑第一张物理网卡</b>（macOS 上
              是 <code>en0</code>），不用每次手动选；只有想绑特定接口时才
              需要动这个下拉。改完<b>需要重启 App</b>。
            </em>
          </span>
          {IS_MAC ? (
            <select
              className="setting-select"
              value={draft.bindDevice ?? ""}
              onChange={(e) => patch({ bindDevice: e.target.value || null })}
            >
              <option value="">自动挑一张物理网卡（绕过 VPN 隧道，默认）</option>
              {/* 有人装 VPN 恰恰是为了让 BT 走它。不留这条路的话，升级之后
                  他们会静默用真实 IP 裸奔，而且没有任何办法要回原来的行为
                  —— 选某条 utun* 不等价，隧道重连后编号会变。 */}
              <option value={FOLLOW_SYSTEM_ROUTE}>
                跟随系统默认路由（有 VPN 时 BT 也走 VPN）
              </option>
              {ifaces.map((i) => (
                <option key={i.name} value={i.name}>
                  {i.name}
                  {i.ipv4 ? ` — ${i.ipv4}` : ""}
                  {i.isTunnel ? "（隧道，绑了等于没绕过去）" : ""}
                </option>
              ))}
            </select>
          ) : (
            <p className="setting-unsupported">
              Windows 上不可用 —— librqbit 的绑定网卡只实现了 macOS 的
              <code> IP_BOUND_IF </code>和 Linux 的<code> SO_BINDTODEVICE </code>，
              Windows 分支直接返回不支持。想让 BT 绕过 VPN，只能在代理客户端的
              规则里给 mydl 加一条直连。
            </p>
          )}
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.adaptiveUploadLimit}
            onChange={(e) => patch({ adaptiveUploadLimit: e.target.checked })}
          />
          <span>
            <b>自动调整上传限速</b>
            <em>
              每 5 秒测一次到网关的延迟：<b>排队延迟涨起来就退让，稳了就往上爬</b>。
              补的是 <code>librqbit-utp</code> 缺的 LEDBAT —— 它用 CUBIC，抢带宽和
              普通 TCP 一样凶。
              <br />
              手填固定值的问题是<b>必须按最坏情况填</b>，没人开会时也跑不满。开启后
              上面那个上传限速变成<b>天花板</b>而不是固定值。
              <br />
              只在<b>我们自己在满负荷上传时</b>才降速 —— 别人占带宽或 Wi-Fi 抖动
              不该让我们白白限死自己。
            </em>
          </span>
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.usePublicTrackers}
            onChange={(e) => patch({ usePublicTrackers: e.target.checked })}
          />
          <span>
            <b>为所有任务补充公共 tracker</b>
            <em>
              只有裸 info-hash 的磁力链不带 tracker，纯靠 DHT 找源；补上公共
              tracker 能多一条路。代价是<b>你的 IP 会被上报给这几个 tracker</b>，
              所有任务都会。改完<b>需要重启 App</b> 才生效。
            </em>
          </span>
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.swarmHealthCheck}
            onChange={(e) => patch({ swarmHealthCheck: e.target.checked })}
          />
          <span>
            <b>记录 swarm 健康度走势</b>
            <em>
              每 {draft.swarmHealthIntervalMinutes} 分钟向公共 tracker 查一次各任务的
              做种/下载人数，攒成曲线，展开任务时显示。用来判断一个种子是
              <b>还在变好还是已经没人做了</b> —— 卡住时不用再靠猜。
              会把 info-hash 发给这几个 tracker（只是查询，不汇报你在下载），
              <b>关掉就一个包都不发</b>。改完立刻生效。
            </em>
          </span>
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.uploadLimitKbps !== null}
            onChange={(e) => patch({ uploadLimitKbps: e.target.checked ? DEFAULT_UPLOAD_LIMIT : null })}
          />
          <span>
            <b>限制上传速度</b>
            <em>
              不限速时上行会被占满，同一条线路上刷网页、开会都会跟着卡 ——
              BT 的 uTP 本该给其他流量让路，但 librqbit 现在用的是 CUBIC，不会让。
              <b>做种和下载中的任务都在上传</b>，所以两种情况都受限。填太低会连带
              拖慢下载（BT 靠上传换下载），低于 32 就明显了。改完<b>立刻生效</b>。
            </em>
          </span>
        </label>

        {draft.uploadLimitKbps !== null && (
          <div className="setting-sub">
            <input
              type="number"
              min={1}
              step={32}
              value={draft.uploadLimitKbps}
              disabled={saving}
              onChange={(e) =>
                patch({ uploadLimitKbps: Math.max(0, Math.floor(Number(e.target.value) || 0)) })
              }
            />
            <span className="path">KB/s（限的是上传；下载中的任务也在上传）</span>
          </div>
        )}

        {draft.uploadLimitKbps !== null && (
          <div className="setting-sub">
            <input
              type="number"
              min={1}
              step={256}
              value={draft.downloadLimitKbps ?? 0}
              disabled={saving}
              onChange={(e) =>
                patch({
                  downloadLimitKbps:
                    Math.max(0, Math.floor(Number(e.target.value) || 0)) || null,
                })
              }
            />
            <span className="path">KB/s 下载限速（0 = 不限；很少需要，除非在共享网络里）</span>
          </div>
        )}

        <div className="setting setting-block">
          <span>
            <b>SOCKS5 代理</b>
            <em>
              格式 <code>socks5://[用户名:密码@]主机:端口</code>。
              <b>只代理出站 TCP 连接</b> —— DHT、uTP、UDP tracker 走的是 UDP，
              代理不了，仍然直连。所以这**不等于「BT 全程匿名」**，别拿它当
              隐私保障的全部。改完<b>需要重启 App</b>。
            </em>
            <input
              className="setting-input"
              type="text"
              spellCheck={false}
              placeholder="留空 = 不用代理"
              value={draft.proxyUrl ?? ""}
              onChange={(e) => patch({ proxyUrl: e.target.value.trim() || null })}
            />
          </span>
        </div>

        <div className="setting setting-block">
          <span>
            <b>IP 黑名单</b>
            <em>
              一个列表文件的地址，会话启动时拉取，命中的 IP 不再连接。
              常见来源是 iblocklist 那类公开列表。改完<b>需要重启 App</b>。
            </em>
            <input
              className="setting-input"
              type="text"
              spellCheck={false}
              placeholder="留空 = 不启用"
              value={draft.blocklistUrl ?? ""}
              onChange={(e) => patch({ blocklistUrl: e.target.value.trim() || null })}
            />
          </span>
        </div>

        <div className="setting setting-block">
          <span>
            <b>每个任务的 peer 上限</b>
            <em>
              留空用 librqbit 的默认值。弱网或老路由器上连接数太多会打爆 NAT
              表，表现为整个网络变卡。改完<b>需要重启 App</b>。
            </em>
            <input
              className="setting-input"
              type="number"
              min={1}
              placeholder="留空 = 默认"
              value={draft.peerLimit ?? ""}
              onChange={(e) =>
                patch({ peerLimit: Math.max(0, Math.floor(Number(e.target.value) || 0)) || null })
              }
            />
          </span>
        </div>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.notifyOnComplete}
            onChange={(e) => patch({ notifyOnComplete: e.target.checked })}
          />
          <span>
            <b>下载完成时发系统通知</b>
            <em>通知里会说明内容最终放在哪里。</em>
          </span>
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.soundOnComplete}
            onChange={(e) => patch({ soundOnComplete: e.target.checked })}
          />
          <span>
            <b>下载完成时播提示音</b>
            <em>
              和通知是两件事：通知权限被拒、或者开了勿扰，系统通知不会响，
              而这一声是 App 自己播的，照样听得见。用的是系统自带提示音，
              没有额外打包音频。
            </em>
          </span>
        </label>

        {draft.soundOnComplete && (
          <div className="setting-sub">
            <button
              disabled={saving}
              onClick={() => {
                invoke("play_done_sound").catch((e) => onError(String(e)));
              }}
            >
              试听
            </button>
            <span className="path">不用等任务下完就能听到是什么声</span>
          </div>
        )}

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.extractArchives}
            onChange={(e) => patch({ extractArchives: e.target.checked })}
          />
          <span>
            <b>自动解压 .zip</b>
            <em>解到同名子目录，原压缩包保留。rar / 7z 需要外部工具，暂不支持。</em>
          </span>
        </label>

        <div className="setting setting-block">
          <span>
            <b>同时最多下载几个</b>
            <em>
              超出的自动排队，前面下完再放出来。做种不占名额。
              <b>只会恢复它自己暂停的那些</b> —— 你手动暂停的任务不会被擅自放出来。
              这份记录只在内存里，重启后排队中的任务要手动继续。留空 = 不限。
            </em>
            <input
              className="setting-input"
              type="number"
              min={1}
              placeholder="留空 = 不限"
              value={draft.maxActiveDownloads ?? ""}
              onChange={(e) =>
                patch({
                  maxActiveDownloads:
                    Math.max(0, Math.floor(Number(e.target.value) || 0)) || null,
                })
              }
            />
          </span>
        </div>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.seedRatioLimit !== null}
            onChange={(e) => patch({ seedRatioLimit: e.target.checked ? 2 : null })}
          />
          <span>
            <b>分享率到顶就停止做种</b>
            <em>
              上传量达到文件大小的这个倍数就自动暂停。<b>分享率每次重启会归零</b> ——
              librqbit 只统计本次会话的上传量，不持久化，所以这个值的实际含义是
              「本次运行期间上传到几倍」，不是 PT 站看到的那个累计分享率。
            </em>
          </span>
        </label>

        {draft.seedRatioLimit !== null && (
          <div className="setting-sub">
            <input
              type="number"
              min={0.1}
              step={0.5}
              value={draft.seedRatioLimit}
              disabled={saving}
              onChange={(e) =>
                patch({ seedRatioLimit: Math.max(0, Number(e.target.value) || 0) || null })
              }
            />
            <span className="path">倍（2 = 上传量达到文件大小的两倍）</span>
          </div>
        )}

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.sleepWhenAllDone}
            onChange={(e) => patch({ sleepWhenAllDone: e.target.checked })}
          />
          <span>
            <b>全部下完后让电脑睡眠</b>
            <em>
              只在「这一轮真的有任务完成、且完成后一个未完成的都不剩」时触发，
              睡前等 20 秒（给通知留时间，也等阻止休眠的开关松手）。
              等待期间有新任务进来就取消。醒来后不会立刻又睡回去。
            </em>
          </span>
        </label>

        <label className="setting">
          <input
            type="checkbox"
            checked={draft.moveTo !== null}
            onChange={(e) => patch({ moveTo: e.target.checked ? "" : null })}
          />
          <span>
            <b>完成后移动到指定目录</b>
            <em>
              移动之后 librqbit 就找不到文件了，所以会把任务从列表移除（文件保留）
              —— 也就是<b>不再做种</b>。目标已有同名内容时会自动加后缀，不会覆盖。
            </em>
          </span>
        </label>

        {draft.moveTo !== null && (
          <div className="setting-sub">
            <button onClick={pickMoveTo} disabled={saving}>
              选择目录…
            </button>
            <span className="path" title={draft.moveTo || undefined}>
              {draft.moveTo || "尚未选择"}
            </span>
          </div>
        )}

        <div className="dialog-actions">
          <button onClick={onClose} disabled={saving}>
            取消
          </button>
          <button
            className="primary"
            disabled={
              saving ||
              (draft.moveTo !== null && draft.moveTo.trim() === "") ||
              // 0 在后端等于不限速，别让界面上写着「限制」实际却没限。
              draft.uploadLimitKbps === 0
            }
            onClick={save}
          >
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
