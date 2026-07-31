/**
 * 深色 / 浅色主题。默认深色。
 *
 * 存在 localStorage 而不是 settings.json：主题必须在**首帧之前**同步定下来，
 * 走 Rust 命令的话会先按默认色渲染一帧再跳，切换时能看见闪白。代价是这项
 * 设置不跟着 settings.json 走，换机器要重设一次 —— 对一个显示偏好来说划算。
 */

export type Theme = "dark" | "light";

const KEY = "mydl.theme";

function stored(): Theme | null {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "light" ? v : null;
}

export function getTheme(): Theme {
    return stored() ?? "dark";
}

export function setTheme(theme: Theme) {
    localStorage.setItem(KEY, theme);
    apply(theme);
}

function apply(theme: Theme) {
    // CSS 里 :root[data-theme="dark"] 会盖掉 prefers-color-scheme 的判断，
    // 所以手动选过之后就不再跟随系统。
    document.documentElement.dataset.theme = theme;
}

// 模块在渲染前 import，所以这一行就是首帧的主题，不会闪。
apply(getTheme());
