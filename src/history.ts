/**
 * 搜索历史。存 localStorage，和主题（theme.ts）一个理由：
 *
 * 这是纯粹的界面便利，按机器算，不该混进 settings.json —— 那个文件是用来
 * 描述「下载行为」的，而且 README 的「分发」一节还在教人把它发出来排查问题，
 * 搜索记录跟着一起发出去不合适。
 */

const KEY = "mydl.searchHistory";

/** 最多留几条。再多就得做搜索框自动补全了，那是另一个功能。 */
const MAX = 12;

export function history(): string[] {
    try {
        const raw = JSON.parse(localStorage.getItem(KEY) ?? "[]");
        return Array.isArray(raw) ? raw.filter((x) => typeof x === "string") : [];
    } catch {
        // 存坏了当没有，不值得为此弹错误。
        return [];
    }
}

function save(list: string[]) {
    localStorage.setItem(KEY, JSON.stringify(list.slice(0, MAX)));
}

/** 记一条。已经存在的挪到最前面而不是重复添加。 */
export function remember(query: string): string[] {
    const q = query.trim();
    if (!q) return history();

    const next = [q, ...history().filter((x) => x !== q)];
    save(next);
    return next.slice(0, MAX);
}

export function forget(query: string): string[] {
    const next = history().filter((x) => x !== query);
    save(next);
    return next;
}

export function clearHistory(): string[] {
    localStorage.removeItem(KEY);
    return [];
}
