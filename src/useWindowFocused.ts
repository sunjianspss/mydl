import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * 窗口当前是否有焦点。
 *
 * 轮询刷新只在有焦点时才值得做：窗口在后台时每秒全量拉一遍任务列表纯粹
 * 烧 CPU，等切回来再刷也不迟。失焦 → false，回焦 → true。
 */
export function useWindowFocused(): boolean {
  const [focused, setFocused] = useState(true);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    const win = getCurrentWindow();

    // onFocusChanged 只在状态变化时触发，初值要自己查一次。
    win
      .isFocused()
      .then(setFocused)
      .catch(() => {});
    win
      .onFocusChanged(({ payload }) => setFocused(payload))
      .then((fn) => (unlisten = fn))
      .catch(() => {});

    return () => {
      unlisten?.();
    };
  }, []);

  return focused;
}
