import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { FileView } from "./types";
import { formatBytes, percent } from "./format";
import { useWindowFocused } from "./useWindowFocused";

const POLL_INTERVAL_MS = 2000;

interface Props {
  torrentId: number;
  /** 任务不在下载中时不能起播：暂停状态没有新数据，播放器只会卡住。 */
  streamable: boolean;
  onError: (message: string) => void;
}

export default function FileList({ torrentId, streamable, onError }: Props) {
  const [files, setFiles] = useState<FileView[] | null>(null);
  const [copied, setCopied] = useState<number | null>(null);
  const [saving, setSaving] = useState(false);
  // 每次展开都重查一遍已装播放器。放在启动时查过一次就不管的话，
  // 之后新装的播放器要重启 App 才认得出来。
  const [players, setPlayers] = useState<string[]>([]);

  useEffect(() => {
    invoke<string[]>("available_players").then(setPlayers).catch(() => {});
  }, []);

  const refresh = useCallback(async () => {
    try {
      setFiles(await invoke<FileView[]>("list_files", { id: torrentId }));
    } catch (e) {
      onError(String(e));
    }
  }, [torrentId, onError]);

  // 文件列表展开在任务行里，窗口失焦时一样该停 —— 后台没人在看进度。
  const windowFocused = useWindowFocused();

  useEffect(() => {
    if (!windowFocused) return;
    refresh();
    // 文件进度变化比任务整体慢，用更低的频率轮询。
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh, windowFocused]);

  /// 把整份选择集发给后端 —— 后端接口就是「只下这些」，不是增量操作。
  async function applySelection(next: FileView[]) {
    const chosen = next.filter((f) => f.selected).map((f) => f.index);
    if (chosen.length === 0) {
      onError("至少要选一个文件；一个都不要的话请直接删除任务");
      return;
    }

    // 先动界面，让勾选立刻有反馈；失败了再拉回真实状态。
    setFiles(next);
    setSaving(true);
    try {
      await invoke("set_only_files", { id: torrentId, files: chosen });
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
      await refresh();
    }
  }

  const toggle = (index: number) =>
    applySelection(
      (files ?? []).map((f) => (f.index === index ? { ...f, selected: !f.selected } : f)),
    );

  const setAll = (selected: boolean) =>
    applySelection((files ?? []).map((f) => ({ ...f, selected })));

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

  const chosen = files.filter((f) => f.selected);
  const chosenBytes = chosen.reduce((sum, f) => sum + f.len, 0);
  const totalBytes = files.reduce((sum, f) => sum + f.len, 0);

  return (
    <div className="files-panel">
      <div className="files-toolbar">
        <span>
          已选 {chosen.length}/{files.length} 个，{formatBytes(chosenBytes)}
          {chosen.length < files.length && ` / 共 ${formatBytes(totalBytes)}`}
        </span>
        <span className="spacer" />
        <button disabled={saving} onClick={() => setAll(true)}>
          全选
        </button>
        <button
          disabled={saving}
          title="留下第一个文件，其余取消 —— 后端不允许一个都不选"
          onClick={() =>
            applySelection(files.map((f, i) => ({ ...f, selected: i === 0 })))
          }
        >
          全不选
        </button>
      </div>

      <ul className="files">
        {files.map((f) => (
          <li key={f.index} className={`file${f.selected ? "" : " file-skipped"}`}>
            <input
              type="checkbox"
              checked={f.selected}
              disabled={saving}
              title={f.selected ? "取消后不再下载这个文件" : "勾选以下载这个文件"}
              onChange={() => toggle(f.index)}
            />
            <span className="file-name" title={f.name}>
              {f.name}
            </span>
            <span className="file-size">
              {f.selected ? (
                <>
                  {formatBytes(f.downloaded)} / {formatBytes(f.len)}
                  <span className="file-pct">（{percent(f.downloaded, f.len).toFixed(0)}%）</span>
                </>
              ) : (
                <>{formatBytes(f.len)}　已跳过</>
              )}
            </span>
            {f.playable && f.selected && (
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
    </div>
  );
}
