import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { Settings } from "./types";

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
