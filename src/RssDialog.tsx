import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { CheckReport, RssFeed, Settings } from "./types";

interface Props {
  initial: Settings;
  onSaved: (settings: Settings) => void;
  onClose: () => void;
  onError: (message: string) => void;
}

function newFeed(): RssFeed {
  return {
    // 只用来标识「这条订阅处理过哪些条目」，随机就够。
    id: crypto.randomUUID(),
    name: "",
    url: "",
    enabled: true,
    include: "",
    exclude: "",
  };
}

export default function RssDialog({ initial, onSaved, onClose, onError }: Props) {
  const [feeds, setFeeds] = useState<RssFeed[]>(initial.rssFeeds);
  const [interval, setIntervalMinutes] = useState(initial.rssIntervalMinutes);
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);
  const [reports, setReports] = useState<CheckReport[] | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !saving && !checking) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [saving, checking, onClose]);

  const patch = (id: string, p: Partial<RssFeed>) =>
    setFeeds((fs) => fs.map((f) => (f.id === id ? { ...f, ...p } : f)));

  function build(): Settings {
    // 空 URL 的条目直接丢掉，免得存一堆没填完的。
    return {
      ...initial,
      rssFeeds: feeds.filter((f) => f.url.trim() !== ""),
      rssIntervalMinutes: Math.max(5, interval || 5),
    };
  }

  async function save(): Promise<Settings | null> {
    const next = build();
    setSaving(true);
    try {
      await invoke("save_settings", { settings: next });
      onSaved(next);
      return next;
    } catch (e) {
      onError(String(e));
      return null;
    } finally {
      setSaving(false);
    }
  }

  /// 先存再查 —— 否则查的是改动之前的规则，结果对不上界面。
  async function checkNow() {
    if ((await save()) === null) return;
    setChecking(true);
    setReports(null);
    try {
      setReports(await invoke<CheckReport[]>("check_rss_now"));
    } catch (e) {
      onError(String(e));
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="overlay" onClick={() => !saving && !checking && onClose()}>
      <div className="dialog rss-dialog" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">RSS 订阅</h2>
        <p className="dialog-sub">
          命中规则的条目会自动加入下载。关键词用空格分隔、不区分大小写；
          <b>包含</b>要全部命中，<b>排除</b>命中任一即跳过。
        </p>

        <div className="rss-list">
          {feeds.length === 0 && <p className="files-hint">还没有订阅。</p>}
          {feeds.map((f) => (
            <div key={f.id} className={`rss-item${f.enabled ? "" : " rss-off"}`}>
              <div className="rss-row">
                <input
                  type="checkbox"
                  checked={f.enabled}
                  title={f.enabled ? "已启用" : "已停用"}
                  onChange={(e) => patch(f.id, { enabled: e.target.checked })}
                />
                <input
                  className="rss-name"
                  value={f.name}
                  placeholder="名称（可留空）"
                  onChange={(e) => patch(f.id, { name: e.target.value })}
                />
                <input
                  className="rss-url"
                  value={f.url}
                  placeholder="订阅地址 https://…"
                  spellCheck={false}
                  onChange={(e) => patch(f.id, { url: e.target.value })}
                />
                <button
                  className="danger"
                  title="删除这条订阅"
                  onClick={() => setFeeds((fs) => fs.filter((x) => x.id !== f.id))}
                >
                  删除
                </button>
              </div>
              <div className="rss-row rss-rules">
                <input
                  value={f.include}
                  placeholder="包含：例如  1080p 中文字幕"
                  onChange={(e) => patch(f.id, { include: e.target.value })}
                />
                <input
                  value={f.exclude}
                  placeholder="排除：例如  hdtv 预告"
                  onChange={(e) => patch(f.id, { exclude: e.target.value })}
                />
              </div>
            </div>
          ))}
        </div>

        <div className="rss-footer">
          <button onClick={() => setFeeds((fs) => [...fs, newFeed()])} disabled={saving}>
            添加订阅
          </button>
          <label className="rss-interval">
            每
            <input
              type="number"
              min={5}
              value={interval}
              onChange={(e) => setIntervalMinutes(Number(e.target.value))}
            />
            分钟检查一次（最少 5）
          </label>
        </div>

        {reports && (
          <ul className="rss-reports">
            {reports.length === 0 && <li>没有启用中的订阅。</li>}
            {reports.map((r, i) => (
              <li key={i}>
                <b>{r.feed}</b>：{r.total} 条，命中 {r.matched}，新增 {r.added}
                {r.errors.length > 0 && (
                  <span className="rss-error">　{r.errors.join("；")}</span>
                )}
              </li>
            ))}
          </ul>
        )}

        <div className="dialog-actions">
          <button onClick={checkNow} disabled={saving || checking}>
            {checking ? "检查中…" : "保存并立即检查"}
          </button>
          <span className="spacer" />
          <button onClick={onClose} disabled={saving || checking}>
            取消
          </button>
          <button
            className="primary"
            disabled={saving || checking}
            onClick={async () => {
              if ((await save()) !== null) onClose();
            }}
          >
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
