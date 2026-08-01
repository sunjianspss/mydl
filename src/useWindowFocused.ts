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
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;
    const win = getCurrentWindow();

    // onFocusChanged 只在状态变化时触发，初值要自己查一次。
    win
      .isFocused()
      .then(setFocused)
      .catch(() => {});
    // 注册是异步的，effect 可能在 Promise 落地**之前**就卸载 —— 收起文件列表、
    // 或 StrictMode 下 effect 双跑都会。那时 unlisten 还是 null，清理函数摘了个
    // 空，之后监听器才注册上、从此再也摘不掉。cancelled 兜住这段时间差。
    win
      .onFocusChanged(({ payload }) => setFocused(payload))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return focused;
}
