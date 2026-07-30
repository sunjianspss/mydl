/** 对应 src-tauri/src/engine.rs 的 TorrentView，字段名由 serde 转成 camelCase。 */
export type TorrentState = "initializing" | "live" | "paused" | "error";

export interface TorrentView {
  id: number;
  name: string;
  infoHash: string;
  state: TorrentState;
  error: string | null;
  finished: boolean;
  progressBytes: number;
  totalBytes: number;
  uploadedBytes: number;
  downloadSpeedBps: number;
  uploadSpeedBps: number;
  peersLive: number;
  eta: string | null;
}

/** 对应 settings.rs 的 Settings。 */
export interface Settings {
  /** null 表示跟随系统默认下载文件夹。 */
  downloadDir: string | null;
}

/** 对应 engine.rs 的 FileView。 */
export interface FileView {
  index: number;
  name: string;
  len: number;
  downloaded: number;
  playable: boolean;
  /** 是否在下载范围内。未选中的文件不会被请求。 */
  selected: boolean;
}
