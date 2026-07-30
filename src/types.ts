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
