import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import type { Settings, TorrentPreview, TorrentView } from "./types";
import { formatBytes, formatSpeed, percent } from "./format";
import FileList from "./FileList";
import AddDialog from "./AddDialog";
import "./App.css";

const POLL_INTERVAL_MS = 1000;

const STATE_LABEL: Record<string, string> = {
  initializing: "准备中",
  live: "下载中",
  paused: "已暂停",
  error: "出错",
};

export default function App() {
  const [torrents, setTorrents] = useState<TorrentView[]>([]);
  const [uri, setUri] = useState("");
  const [outputFolder, setOutputFolder] = useState<string | null>(null);
  const [defaultDir, setDefaultDir] = useState("");
  // 只给添加流程用。磁力链要先解析元信息，可能要等几十秒，不能因此把
  // 整个工具栏锁死 —— 之前一个卡住的操作会让界面看起来像死了。
  const [adding, setAdding] = useState(false);
  // 解析出来待确认的种子；null 表示没有对话框。
  const [preview, setPreview] = useState<TorrentPreview | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 哪一行正处于「确认删除」状态。用行内确认而不是系统弹窗，避免阻塞 webview。
  const [confirmingDelete, setConfirmingDelete] = useState<number | null>(null);
  // 展开了文件列表的任务。
  const [expanded, setExpanded] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setTorrents(await invoke<TorrentView[]>("list_torrents"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    invoke<string>("default_download_dir").then(setDefaultDir).catch(() => {});
    // 上次选的目录存在 Rust 侧，重启后要读回来，否则会静悄悄地下到别处去。
    invoke<Settings>("get_settings")
      .then((s) => setOutputFolder(s.downloadDir))
      .catch(() => {});
    refresh();
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

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
      setPreview(await invoke<TorrentPreview>("preview_torrent", { uri: value }));
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

  return (
    <main className="app">
      <header className="toolbar">
        <form
          className="add-row"
          onSubmit={(e) => {
            e.preventDefault();
            if (uri.trim()) addTorrent(uri);
          }}
        >
          <input
            className="uri-input"
            value={uri}
            placeholder="粘贴磁力链或种子地址…"
            onChange={(e) => setUri(e.target.value)}
            spellCheck={false}
          />
          <button type="submit" disabled={adding || !uri.trim()}>
            {adding ? "解析中…" : "添加"}
          </button>
          <button type="button" onClick={pickTorrentFile} disabled={adding}>
            打开种子…
          </button>
        </form>

        {adding && (
          <p className="adding-hint">
            正在解析…磁力链需要先从其他 peer 拿到文件列表，最多等 2 分钟。
          </p>
        )}

        {preview && (
          <AddDialog
            preview={preview}
            targetDir={outputFolder ?? defaultDir}
            busy={confirming}
            onConfirm={confirmAdd}
            onCancel={() => setPreview(null)}
          />
        )}

        <div className="meta-row">
          <button type="button" className="link" onClick={pickOutputFolder}>
            下载到：{outputFolder ?? (defaultDir || "…")}
          </button>
          {outputFolder && (
            <button type="button" className="link" onClick={() => changeOutputFolder(null)}>
              恢复默认
            </button>
          )}
          <span className="spacer" />
          <button
            type="button"
            className="link"
            title="出问题时把日志目录翻出来"
            onClick={() =>
              run(async () => revealItemInDir(await invoke<string>("log_dir")))
            }
          >
            日志
          </button>
          <span className="totals">
            ↓ {formatSpeed(totalDown)}　↑ {formatSpeed(totalUp)}
          </span>
        </div>

        {error && (
          <div className="error-banner" onClick={() => setError(null)} title="点击关闭">
            {error}
          </div>
        )}
      </header>

      <section className="list">
        {torrents.length === 0 && (
          <p className="empty">还没有任务。粘贴一个磁力链，或打开本地 .torrent 文件。</p>
        )}
        {torrents.map((t) => (
          <TorrentRow
            key={t.id}
            torrent={t}
            expanded={expanded === t.id}
            onToggleExpand={() => setExpanded(expanded === t.id ? null : t.id)}
            onError={setError}
            confirming={confirmingDelete === t.id}
            onConfirmDelete={() => setConfirmingDelete(t.id)}
            onCancelDelete={() => setConfirmingDelete(null)}
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
  onPause,
  onResume,
  onDelete,
  onReveal,
}: RowProps) {
  const pct = percent(t.progressBytes, t.totalBytes);
  const paused = t.state === "paused";

  return (
    <article className={`row state-${t.state}`}>
      {/* 整行可点：光靠那个小三角太难发现。 */}
      <div
        className="row-head"
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
        <span className="disclosure">{expanded ? "▾" : "▸"}</span>
        <span className="name" title={t.infoHash}>
          {t.name}
        </span>
        <span className={`badge badge-${t.state}`}>
          {t.finished && t.state === "live" ? "做种中" : (STATE_LABEL[t.state] ?? t.state)}
        </span>
      </div>

      <div className="bar">
        <div className="bar-fill" style={{ width: `${pct}%` }} />
      </div>

      <div className="row-stats">
        <span className="pct">{pct.toFixed(1)}%</span>
        <span>
          {formatBytes(t.progressBytes)} / {formatBytes(t.totalBytes)}
        </span>
        <span>↓ {formatSpeed(t.downloadSpeedBps)}</span>
        <span>↑ {formatSpeed(t.uploadSpeedBps)}</span>
        <span>{t.peersLive} peers</span>
        {t.eta && !t.finished && <span>剩余 {t.eta}</span>}
        <span className="spacer" />
        {confirming ? (
          <>
            <span className="confirm-label">确认删除？</span>
            <button onClick={() => onDelete(false)}>仅移除任务</button>
            <button className="danger" onClick={() => onDelete(true)}>
              连同文件
            </button>
            <button onClick={onCancelDelete}>取消</button>
          </>
        ) : (
          <>
            <button onClick={paused ? onResume : onPause}>{paused ? "继续" : "暂停"}</button>
            <button onClick={onReveal}>在访达中显示</button>
            <button className="danger" onClick={onConfirmDelete}>
              删除
            </button>
          </>
        )}
      </div>

      {t.error && <p className="row-error">{t.error}</p>}

      {expanded && (
        <FileList torrentId={t.id} streamable={t.state === "live"} onError={onError} />
      )}
    </article>
  );
}
