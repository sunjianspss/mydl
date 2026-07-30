import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { Settings } from "./types";

interface Props {
  initial: Settings;
  onSaved: (settings: Settings) => void;
  onClose: () => void;
  onError: (message: string) => void;
}

export default function SettingsDialog({ initial, onSaved, onClose, onError }: Props) {
  const [draft, setDraft] = useState<Settings>(initial);
  const [saving, setSaving] = useState(false);

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
            checked={draft.extractArchives}
            onChange={(e) => patch({ extractArchives: e.target.checked })}
          />
          <span>
            <b>自动解压 .zip</b>
            <em>解到同名子目录，原压缩包保留。rar / 7z 需要外部工具，暂不支持。</em>
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
            disabled={saving || (draft.moveTo !== null && draft.moveTo.trim() === "")}
            onClick={save}
          >
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
