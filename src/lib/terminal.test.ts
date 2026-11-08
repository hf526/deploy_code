import { describe, expect, it } from "vitest";

import {
  FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  FONT_SIZE_STEP,
  clampFontSize,
  createDispatcher,
  decodeTerminalBytes,
  dropTab,
  markTabClosed,
  nextFontSize,
  normalizeSplit,
  pickSplitTarget,
  readStoredFontSize,
  reconcileTabs,
  resolveShortcut,
  searchDecorations,
  tabCountByServer,
  tabDisplayName,
  tabFromInfo,
  terminalTheme,
  type ShortcutKey,
  type TerminalSink,
  type TerminalTab,
} from "./terminal";
import type { TerminalInfo } from "./types";

function info(id: string, serverName = "深圳"): TerminalInfo {
  return { id, serverId: `s-${id}`, serverName, cols: 80, rows: 24 };
}

function tab(id: string, overrides: Partial<TerminalTab> = {}): TerminalTab {
  return {
    id,
    serverId: `s-${id}`,
    serverName: "深圳",
    closed: false,
    reason: null,
    ...overrides,
  };
}

describe("decodeTerminalBytes", () => {
  it("解回原始字节，含控制序列与非法 UTF-8", () => {
    // ESC [ 31 m + 「中文」 + 换行 + 两个不成对的字节
    const raw = [0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87, 0x0a, 0xff, 0xfe];
    const base64 = Buffer.from(raw).toString("base64");
    expect(Array.from(decodeTerminalBytes(base64))).toEqual(raw);
  });

  it("空串解成空数组而不是 undefined", () => {
    expect(decodeTerminalBytes("")).toEqual(new Uint8Array(0));
  });
});

describe("标签状态", () => {
  it("通道结束只改状态，标签留在列表里", () => {
    const next = markTabClosed([tab("a"), tab("b")], "a", "连接已断开");
    expect(next).toHaveLength(2);
    expect(next[0]).toEqual(tab("a", { closed: true, reason: "连接已断开" }));
    expect(next[1].closed).toBe(false);
  });

  it("正常退出把原因写成 null，而不是留着上一个原因", () => {
    const closed = markTabClosed([tab("a", { closed: true, reason: "旧原因" })], "a", null);
    expect(closed[0].reason).toBeNull();
    expect(closed[0].closed).toBe(true);
  });

  it("点 × 才真的摘掉标签", () => {
    expect(dropTab([tab("a"), tab("b")], "a").map((item) => item.id)).toEqual(["b"]);
  });
});

describe("reconcileTabs", () => {
  it("快照拍下去之后才开出来的标签不许被对账抹掉", () => {
    // 后端这份 list 里没有 "a"，只说明拍快照那一刻它还不存在，不说明它已经死了：
    // 深链自动开的那个标签正是这样被旧快照删掉的，而它在后端还占着一条通道。
    const next = reconcileTabs([tab("a"), tab("b")], [info("b")]);
    expect(next.map((item) => item.id)).toEqual(["a", "b"]);
  });

  it("后端有、本地没有的补进来，排在已有标签后面", () => {
    const local = [tab("a")];
    const next = reconcileTabs(local, [info("z"), info("a")]);
    expect(next.map((item) => item.id)).toEqual(["a", "z"]);
    expect(next[1].serverName).toBe("深圳");
  });

  it("本地已结束的标签对账时保留它的 closed 状态，不因为还活着就被重置", () => {
    const local = [tab("a", { closed: true, reason: "远端 shell 已退出（退出码 0）" })];
    const next = reconcileTabs(local, [info("a")]);
    expect(next[0].closed).toBe(true);
    expect(next[0].reason).toContain("退出码");
  });

  it("两边都空就是空列表", () => {
    expect(reconcileTabs([], [])).toEqual([]);
  });
});

describe("terminalTheme", () => {
  const read = (name: string) => `var(${name})`;

  it("每个颜色都来自语义 token，且读的是两种主题各自覆盖的那一层", () => {
    const theme = terminalTheme(read);
    for (const [key, value] of Object.entries(theme)) {
      expect(value, `主题色 ${key} 必须是 token 派生的`).toMatch(/^var\(--[a-z-]+\)$/);
      // --color-* 是 @theme 生成的别名，只有本次构建真被某个工具类用到才会落到 CSS 上
      // （实测 --color-sunken 在、--color-chip 不在）；读它等于把终端配色押在构建产物上。
      expect(value.includes("--color-"), `不该读别名层：${key}=${value}`).toBe(false);
    }
  });

  it("底色用日志区那个沉底色，正文用主文字色", () => {
    const theme = terminalTheme(read);
    expect(theme.background).toBe("var(--sunken)");
    expect(theme.foreground).toBe("var(--ink)");
    expect(theme.cursor).toBe("var(--brand)");
    expect(theme.selectionBackground).toBe("var(--chip)");
  });

  it("ANSI 的 16 色补齐，缺一个 xterm 就退回自己的默认调色板（浅色主题下会出现黄字白底）", () => {
    const theme = terminalTheme(read);
    const ansi = [
      "black",
      "red",
      "green",
      "yellow",
      "blue",
      "magenta",
      "cyan",
      "white",
      "brightBlack",
      "brightRed",
      "brightGreen",
      "brightYellow",
      "brightBlue",
      "brightMagenta",
      "brightCyan",
      "brightWhite",
    ];
    for (const key of ansi) {
      expect(theme[key], `缺 ANSI 色 ${key}`).toBeDefined();
    }
    expect(theme.red).toBe("var(--neg)");
    expect(theme.green).toBe("var(--pos)");
    expect(theme.yellow).toBe("var(--warn)");
  });

  // 蓝/品红/青同色时 ls --color 与提示符分不出谁是谁，这三色必须各自独立。
  it("蓝、品红、青互不相同", () => {
    const theme = terminalTheme(read);
    expect(theme.magenta).toBe("var(--term-magenta)");
    expect(theme.cyan).toBe("var(--term-cyan)");
    expect(new Set([theme.blue, theme.magenta, theme.cyan]).size).toBe(3);
  });
});

describe("searchDecorations", () => {
  it("四个色都从 token 取，普通命中与当前命中分得开", () => {
    const read = (name: string) => `var(${name})`;
    const deco = searchDecorations(read);
    expect(deco.matchBackground).toBe("var(--term-search-match)");
    expect(deco.activeMatchBackground).toBe("var(--term-search-active)");
    expect(deco.matchOverviewRuler).toBe("var(--term-search-match)");
    expect(deco.activeMatchColorOverviewRuler).toBe("var(--term-search-active)");
    expect(deco.matchBackground).not.toBe(deco.activeMatchBackground);
  });
});

describe("tabFromInfo", () => {
  it("后端回来的标签默认是活的、没有结束原因", () => {
    expect(tabFromInfo(info("a"))).toEqual(tab("a"));
  });
});

describe("createDispatcher", () => {
  function recorder() {
    const written: number[][] = [];
    const sink: TerminalSink = {
      write: (data) => written.push(Array.from(data)),
    };
    return { sink, written };
  }

  it("组件还没挂载时先攒着，挂上按原顺序补发", () => {
    const dispatcher = createDispatcher();
    dispatcher.push("a", new Uint8Array([1, 2]));
    dispatcher.push("a", new Uint8Array([3]));
    expect(dispatcher.queuedCount("a")).toBe(2);
    const { sink, written } = recorder();
    dispatcher.attach("a", sink);
    expect(written).toEqual([[1, 2], [3]]);
    expect(dispatcher.queuedCount("a")).toBe(0);
  });

  it("挂上之后直写，不再绕队列", () => {
    const dispatcher = createDispatcher();
    const { sink, written } = recorder();
    dispatcher.attach("a", sink);
    dispatcher.push("a", new Uint8Array([9]));
    expect(written).toEqual([[9]]);
    expect(dispatcher.queuedCount("a")).toBe(0);
  });

  it("切走的标签继续攒，切回来不丢中间的输出", () => {
    const dispatcher = createDispatcher();
    const first = recorder();
    dispatcher.attach("a", first.sink);
    dispatcher.detach("a");
    dispatcher.push("a", new Uint8Array([7, 7]));
    const second = recorder();
    dispatcher.attach("a", second.sink);
    expect(second.written).toEqual([[7, 7]]);
    // 旧实例已经接不走，不该再多写一份。
    expect(first.written).toEqual([]);
  });

  it("标签关掉时连同攒着的输出一起丢", () => {
    const dispatcher = createDispatcher();
    dispatcher.push("a", new Uint8Array([1]));
    dispatcher.forget("a");
    expect(dispatcher.queuedCount("a")).toBe(0);
    const { sink, written } = recorder();
    dispatcher.attach("a", sink);
    expect(written).toEqual([]);
  });

  it("两个标签的输出不串台", () => {
    const dispatcher = createDispatcher();
    const a = recorder();
    const b = recorder();
    dispatcher.attach("a", a.sink);
    dispatcher.attach("b", b.sink);
    dispatcher.push("b", new Uint8Array([2]));
    expect(a.written).toEqual([]);
    expect(b.written).toEqual([[2]]);
  });
});

describe("tabDisplayName", () => {
  it("一台一个标签时不挂序号", () => {
    const tabs = [tab("a"), tab("b", { serverId: "s-other", serverName: "港机" })];
    expect(tabDisplayName(tabs, tabs[0])).toBe("深圳");
    expect(tabDisplayName(tabs, tabs[1])).toBe("港机");
  });

  // 同一台开第二个标签时，两个一模一样的名字没法分——这是要修的那件事本身。
  it("同一台的第二个标签起带位次，第一个也一起带上", () => {
    const tabs = [
      tab("a", { serverId: "s1" }),
      tab("b", { serverId: "s1" }),
      tab("c", { serverId: "s1" }),
    ];
    expect(tabs.map((item) => tabDisplayName(tabs, item))).toEqual([
      "深圳 · 1",
      "深圳 · 2",
      "深圳 · 3",
    ]);
  });

  it("关掉中间那个之后序号重排，不会留下没有对应标签的位次", () => {
    const tabs = [tab("a", { serverId: "s1" }), tab("b", { serverId: "s1" })];
    const remaining = dropTab(tabs, "a");
    expect(tabDisplayName(remaining, remaining[0])).toBe("深圳");
  });

  it("查不到自己时退回服务器名而不是「· 0」", () => {
    const tabs = [tab("a", { serverId: "s1" }), tab("b", { serverId: "s1" })];
    expect(tabDisplayName(tabs, tab("zzz", { serverId: "s1" }))).toBe("深圳");
  });
});

describe("分屏", () => {
  it("右侧标签被关掉或与左侧重合时归一成 null", () => {
    const tabs = [tab("a"), tab("b")];
    expect(normalizeSplit(tabs, "a", "b")).toBe("b");
    expect(normalizeSplit(tabs, "a", "a")).toBeNull();
    expect(normalizeSplit(tabs, "a", "zzz")).toBeNull();
    expect(normalizeSplit(tabs, "a", null)).toBeNull();
    expect(normalizeSplit([tab("a")], "a", "b")).toBeNull();
  });

  it("打开分屏时右侧给除当前外的第一个，只有一个标签时没得分", () => {
    expect(pickSplitTarget([tab("a"), tab("b"), tab("c")], "a")).toBe("b");
    expect(pickSplitTarget([tab("a"), tab("b")], "b")).toBe("a");
    expect(pickSplitTarget([tab("a")], "a")).toBeNull();
    expect(pickSplitTarget([], "")).toBeNull();
  });
});

describe("tabCountByServer", () => {
  it("按服务器分组数标签，没开的分组干脆不出现", () => {
    const tabs = [
      tab("a", { serverId: "s1" }),
      tab("b", { serverId: "s1" }),
      tab("c", { serverId: "s2" }),
    ];
    expect(tabCountByServer(tabs)).toEqual({ s1: 2, s2: 1 });
    expect(tabCountByServer([])).toEqual({});
  });
});

describe("字号", () => {
  it("限幅在两端夹住，不做环绕", () => {
    expect(nextFontSize(FONT_SIZE_MIN, -FONT_SIZE_STEP)).toBe(FONT_SIZE_MIN);
    expect(nextFontSize(FONT_SIZE_MAX, FONT_SIZE_STEP)).toBe(FONT_SIZE_MAX);
    expect(nextFontSize(14, FONT_SIZE_STEP)).toBe(15);
    expect(clampFontSize(999)).toBe(FONT_SIZE_MAX);
    expect(clampFontSize(4)).toBe(FONT_SIZE_MIN);
  });

  it("非数字、0、负数都回落到默认而不是把 NaN 喂给 xterm", () => {
    expect(readStoredFontSize(null)).toBe(FONT_SIZE_DEFAULT);
    expect(readStoredFontSize("")).toBe(FONT_SIZE_DEFAULT);
    expect(readStoredFontSize("abc")).toBe(FONT_SIZE_DEFAULT);
    expect(readStoredFontSize("0")).toBe(FONT_SIZE_DEFAULT);
    expect(readStoredFontSize("-13")).toBe(FONT_SIZE_DEFAULT);
    expect(clampFontSize(Number.NaN)).toBe(FONT_SIZE_DEFAULT);
  });

  it("盘上越界的值夹回端点，认得出的数字不会被当成没读到", () => {
    expect(readStoredFontSize("18")).toBe(18);
    expect(readStoredFontSize("40")).toBe(FONT_SIZE_MAX);
  });
});

describe("resolveShortcut", () => {
  function key(overrides: Partial<ShortcutKey> = {}): ShortcutKey {
    return { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, code: "", key: "", ...overrides };
  }

  // Ctrl+C 是远端的 SIGINT，抢不得；复制只能走 Ctrl+Shift+C。
  it("不带 Shift 的 Ctrl+C 与 Ctrl+V 不是快捷键", () => {
    expect(resolveShortcut(key({ ctrlKey: true, key: "c", code: "KeyC" }))).toBeNull();
    expect(resolveShortcut(key({ ctrlKey: true, key: "v", code: "KeyV" }))).toBeNull();
  });

  it("Ctrl+Shift 的复制/粘贴/全选/关标签/搜索", () => {
    expect(resolveShortcut(key({ ctrlKey: true, shiftKey: true, key: "C", code: "KeyC" }))).toBe("copy");
    expect(resolveShortcut(key({ ctrlKey: true, shiftKey: true, key: "V", code: "KeyV" }))).toBe("paste");
    expect(resolveShortcut(key({ ctrlKey: true, shiftKey: true, key: "A", code: "KeyA" }))).toBe("selectAll");
    expect(resolveShortcut(key({ ctrlKey: true, shiftKey: true, key: "W", code: "KeyW" }))).toBe("closeTab");
    expect(resolveShortcut(key({ ctrlKey: true, shiftKey: true, key: "F", code: "KeyF" }))).toBe("find");
  });

  it("Shift+Insert 粘贴、Ctrl+Insert 复制", () => {
    expect(resolveShortcut(key({ shiftKey: true, key: "Insert", code: "Insert" }))).toBe("paste");
    expect(resolveShortcut(key({ ctrlKey: true, key: "Insert", code: "Insert" }))).toBe("copy");
    expect(resolveShortcut(key({ key: "Insert", code: "Insert" }))).toBeNull();
  });

  it("切标签用 Ctrl+PageUp/Down，带 Alt 的一律放行给远端", () => {
    expect(resolveShortcut(key({ ctrlKey: true, code: "PageUp", key: "PageUp" }))).toBe("prevTab");
    expect(resolveShortcut(key({ ctrlKey: true, code: "PageDown", key: "PageDown" }))).toBe("nextTab");
    expect(resolveShortcut(key({ altKey: true, ctrlKey: true, shiftKey: true, key: "C" }))).toBeNull();
  });

  // readline 的删词与转置：这两条被抢掉的话，终端里打字会变得更难用，不是更好用。
  it("不劫持 Ctrl+W 与 Ctrl+T", () => {
    expect(resolveShortcut(key({ ctrlKey: true, key: "w", code: "KeyW" }))).toBeNull();
    expect(resolveShortcut(key({ ctrlKey: true, key: "t", code: "KeyT" }))).toBeNull();
  });

  it("⌘ 组合与 Ctrl 同等对待，纯字母键不命中", () => {
    expect(resolveShortcut(key({ metaKey: true, shiftKey: true, key: "c", code: "KeyC" }))).toBe("copy");
    expect(resolveShortcut(key({ key: "a", code: "KeyA" }))).toBeNull();
  });
});
