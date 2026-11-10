import type { TerminalInfo } from "./types";

/**
 * 终端标签的前端状态与展示用纯逻辑：base64 解码、主题取色、标签显示名、字号、快捷键、剪贴板。
 *
 * 上限判定不在这里：名额是后端按「同一条常驻会话的通道数」算的，前端再算一份就是两个真相，
 * 只会各说各话。这里只负责把后端已经给出的结论显示出来。
 */

export interface TerminalTab {
  id: string;
  serverId: string;
  serverName: string;
  /** 远端通道已结束：标签留在列表里（输出还能看、能复制），但不再接受输入。 */
  closed: boolean;
  /** 结束原因；`null` 是远端 shell 正常退出。 */
  reason: string | null;
}

export function tabFromInfo(info: TerminalInfo): TerminalTab {
  return {
    id: info.id,
    serverId: info.serverId,
    serverName: info.serverName,
    closed: false,
    reason: null,
  };
}

/** 通道结束：只改状态，不摘标签——把已经打出来的输出连带标签一起消失，等于把用户刚看到的东西弄丢了。 */
export function markTabClosed(tabs: TerminalTab[], id: string, reason: string | null): TerminalTab[] {
  return tabs.map((tab) => (tab.id === id ? { ...tab, closed: true, reason } : tab));
}

/** 用户点 × 关掉标签：这一步才真的从列表里摘掉。 */
export function dropTab(tabs: TerminalTab[], id: string): TerminalTab[] {
  return tabs.filter((tab) => tab.id !== id);
}

/**
 * 与后端的存活标签对账（进页面那一次），只做加法：把后端有、本地没有的标签补回来。
 *
 * 反过来「本地有、后端没有就摘掉」看着整齐，实际站不住：这条对账只在挂载那一次跑，那时
 * 本地能有的标签只可能是同一次挂载里刚开出来的（深链自动开、或用户抢在对账回来之前点了
 * 「打开」），而后端那份快照拍下时它还不存在。拿旧快照去删新事实，结果是用户刚开的标签
 * 在界面上被抹掉，而那条通道在后端还活着——从此关不掉，白占一个名额。
 * 标签的死活另有 `closed` 事件负责，不需要靠对账兜。
 */
export function reconcileTabs(local: TerminalTab[], remote: TerminalInfo[]): TerminalTab[] {
  const known = new Set(local.map((tab) => tab.id));
  return [...local, ...remote.filter((info) => !known.has(info.id)).map(tabFromInfo)];
}

/**
 * 解 base64 成原始字节。
 *
 * 必须按字节给 xterm：PTY 输出里是控制序列，也可能有不成对的 UTF-8 字节，
 * 当成字符串走一遍就把 vim 的画面画花了。
 */
export function decodeTerminalBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/** 界面侧的接收端：只用到 xterm 的 `write`，这里不直接依赖 xterm。 */
export interface TerminalSink {
  write(data: Uint8Array): void;
}

/**
 * 输出分发：有接收端就直写，没有就先攒着。
 *
 * 为什么要攒：远端 shell 的第一屏提示符常常比组件挂载先到，直写的话那几个字节就没了，
 * 而用户看到的是「终端打开了一片空白，得敲一下回车才醒」。
 */
export function createDispatcher() {
  const sinks = new Map<string, TerminalSink>();
  const pending = new Map<string, Uint8Array[]>();

  const flush = (id: string, sink: TerminalSink) => {
    const queued = pending.get(id);
    if (!queued || queued.length === 0) return;
    pending.delete(id);
    for (const chunk of queued) sink.write(chunk);
  };

  return {
    /** 组件挂载：接上并补发攒下的输出。 */
    attach(id: string, sink: TerminalSink) {
      sinks.set(id, sink);
      flush(id, sink);
    },
    /** 组件卸载（切走的标签会被销毁、或整页关掉）：之后的输出回到队列里。 */
    detach(id: string) {
      sinks.delete(id);
    },
    /** 事件回来的一截输出。 */
    push(id: string, chunk: Uint8Array) {
      const sink = sinks.get(id);
      if (sink) {
        sink.write(chunk);
        return;
      }
      const queued = pending.get(id);
      if (queued) queued.push(chunk);
      else pending.set(id, [chunk]);
    },
    /** 标签真的没了：连同它攒下的输出一起丢掉，不留着占内存。 */
    forget(id: string) {
      sinks.delete(id);
      pending.delete(id);
    },
    /** 只读，给单测看队列状态用。 */
    queuedCount(id: string): number {
      return pending.get(id)?.length ?? 0;
    },
  };
}

/** 取一个 CSS 变量的当前值（主题切换后重新取，不缓存）。 */
export type TokenReader = (name: string) => string;
/**
 * xterm 的主题色，全部从既有的语义 token 取，不新造一份调色板。
 *
 * 读 `--sunken` / `--chip` 这一层，不读 `--color-sunken` / `--color-chip`：后者是 `@theme`
 * 生成的别名，只有这次构建里真被某个工具类用到才会落到 CSS 上（实测 `--color-sunken` 在、
 * `--color-chip` 不在）。跟着「某个界面恰好用到过」这种条件走，哪天那处样式一改，终端就取到
 * 空串然后 xterm 悄悄退回默认调色板。两种主题各自覆盖的本来也是前面那一层。
 *
 * ANSI 的 16 色按语义就近映射（红=负向、绿=正向、黄=警示、蓝=强调）；品红与青各有专用
 * token（--term-magenta / --term-cyan）：三者同色时 ls --color、shell 提示符、man 页全是一个
 * 颜色，分不出谁是谁。xterm 的默认调色板是给深底配的，浅色主题下不覆盖就会出现黄字白底。
 */
export function terminalTheme(read: TokenReader): Record<string, string> {
  const ink = read("--ink");
  const dim = read("--ink-dim");
  const faint = read("--ink-faint");
  const brand = read("--brand");
  const pos = read("--pos");
  const neg = read("--neg");
  const warn = read("--warn");
  const magenta = read("--term-magenta");
  const cyan = read("--term-cyan");
  return {
    background: read("--sunken"),
    foreground: ink,
    cursor: brand,
    selectionBackground: read("--chip"),
    // 黑=最弱的一档文字，白=正文；剩下的按语义色走，亮色档只把 dim 提到 ink。
    black: faint,
    red: neg,
    green: pos,
    yellow: warn,
    blue: brand,
    magenta,
    cyan,
    white: ink,
    brightBlack: dim,
    brightRed: neg,
    brightGreen: pos,
    brightYellow: warn,
    brightBlue: brand,
    brightMagenta: magenta,
    brightCyan: cyan,
    brightWhite: ink,
  };
}

/**
 * 搜索命中的装饰色。addon-search 只有传了 decorations 才画高亮、才发 onDidChangeResults
 * （不传时静默什么都不报），所以这份选项是必需品而不是锦上添花。颜色只从 token 取，
 * 且必须是 #RRGGBB——token 层本来就按这个格式写。
 */
export function searchDecorations(read: TokenReader): {
  matchBackground: string;
  activeMatchBackground: string;
  matchOverviewRuler: string;
  activeMatchColorOverviewRuler: string;
} {
  const match = read("--term-search-match");
  const active = read("--term-search-active");
  return {
    matchBackground: match,
    activeMatchBackground: active,
    matchOverviewRuler: match,
    activeMatchColorOverviewRuler: active,
  };
}

/**
 * 标签显示名：同一台服务器开第二个标签时必须分得出谁是谁。
 *
 * 只有一台一个标签时不挂序号（绝大多数时候界面上就是「深圳轻量云」，加个「· 1」是噪音）；
 * 从第二个起带上它在同机标签里的位次。序号按当前列表算，关掉中间那个会重排——标签本来就
 * 只剩那几个，重排比留着「· 3」而总数只有 2 要好读。
 */
export function tabDisplayName(tabs: TerminalTab[], tab: TerminalTab): string {
  const sameServer = tabs.filter((item) => item.serverId === tab.serverId);
  if (sameServer.length <= 1) return tab.serverName;
  const index = sameServer.findIndex((item) => item.id === tab.id);
  return index < 0 ? tab.serverName : `${tab.serverName} · ${index + 1}`;
}

/** 每台服务器开了几个标签：树上的徽章用，0 就不画。 */
export function tabCountByServer(tabs: TerminalTab[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const tab of tabs) counts[tab.serverId] = (counts[tab.serverId] ?? 0) + 1;
  return counts;
}

// ---------------------------------------------------------------------------
// 分屏
// ---------------------------------------------------------------------------

/**
 * 分屏归一：右侧那个标签必须还活着、且不能与左侧重合。
 *
 * 关标签 / 关其他 / 切标签都可能让 splitId 悬空或等于 activeId，界面拿着它画第二 pane
 * 会出现「同一个终端挂两份」或空 pane。每次 tabs/activeId 变化后过一遍这里。
 */
export function normalizeSplit(
  tabs: TerminalTab[],
  activeId: string,
  splitId: string | null,
): string | null {
  if (!splitId || splitId === activeId) return null;
  return tabs.some((tab) => tab.id === splitId) ? splitId : null;
}

/** 打开分屏时右侧给谁：除当前标签外的第一个；只有一个标签时没得分。 */
export function pickSplitTarget(tabs: TerminalTab[], activeId: string): string | null {
  return tabs.find((tab) => tab.id !== activeId)?.id ?? null;
}

// ---------------------------------------------------------------------------
// 字号
// ---------------------------------------------------------------------------

export const FONT_SIZE_DEFAULT = 13;
export const FONT_SIZE_MIN = 11;
export const FONT_SIZE_MAX = 22;
export const FONT_SIZE_STEP = 1;
/** 本机视图偏好，不进 settings：它不参与导出抹凭据那两条不变量，也不该被配置导入盖掉。 */
export const FONT_SIZE_STORAGE_KEY = "deploycode:terminal.fontSize";

export function clampFontSize(value: number): number {
  if (!Number.isFinite(value)) return FONT_SIZE_DEFAULT;
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, Math.round(value)));
}

/** delta 传 ±STEP（滚轮用 ±1）：越界就停在端点，不做环绕。 */
export function nextFontSize(current: number, delta: number): number {
  return clampFontSize(current + delta);
}

/** 盘上的值可能是别的版本写的、也可能根本不是数字：认不出就用默认，不拿 NaN 去喂 xterm。 */
export function readStoredFontSize(raw: string | null): number {
  if (!raw) return FONT_SIZE_DEFAULT;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) return FONT_SIZE_DEFAULT;
  return clampFontSize(parsed);
}

export function writeStoredFontSize(value: number): void {
  try {
    localStorage.setItem(FONT_SIZE_STORAGE_KEY, String(value));
  } catch {
    // 隐私模式 / WebView 禁写：字号照样生效，只是下次进来回到默认，不值得为它弹错。
  }
}

export function readStoredFontSizeSafe(): number {
  try {
    return readStoredFontSize(localStorage.getItem(FONT_SIZE_STORAGE_KEY));
  } catch {
    return FONT_SIZE_DEFAULT;
  }
}

// ---------------------------------------------------------------------------
// tmux 会话
// ---------------------------------------------------------------------------

/**
 * 新标签是否包一层 tmux（远端跑 `tmux new -A -s deploycode-N`）。
 *
 * 与字号同例属于本机使用习惯，不进 settings：它不该被配置导入盖掉，也不参与导出抹凭据
 * 那两条不变量。默认关——不是每台机器都装了 tmux，而勾上之后没装的那台会以 127 退出、
 * 标签当场结束，这个决定该由用户按机器情况自己按下去。
 */
export const TMUX_STORAGE_KEY = "deploycode:terminal.tmux";

/** 只认字面量 "true"：盘上可能是别的版本写的东西，认不出就当没勾。 */
export function readStoredTmux(raw: string | null): boolean {
  return raw === "true";
}

export function writeStoredTmux(value: boolean): void {
  try {
    localStorage.setItem(TMUX_STORAGE_KEY, value ? "true" : "false");
  } catch {
    // 隐私模式 / WebView 禁写：这一次照样生效，只是下次进来回到默认。
  }
}

export function readStoredTmuxSafe(): boolean {
  try {
    return readStoredTmux(localStorage.getItem(TMUX_STORAGE_KEY));
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// 快捷键
// ---------------------------------------------------------------------------

export type TerminalShortcut =
  | "copy"
  | "paste"
  | "selectAll"
  | "prevTab"
  | "nextTab"
  | "closeTab"
  | "find";

/** KeyboardEvent 里这一组字段就够判定，测试可以直接传对象字面量。 */
export interface ShortcutKey {
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  code: string;
  key: string;
}

/**
 * 终端区域的快捷键表。
 *
 * 三条口径：① 只认带 Ctrl/⌘ 或 Shift 的组合，`Ctrl+C` 必须是远端的 SIGINT，所以复制要
 * `Ctrl+Shift+C`；② 不劫持 `Ctrl+W`（readline 删词）与 `Ctrl+T`（转置字符）——那是 shell 的
 * 键，标签操作交给 `+`、`×` 与 `Ctrl+PageUp/Down`；③ 命中即返回，调用方据此不让 xterm 把
 * 这几个键发到远端，否则切一次标签就往 shell 里塞一串转义序列。
 */
export function resolveShortcut(event: ShortcutKey): TerminalShortcut | null {
  if (event.altKey) return null;
  const held = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();

  if (key === "insert") {
    if (event.ctrlKey && !event.shiftKey) return "copy";
    if (event.shiftKey && !event.ctrlKey) return "paste";
    return null;
  }
  if (!held) return null;
  if (!event.shiftKey) {
    if (event.code === "PageUp") return "prevTab";
    if (event.code === "PageDown") return "nextTab";
    return null;
  }
  if (key === "c") return "copy";
  if (key === "v") return "paste";
  if (key === "a") return "selectAll";
  if (key === "w") return "closeTab";
  if (key === "f") return "find";
  return null;
}

// ---------------------------------------------------------------------------
// 剪贴板
// ---------------------------------------------------------------------------

/**
 * 写剪贴板：优先异步 Clipboard API，拿不到（非安全上下文 / WebView 没给权限）再退回
 * execCommand。两条都不行就返回 false——调用方不弹错，选中内容还在，用户可以用菜单再复制一次。
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 落到下面的兜底
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
}

/**
 * 读剪贴板。读不到返回 null 而不是空串：空串是「剪贴板里确实没东西」，
 * null 是「这个环境不让我们读」，界面对这两件事的说法不一样。
 */
export async function readFromClipboard(): Promise<string | null> {
  try {
    if (navigator.clipboard?.readText) return await navigator.clipboard.readText();
  } catch {
    return null;
  }
  return null;
}
