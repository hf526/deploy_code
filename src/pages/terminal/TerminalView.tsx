import { useEffect, useRef } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import { terminalTheme, type TerminalSink, type TerminalTab } from "../../lib/terminal";

/**
 * 起 PTY 时先用的尺寸：真正的行列要等组件挂载量过才知道，所以先按这个开，
 * 挂载后 fit 一次并立刻把 `window-change` 报上去。远端 shell 只是折行宽度会变，
 * 不会因此丢东西。
 */
export const INITIAL_DIMS = { cols: 80, rows: 24 };

/**
 * 取一个 CSS 变量的当前值。
 *
 * 只用于 `--sunken` 那一层原始变量：`--color-*` 是 `@theme` 生成的别名，本次构建里没被工具类
 * 用到的那几个根本不会落到 CSS 上，取到空串就等于让 xterm 退回它自带的默认色。
 */
function readToken(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export function TerminalView({
  tab,
  active,
  attach,
  detach,
}: {
  tab: TerminalTab;
  active: boolean;
  attach: (id: string, sink: TerminalSink) => void;
  detach: (id: string) => void;
}) {
  const toast = useApp((state) => state.toast);
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // 输入回调里读到的是最新状态：标签已经结束时不再往远端发按键，否则那份输入只能由后端报错。
  const closedRef = useRef(tab.closed);
  closedRef.current = tab.closed;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      cursorBlink: true,
      // 3000 行够回滚一次构建日志；再多就是拿内存换一个看不到的地方。
      scrollback: 3000,
      // 等宽字体直接取这个元素算出来的 font-family：容器上挂着 .font-mono，取到的就是界面
      // 现在真正在用的那一套，不必再抄一份字体栈进来。
      fontFamily: getComputedStyle(host).fontFamily,
      fontSize: 13,
      theme: terminalTheme(readToken),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;
    fitRef.current = fit;

    term.onData((data) => {
      if (closedRef.current) return;
      // 发送失败要弹出来：粘贴太大被拒时，静默吞掉等于用户看到「粘了半天什么都没进去」。
      void api.sendTerminalInput(tab.id, data).catch((err) => toast("error", String(err)));
    });
    // 前端量到的行列变化是唯一来源：fit 之后由这里把 window-change 报给远端。
    term.onResize(({ cols, rows }) => {
      void api.resizeTerminal(tab.id, cols, rows).catch(() => undefined);
    });

    // 挂载时也要先问过尺寸：对账回来的多个标签里，不是当前那一个挂在 hidden 的容器里，量出来
    // 是 0，此时 fit 会把本地缓冲区压成小窗（实测 6 行），已经打出来的输出被重排一遍，而那一次
    // resize 到后端必然被 check_dims 拒掉（远端没被改坏，但这一趟白跑）。
    // 切到跟前时下面那个 effect 会补上这一次 fit。
    if (host.clientWidth > 0 && host.clientHeight > 0) fit.fit();
    attach(tab.id, { write: (data) => term.write(data) });

    const observer = new ResizeObserver(() => {
      // 隐藏的那一侧量出来是 0，报上去会把远端窗口改成 0 列。
      if (host.clientWidth === 0 || host.clientHeight === 0) return;
      fit.fit();
    });
    observer.observe(host);

    // 主题切换只改 document 上的 data-theme，xterm 不会跟着重算：自己接上。
    const themeWatcher = new MutationObserver(() => {
      term.options.theme = terminalTheme(readToken);
    });
    themeWatcher.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });

    return () => {
      observer.disconnect();
      themeWatcher.disconnect();
      detach(tab.id);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // 每个标签一个终端实例：只有 id 变了才重建，切标签不销毁（否则输出缓冲跟着没）。
  }, [tab.id, attach, detach, toast]);

  useEffect(() => {
    if (!active) return;
    // 切回来时容器尺寸可能已经变过（隐藏期间 ResizeObserver 被 0 尺寸挡掉了），补一次。
    fitRef.current?.fit();
    termRef.current?.focus();
  }, [active]);

  return <div ref={hostRef} className="font-mono h-full w-full overflow-hidden" />;
}
