import { describe, expect, it } from "vitest";

import {
  createDispatcher,
  decodeTerminalBytes,
  dropTab,
  markTabClosed,
  reconcileTabs,
  tabFromInfo,
  terminalTheme,
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
