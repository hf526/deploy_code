import type { TerminalInfo } from "./types";

/**
 * 终端标签的前端状态与两处纯逻辑（base64 解码、主题取色）。
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
 * ANSI 的 16 色按语义就近映射（红=负向、绿=正向、黄=警示、蓝/青/品红=强调）：xterm 的默认
 * 调色板是给深底配的，浅色主题下不覆盖就会出现黄字白底。
 */
export function terminalTheme(read: TokenReader): Record<string, string> {
  const ink = read("--ink");
  const dim = read("--ink-dim");
  const faint = read("--ink-faint");
  const brand = read("--brand");
  const pos = read("--pos");
  const neg = read("--neg");
  const warn = read("--warn");
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
    magenta: brand,
    cyan: brand,
    white: ink,
    brightBlack: dim,
    brightRed: neg,
    brightGreen: pos,
    brightYellow: warn,
    brightBlue: brand,
    brightMagenta: brand,
    brightCyan: brand,
    brightWhite: ink,
  };
}
