import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { FileView } from "./types";
import { formatBytes, percent } from "./format";

const POLL_INTERVAL_MS = 2000;

interface Props {
  torrentId: number;
  players: string[];
  /** 任务不在下载中时不能起播：暂停状态没有新数据，播放器只会卡住。 */
  streamable: boolean;
  onError: (message: string) => void;
}

export default function FileList({ torrentId, players, streamable, onError }: Props) {
  const [files, setFiles] = useState<FileView[] | null>(null);
  const [copied, setCopied] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setFiles(await invoke<FileView[]>("list_files", { id: torrentId }));
    } catch (e) {
      onError(String(e));
    }
  }, [torrentId, onError]);

  useEffect(() => {
    refresh();
    // 文件进度变化比任务整体慢，用更低的频率轮询。
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  async function play(file: FileView, app: string) {
    try {
      const url = await invoke<string>("stream_url", {
        id: torrentId,
        fileId: file.index,
      });
      await invoke("open_in_player", { url, app });
    } catch (e) {
      onError(String(e));
    }
  }

  async function copyLink(file: FileView) {
    try {
      const url = await invoke<string>("stream_url", {
        id: torrentId,
        fileId: file.index,
      });
      await navigator.clipboard.writeText(url);
      setCopied(file.index);
      setTimeout(() => setCopied(null), 1500);
    } catch (e) {
      onError(String(e));
    }
  }

  if (files === null) return <p className="files-hint">读取文件列表…</p>;
  if (files.length === 0)
    return <p className="files-hint">还没拿到种子元信息，稍候…</p>;

  return (
    <ul className="files">
      {files.map((f) => (
        <li key={f.index} className="file">
          <span className="file-name" title={f.name}>
            {f.name}
          </span>
          <span className="file-size">
            {formatBytes(f.downloaded)} / {formatBytes(f.len)}
            <span className="file-pct">（{percent(f.downloaded, f.len).toFixed(0)}%）</span>
          </span>
          {f.playable && (
            <span className="file-actions">
              {players.map((app) => (
                <button
                  key={app}
                  disabled={!streamable}
                  title={streamable ? `用 ${app} 边下边播` : "任务不在下载中，无法播放"}
                  onClick={() => play(f, app)}
                >
                  {app}
                </button>
              ))}
              <button
                disabled={!streamable}
                title={streamable ? "复制流地址，可粘到播放器的「打开网络串流」" : ""}
                onClick={() => copyLink(f)}
              >
                {copied === f.index ? "已复制" : "复制链接"}
              </button>
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
