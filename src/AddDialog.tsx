import { useEffect, useMemo, useState } from "react";

import type { TorrentPreview } from "./types";
import { formatBytes } from "./format";

interface Props {
  preview: TorrentPreview;
  targetDir: string;
  busy: boolean;
  onConfirm: (files: number[]) => void;
  onCancel: () => void;
}

/// 常见的、几乎肯定用不上的附带文件。默认仍然勾选 —— 猜错了比让用户
/// 多点两下更烦，所以只提供一个「只要媒体文件」的快捷方式。
const MEDIA_ONLY_HINT = "只勾选视频和音频文件";

export default function AddDialog({
  preview,
  targetDir,
  busy,
  onConfirm,
  onCancel,
}: Props) {
  const [chosen, setChosen] = useState<Set<number>>(
    () => new Set(preview.files.map((f) => f.index)),
  );

  // Esc 关闭。对话框会挡住整个界面，必须给个不用鼠标的出口。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onCancel]);

  const chosenBytes = useMemo(
    () =>
      preview.files.reduce((sum, f) => (chosen.has(f.index) ? sum + f.len : sum), 0),
    [preview.files, chosen],
  );

  const hasMedia = preview.files.some((f) => f.playable);

  function toggle(index: number) {
    setChosen((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }

  return (
    <div className="overlay" onClick={() => !busy && onCancel()}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title" title={preview.infoHash}>
          {preview.name}
        </h2>

        <p className="dialog-sub">
          共 {preview.files.length} 个文件，{formatBytes(preview.totalBytes)}
          　→　下载到 {targetDir}
        </p>

        {preview.alreadyAdded && (
          <p className="dialog-warn">
            这个种子已经在任务列表里了。继续会沿用已有任务，只更新文件选择。
          </p>
        )}

        <div className="dialog-toolbar">
          <button
            disabled={busy}
            onClick={() => setChosen(new Set(preview.files.map((f) => f.index)))}
          >
            全选
          </button>
          {hasMedia && (
            <button
              disabled={busy}
              title={MEDIA_ONLY_HINT}
              onClick={() =>
                setChosen(
                  new Set(preview.files.filter((f) => f.playable).map((f) => f.index)),
                )
              }
            >
              只要媒体
            </button>
          )}
          <span className="spacer" />
          <span>
            已选 {chosen.size}/{preview.files.length}，{formatBytes(chosenBytes)}
          </span>
        </div>

        <ul className="dialog-files">
          {preview.files.map((f) => (
            <li key={f.index} className={chosen.has(f.index) ? "" : "file-skipped"}>
              <input
                type="checkbox"
                checked={chosen.has(f.index)}
                disabled={busy}
                onChange={() => toggle(f.index)}
              />
              <span className="file-name" title={f.name}>
                {f.name}
              </span>
              <span className="file-size">{formatBytes(f.len)}</span>
            </li>
          ))}
        </ul>

        <div className="dialog-actions">
          <button onClick={onCancel} disabled={busy}>
            取消
          </button>
          <button
            className="primary"
            disabled={busy || chosen.size === 0}
            onClick={() => onConfirm([...chosen])}
          >
            {busy
              ? "添加中…"
              : chosen.size === preview.files.length
                ? "全部下载"
                : `下载选中的 ${chosen.size} 个`}
          </button>
        </div>
      </div>
    </div>
  );
}
