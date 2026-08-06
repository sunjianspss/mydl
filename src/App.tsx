import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { readText } from "@tauri-apps/plugin-clipboard-manager";

import type { SessionStatus, Settings, TorrentPreview, TorrentView } from "./types";
import { formatBytes, formatSpeed, percent } from "./format";
import { useWindowFocused } from "./useWindowFocused";
import FileList from "./FileList";
import SwarmHealth from "./SwarmHealth";
import BetterSources from "./BetterSources";
import Diagnose from "./Diagnose";
import Verify from "./Verify";
import AddDialog from "./AddDialog";
import SettingsDialog from "./SettingsDialog";
import RssDialog from "./RssDialog";
import SearchDialog from "./SearchDialog";
import { getTheme, setTheme, type Theme } from "./theme";
import {
  CheckIcon,
  DocIcon,
  DownIcon,
  FolderIcon,
  GearIcon,
  MoonIcon,
  PauseAllIcon,
  PauseIcon,
  PauseSeedIcon,
  PlayAllIcon,
  PlayIcon,
  PlusIcon,
  RssIcon,
  SearchIcon,
  StackIcon,
  SunIcon,
  TrashIcon,
  TypeIcon,
  UpIcon,
  iconKindFor,
} from "./icons";
import "./App.css";

// 有任务在跑时轮询快一点，速度数字动得跟手；空闲时没必要刷那么勤。
const POLL_ACTIVE_MS = 1000;
const POLL_IDLE_MS = 5000;

const STATE_LABEL: Record<string, string> = {
  initializing: "准备中",
  live: "下载中",
  paused: "已暂停",
  error: "出错",
};

/** 侧边栏的分类。计数和过滤共用同一个判定，不会出现「写着 2 个点进去只有 1 个」。 */
type Filter = "all" | "downloading" | "seeding" | "done" | "paused";

type IconComponent = (p: { className?: string }) => React.ReactElement;

const FILTERS: { id: Filter; label: string; icon: IconComponent }[] = [
  { id: "all", label: "全部任务", icon: StackIcon },
  { id: "downloading", label: "下载中", icon: DownIcon },
  { id: "seeding", label: "做种中", icon: UpIcon },
  { id: "done", label: "已完成", icon: CheckIcon },
  { id: "paused", label: "已暂停", icon: PauseIcon },
];

function matchesFilter(t: TorrentView, f: Filter): boolean {
  switch (f) {
    case "downloading":
      return t.state === "live" && !t.finished;
    case "seeding":
      return t.state === "live" && t.finished;
    case "done":
      return t.finished;
    case "paused":
      return t.state === "paused";
    default:
      return true;
  }
}

export default function App() {
  const [torrents, setTorrents] = useState<TorrentView[]>([]);
  const [status, setStatus] = useState<SessionStatus | null>(null);
  const [uri, setUri] = useState("");
  const [outputFolder, setOutputFolder] = useState<string | null>(null);
  const [defaultDir, setDefaultDir] = useState("");
  // 只给添加流程用。磁力链要先解析元信息，可能要等几十秒，不能因此把
  // 整个工具栏锁死 —— 之前一个卡住的操作会让界面看起来像死了。
  const [adding, setAdding] = useState(false);
  // 解析出来待确认的种子；null 表示没有对话框。
  const [preview, setPreview] = useState<TorrentPreview | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showRss, setShowRss] = useState(false);
  const [showSearch, setShowSearch] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 哪一行正处于「确认删除」状态。用行内确认而不是系统弹窗，避免阻塞 webview。
  const [confirmingDelete, setConfirmingDelete] = useState<number | null>(null);
  // 展开了文件列表的任务。
  const [expanded, setExpanded] = useState<number | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  // 拖着 .torrent 悬在窗口上时给个视觉反馈。
  const [dragging, setDragging] = useState(false);
  // 剪贴板里发现的、还没处理过的磁力链。null = 不显示横幅。
  const [clipMagnet, setClipMagnet] = useState<string | null>(null);
  // 主题存在 localStorage 里，theme.ts 在首帧前就应用好了，这里只是拿来渲染图标。
  const [theme, setThemeState] = useState<Theme>(getTheme);
  // 来自 tauri.conf.json，也就是 .app 实际打包出来的版本号。不在前端另存一份，
  // 否则迟早和 Cargo.toml / package.json 对不上，报 bug 时给的版本就是错的。
  const [version, setVersion] = useState("");

  const refresh = useCallback(async () => {
    try {
      setTorrents(await invoke<TorrentView[]>("list_torrents"));
      setStatus(await invoke<SessionStatus>("session_status"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    invoke<string>("default_download_dir").then(setDefaultDir).catch(() => {});
    // 上次选的目录存在 Rust 侧，重启后要读回来，否则会静悄悄地下到别处去。
    invoke<Settings>("get_settings")
      .then((s) => {
        setSettings(s);
        setOutputFolder(s.downloadDir);
      })
      .catch(() => {});
    refresh();
  }, [refresh]);

  // 轮询只在窗口有焦点时进行：后台每秒全量拉一遍列表纯属烧 CPU，切回来
  // 立即刷一次比后台空转强得多。有任务在跑时刷新快些，全都停着就放慢。
  const windowFocused = useWindowFocused();
  // 下载和做种都算「在动」（live 覆盖两者，finished 只区分是哪一种）：做种时
  // 上传速度一样在变，落到 5s 档会看着一顿一顿的。
  const hasActive = torrents.some((t) => t.state === "live");
  const pollMs = windowFocused ? (hasActive ? POLL_ACTIVE_MS : POLL_IDLE_MS) : null;

  useEffect(() => {
    if (pollMs === null) return;
    refresh();
    const timer = setInterval(refresh, pollMs);
    return () => clearInterval(timer);
  }, [refresh, pollMs]);

  // 停轮询还不够：状态圆点的扩散动画是 infinite 的，窗口在后台也照样让 webview
  // 一帧帧合成，把省下来的 CPU 又还回去。挂个属性交给 CSS 停掉 —— 没人在看的
  // 时候扫给谁看。跟 theme.ts / platform.ts 一样用 :root 的 data 属性。
  useEffect(() => {
    document.documentElement.dataset.focused = String(windowFocused);
  }, [windowFocused]);

  // 拖 .torrent 文件进窗口就直接进预览。
  //
  // 只认文件：Tauri 接管了 webview 的原生拖放，回调里拿到的是**文件路径**，
  // 从浏览器拖过来的磁力链是文本、不会走到这里 —— 那条路交给剪贴板检测。
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "over" || e.payload.type === "enter") {
        setDragging(true);
        return;
      }
      setDragging(false);
      if (e.payload.type !== "drop") return;

      const torrent = e.payload.paths.find((p) => p.toLowerCase().endsWith(".torrent"));
      if (torrent) addTorrent(torrent);
      else setError("只能拖 .torrent 文件；磁力链请粘贴到输入框");
    });
    return () => {
      un.then((f) => f());
    };
    // addTorrent 每次渲染都是新的，但它只用 setState，放进依赖会反复重挂监听。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 切回窗口时看一眼剪贴板。只在获得焦点时读一次 —— 常驻轮询读剪贴板既让人
  // 不安，macOS 15 起还会弹系统提示。读到也只提示，绝不自动添加。
  useEffect(() => {
    if (!settings?.watchClipboard) return;

    const check = async () => {
      try {
        const text = (await readText())?.trim() ?? "";
        if (!/^magnet:\?xt=urn:btih:/i.test(text)) return;
        // 已经在任务列表里的就别再问了。
        const hash = text.slice(text.indexOf("btih:") + 5, text.indexOf("btih:") + 45).toLowerCase();
        if (torrents.some((t) => t.infoHash.toLowerCase().startsWith(hash.slice(0, 40)))) return;
        setClipMagnet(text);
      } catch {
        // 剪贴板里是图片之类的读不出来，忽略。
      }
    };

    window.addEventListener("focus", check);
    check();
    return () => window.removeEventListener("focus", check);
  }, [settings?.watchClipboard, torrents]);

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      setError(null);
      try {
        await action();
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [refresh],
  );

  /// 先解析出文件列表让用户勾选，确认后才真正加入会话。
  /// 预览时拿到的 torrent_bytes 会被后端缓存，确认时直接复用，
  /// 所以磁力链只解析这一次。
  async function addTorrent(value: string) {
    setAdding(true);
    setError(null);
    try {
      // null = 用户点了取消，安静收场，不当错误处理。
      const p = await invoke<TorrentPreview | null>("preview_torrent", { uri: value });
      if (p) setPreview(p);
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  }

  async function confirmAdd(files: number[]) {
    if (!preview) return;
    setConfirming(true);
    setError(null);
    try {
      await invoke("add_previewed", { token: preview.token, files, outputFolder });
      setPreview(null);
      setUri("");
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setConfirming(false);
    }
  }

  async function pickTorrentFile() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Torrent", extensions: ["torrent"] }],
    });
    if (typeof picked === "string") await addTorrent(picked);
  }

  async function pickOutputFolder() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") await changeOutputFolder(picked);
  }

  /// 先落盘再改界面，免得显示的和实际存的不一致。
  async function changeOutputFolder(dir: string | null) {
    try {
      await invoke("set_download_dir", { dir });
      setOutputFolder(dir);
    } catch (e) {
      setError(String(e));
    }
  }

  const totalDown = torrents.reduce((sum, t) => sum + t.downloadSpeedBps, 0);
  const totalUp = torrents.reduce((sum, t) => sum + t.uploadSpeedBps, 0);

  const counts = useMemo(() => {
    const c = {} as Record<Filter, number>;
    for (const f of FILTERS) c[f.id] = torrents.filter((t) => matchesFilter(t, f.id)).length;
    return c;
  }, [torrents]);

  const visible = torrents.filter((t) => matchesFilter(t, filter));
  const dir = outputFolder ?? defaultDir;

  return (
    <main className={`app${dragging ? " is-dragging" : ""}`}>
      {/* 标题栏被设成 Overlay，内容会延伸到红绿灯下面 —— 侧边栏顶部留白避让。
          这块同时是拖拽区：Overlay 之后系统标题栏被网页盖住，鼠标事件全被
          webview 吃掉，不显式标出来窗口就拖不动。 */}
      <aside className="sidebar">
        <div className="titlebar-gap" data-tauri-drag-region />

        <nav className="side-nav">
          {FILTERS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              className="side-item"
              aria-current={filter === id}
              onClick={() => setFilter(id)}
            >
              <Icon />
              <span className="side-name">{label}</span>
              {counts[id] > 0 && <span className="side-count">{counts[id]}</span>}
            </button>
          ))}
        </nav>

        <div className="side-label">订阅</div>
        <button className="side-item" onClick={() => setShowRss(true)} disabled={!settings}>
          <RssIcon />
          <span className="side-name">RSS 订阅</span>
          {settings && settings.rssFeeds.filter((f) => f.enabled).length > 0 && (
            <span className="side-count">
              {settings.rssFeeds.filter((f) => f.enabled).length}
            </span>
          )}
        </button>

        <div className="side-label">下载到</div>
        <button className="side-item" title={dir} onClick={pickOutputFolder}>
          <FolderIcon />
          <span className="side-name side-path">{dir.split("/").pop() || dir || "…"}</span>
        </button>
        {outputFolder && (
          <button className="side-reset" onClick={() => changeOutputFolder(null)}>
            恢复默认目录
          </button>
        )}
      </aside>

      <div className="main">
        {/* 上面那条 32px 留白也是拖拽区。属性只对事件目标本身生效，所以
            下面的输入框和按钮照常可点。 */}
        <header className="toolbar" data-tauri-drag-region>
          <form
            className="add-row"
            onSubmit={(e) => {
              e.preventDefault();
              if (uri.trim()) addTorrent(uri);
            }}
          >
            <button type="submit" className="tb-btn primary" disabled={adding || !uri.trim()}>
              <PlusIcon />
              {adding ? "解析中…" : "添加"}
            </button>
            <input
              className="tb-input"
              value={uri}
              placeholder="粘贴磁力链或种子地址…"
              onChange={(e) => setUri(e.target.value)}
              spellCheck={false}
            />
            <button type="button" className="tb-btn" onClick={pickTorrentFile} disabled={adding}>
              打开种子…
            </button>
            <button
              type="button"
              className="tb-icon"
              title="全部暂停"
              disabled={!torrents.some((t) => t.state === "live")}
              onClick={() => run(() => invoke("pause_all"))}
            >
              <PauseAllIcon />
            </button>
            {/* 上传带宽是全局一份预算，做种会把它吃光，下载中的任务就没有可
                回报给对方的上行，容易被 choke 到零速。「全部暂停」在这时候没用
                —— 它会把还在下的一起停掉。 */}
            <button
              type="button"
              className="tb-icon"
              title="暂停做种（下载中的任务不动）"
              disabled={counts.seeding === 0}
              onClick={() => run(() => invoke("pause_seeding"))}
            >
              <PauseSeedIcon />
            </button>
            <button
              type="button"
              className="tb-icon"
              title="全部继续"
              disabled={!torrents.some((t) => t.state === "paused")}
              onClick={() => run(() => invoke("resume_all"))}
            >
              <PlayAllIcon />
            </button>
            <button
              type="button"
              className="tb-icon"
              title="搜索种子"
              disabled={!settings}
              onClick={() => setShowSearch(true)}
            >
              <SearchIcon />
            </button>
            <button
              type="button"
              className="tb-icon"
              title={theme === "dark" ? "切换到浅色" : "切换到深色"}
              onClick={() => {
                const next: Theme = theme === "dark" ? "light" : "dark";
                setTheme(next);
                setThemeState(next);
              }}
            >
              {theme === "dark" ? <SunIcon /> : <MoonIcon />}
            </button>
            <button
              type="button"
              className="tb-icon"
              title="设置"
              disabled={!settings}
              onClick={() => setShowSettings(true)}
            >
              <GearIcon />
            </button>
          </form>

          {adding && (
            <p className="adding-hint">
              <span>
                正在解析…磁力链需要先从其他 peer 拿到文件列表，最多等 2 分钟。
              </span>
              <button
                type="button"
                className="act-text"
                onClick={() => invoke("cancel_preview").catch(() => {})}
              >
                取消
              </button>
            </p>
          )}

          {clipMagnet && (
            <div className="clip-banner">
              <span className="clip-text">剪贴板里有一条磁力链</span>
              <button
                className="act-text"
                onClick={() => {
                  const m = clipMagnet;
                  setClipMagnet(null);
                  addTorrent(m);
                }}
              >
                添加
              </button>
              <button className="act-text" onClick={() => setClipMagnet(null)}>
                忽略
              </button>
            </div>
          )}

          {error && (
            <div className="error-banner" onClick={() => setError(null)} title="点击关闭">
              {error}
            </div>
          )}
        </header>

        <section className="list">
          {visible.length === 0 && (
            <p className="empty">
              {torrents.length === 0
                ? "还没有任务。粘贴一个磁力链，或打开本地 .torrent 文件。"
                : "这个分类下没有任务。"}
            </p>
          )}
          {visible.map((t) => (
            <TorrentRow
              key={t.id}
              torrent={t}
              expanded={expanded === t.id}
              onToggleExpand={() => setExpanded(expanded === t.id ? null : t.id)}
              onError={setError}
              confirming={confirmingDelete === t.id}
              onConfirmDelete={() => setConfirmingDelete(t.id)}
              onCancelDelete={() => setConfirmingDelete(null)}
              onPick={addTorrent}
              onPause={() => run(() => invoke("pause_torrent", { id: t.id }))}
              onResume={() => run(() => invoke("resume_torrent", { id: t.id }))}
              onDelete={(deleteFiles) =>
                run(async () => {
                  await invoke("delete_torrent", { id: t.id, deleteFiles });
                  setConfirmingDelete(null);
                })
              }
              onReveal={() =>
                run(async () => {
                  const path = await invoke<string>("reveal_path", { id: t.id });
                  await revealItemInDir(path);
                })
              }
            />
          ))}
        </section>

        <footer className="statusbar">
          {/* 只写「N 个任务」会让下完在做种的看起来还在下 —— 这几个字曾经
              让人以为任务卡了好几天。分开数，和侧边栏用同一套判定。 */}
          <span className="sb-item" title={`共 ${torrents.length} 个任务`}>
            {counts.downloading} 下载 · {counts.seeding} 做种
          </span>
          <span className="sb-item">↓ {formatSpeed(totalDown)}</span>
          <span className="sb-item">↑ {formatSpeed(totalUp)}</span>
          {settings?.uploadLimitKbps ? (
            <span className="sb-item sb-limit" title="在设置里改，立刻生效">
              上传限速 {settings.uploadLimitKbps} KB/s
            </span>
          ) : null}
          <span className="spacer" />
          {status?.dhtNodes != null && (
            <span className="sb-item" title="DHT 路由表里的节点数">
              DHT {status.dhtNodes} 节点
            </span>
          )}
          {status?.listenPort != null && (
            <span className="sb-item" title="BT 监听端口（TCP + uTP）">
              端口 {status.listenPort}
            </span>
          )}
          {/* 绑定是建会话时定死的：网卡没了不会自动切换，BT 会静默停摆到重启
              为止。所以这条必须常驻可见，失效时还要变红 —— 否则症状和「没源」
              长得一模一样，没人会想到是网卡掉了。 */}
          {status?.bindDevice && (
            <span
              className={`sb-item${status.bindDeviceUp ? "" : " sb-broken"}`}
              title={
                status.bindDeviceUp
                  ? `BT 绑定在 ${status.bindDevice}，绕过系统默认路由（VPN 不受影响）`
                  : `网卡 ${status.bindDevice} 已经不在了 —— BT 连不上任何 peer。重启 App 会自动重选`
              }
            >
              网卡 {status.bindDevice}
              {!status.bindDeviceUp && " 已失效"}
            </span>
          )}
          <button
            className="sb-btn"
            title="出问题时把日志目录翻出来"
            onClick={() => run(async () => revealItemInDir(await invoke<string>("log_dir")))}
          >
            <DocIcon />
            日志
          </button>
          {version && (
            <span className="sb-item sb-version" title="mydl 版本，报问题时带上这个">
              v{version}
            </span>
          )}
        </footer>
      </div>

      {/* 对话框必须挂在工具栏外面：工具栏有 backdrop-filter，而带 backdrop-filter
          的元素会成为固定定位后代的包含块 —— 挂在里面的话 `position: fixed`
          的遮罩会被困在工具栏那一条里，对话框整个错位。 */}
      {showRss && settings && (
        <RssDialog
          initial={settings}
          onSaved={(s) => {
            setSettings(s);
            refresh();
          }}
          onClose={() => setShowRss(false)}
          onError={setError}
        />
      )}

      {showSettings && settings && (
        <SettingsDialog
          initial={settings}
          onSaved={setSettings}
          onClose={() => setShowSettings(false)}
          onError={setError}
        />
      )}

      {showSearch && settings && (
        <SearchDialog
          configured={!!settings.searchUrl}
          aiEnabled={settings.aiRank}
          onClose={() => setShowSearch(false)}
          onPick={(uri) => {
            setShowSearch(false);
            addTorrent(uri);
          }}
        />
      )}

      {preview && (
        <AddDialog
          preview={preview}
          targetDir={dir}
          busy={confirming}
          onConfirm={confirmAdd}
          onCancel={() => setPreview(null)}
        />
      )}
    </main>
  );
}

interface RowProps {
  torrent: TorrentView;
  expanded: boolean;
  onToggleExpand: () => void;
  onError: (message: string) => void;
  confirming: boolean;
  onConfirmDelete: () => void;
  onCancelDelete: () => void;
  onPick: (uri: string) => void;
  onPause: () => void;
  onResume: () => void;
  onDelete: (deleteFiles: boolean) => void;
  onReveal: () => void;
}

function TorrentRow({
  torrent: t,
  expanded,
  onToggleExpand,
  onError,
  confirming,
  onConfirmDelete,
  onCancelDelete,
  onPick,
  onPause,
  onResume,
  onDelete,
  onReveal,
}: RowProps) {
  const pct = percent(t.progressBytes, t.totalBytes);
  const paused = t.state === "paused";
  // 下完了还挂在 live 上就是在做种 —— 跟「正在下载」用不同颜色区分开，
  // 否则一眼看不出哪个任务还在耗带宽下东西。
  const seeding = t.state === "live" && t.finished;
  const kind = seeding ? "seeding" : t.state;
  const iconKind = iconKindFor(t.name, t.state === "error");

  return (
    <article className={`task state-${t.state}${seeding ? " is-seeding" : ""}`}>
      <div className={`type-icon type-${iconKind}`}>
        <TypeIcon kind={iconKind} />
      </div>

      <div className="task-body">
        {/* 整行标题可点：光靠那个小三角太难发现。 */}
        <div
          className="title-row"
          role="button"
          tabIndex={0}
          title={expanded ? "收起文件列表" : "展开文件列表"}
          onClick={onToggleExpand}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onToggleExpand();
            }
          }}
        >
          <span className="title">{t.name}</span>
          <span className={`state state-dot-${kind}`}>
            {seeding ? "做种中" : (STATE_LABEL[t.state] ?? t.state)}
          </span>
        </div>

        <div className="bar">
          <div className="bar-fill" style={{ width: `${pct}%` }} />
        </div>

        <div className="stats">
          <span className="pct">{pct.toFixed(1)}%</span>
          <span className="meta">
            <span>
              {formatBytes(t.progressBytes)} / {formatBytes(t.totalBytes)}
            </span>
            <span>↓ {formatSpeed(t.downloadSpeedBps)}</span>
            <span>↑ {formatSpeed(t.uploadSpeedBps)}</span>
            <span>{t.peersLive} peers</span>
            {t.eta && !t.finished && <span>剩余 {t.eta}</span>}
          </span>
          <span className="spacer" />

          {/* 确认删除时必须完全显形：这时候压暗等于把关键选择藏起来 */}
          <div className={`acts${confirming ? " is-confirming" : ""}`}>
            {confirming ? (
              <>
                <span className="confirm-label">确认删除？</span>
                <button className="act-text" onClick={() => onDelete(false)}>
                  仅移除任务
                </button>
                <button className="act-text danger" onClick={() => onDelete(true)}>
                  连同文件
                </button>
                <button className="act-text" onClick={onCancelDelete}>
                  取消
                </button>
              </>
            ) : (
              <>
                <button
                  className="act"
                  title={paused ? "继续下载" : "暂停"}
                  onClick={paused ? onResume : onPause}
                >
                  {paused ? <PlayIcon /> : <PauseIcon />}
                </button>
                <button className="act" title="在访达中显示" onClick={onReveal}>
                  <FolderIcon />
                </button>
                <button className="act danger" title="删除" onClick={onConfirmDelete}>
                  <TrashIcon />
                </button>
              </>
            )}
          </div>
        </div>

        {t.error && <p className="task-error">{t.error}</p>}

        {expanded && (
          <>
            <SwarmHealth infoHash={t.infoHash} onError={onError} />
            <BetterSources
              torrentId={t.id}
              progressBytes={t.progressBytes}
              onPick={onPick}
              onError={onError}
            />
            <Diagnose torrentId={t.id} onError={onError} />
            <Verify torrentId={t.id} onError={onError} />
            <FileList torrentId={t.id} streamable={t.state === "live"} onError={onError} />
          </>
        )}
      </div>
    </article>
  );
}
