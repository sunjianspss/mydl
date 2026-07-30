import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import type { TorrentView } from "./types";
import { formatBytes, formatSpeed, percent } from "./format";
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 哪一行正处于「确认删除」状态。用行内确认而不是系统弹窗，避免阻塞 webview。
  const [confirmingDelete, setConfirmingDelete] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setTorrents(await invoke<TorrentView[]>("list_torrents"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    invoke<string>("default_download_dir").then(setDefaultDir).catch(() => {});
    refresh();
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      setBusy(true);
      setError(null);
      try {
        await action();
        await refresh();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const addTorrent = (value: string) =>
    run(async () => {
      await invoke("add_torrent", { uri: value, outputFolder });
      setUri("");
    });

  async function pickTorrentFile() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Torrent", extensions: ["torrent"] }],
    });
    if (typeof picked === "string") await addTorrent(picked);
  }

  async function pickOutputFolder() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setOutputFolder(picked);
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
          <button type="submit" disabled={busy || !uri.trim()}>
            添加
          </button>
          <button type="button" onClick={pickTorrentFile} disabled={busy}>
            打开种子…
          </button>
        </form>

        <div className="meta-row">
          <button type="button" className="link" onClick={pickOutputFolder}>
            下载到：{outputFolder ?? (defaultDir || "…")}
          </button>
          {outputFolder && (
            <button type="button" className="link" onClick={() => setOutputFolder(null)}>
              恢复默认
            </button>
          )}
          <span className="spacer" />
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
      <div className="row-head">
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
    </article>
  );
}
