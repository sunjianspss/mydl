const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // 到 MB 以上才有必要看小数位。
  return `${value.toFixed(unit >= 2 ? 1 : 0)} ${UNITS[unit]}`;
}

export function formatSpeed(bytesPerSecond: number): string {
  if (bytesPerSecond < 1) return "—";
  return `${formatBytes(bytesPerSecond)}/s`;
}

export function percent(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, (done / total) * 100);
}
