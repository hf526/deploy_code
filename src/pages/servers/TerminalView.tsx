import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { ClipboardPaste, Copy, Eraser, SquareDashedMousePointer } from "lucide-react";

import { ContextMenu, type ContextMenuRow } from "../../components/ui";
import { api } from "../../lib/api";
import i18n from "../../lib/i18n";
import { useApp } from "../../lib/store";
import {
  FONT_SIZE_STEP,
  clampFontSize,
  copyToClipboard,
  nextFontSize,
  readFromClipboard,
  resolveShortcut,
  searchDecorations,
  terminalTheme,
  type TerminalSink,
  type TerminalTab,
} from "../../lib/terminal";

/**
 * 起 PTY 时先用的尺寸：真正的行列要等组件挂载量过才知道，所以先按这个开，
 * 挂载后 fit 一次并立刻把 `window-change` 报上去。远端 shell 只是折行宽度会变，
 * 不会因此丢东西。
 */
export const INITIAL_DIMS = { cols: 80, rows: 24 };

/** 工作区搜索条拿这个操作焦点 pane 的 xterm；不直接暴露 addon 实例。 */
export interface TerminalSearchHandle {
  findNext(query: string, caseSensitive: boolean, incremental?: boolean): void;
  findPrevious(query: string, caseSensitive: boolean): void;
  clear(): void;
  /** 搜索条关闭后把焦点还给终端：输入框一卸载焦点就落 body，不还的话敲键全丢。 */
  focus(): void;
}

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
  fontSize,
  onFontSizeChange,
  attach,
  detach,
  onFind,
  onFocus,
  onDims,
  registerSearch,
  onSearchResults,
}: {
  tab: TerminalTab;
  active: boolean;
  fontSize: number;
  onFontSizeChange: (value: number) => void;
  attach: (id: string, sink: TerminalSink) => void;
  detach: (id: string) => void;
  onFind?: () => void;
  onFocus?: (id: string) => void;
  onDims?: (cols: number, rows: number) => void;
  registerSearch?: (id: string, handle: TerminalSearchHandle | null) => void;
  onSearchResults?: (id: string, result: { index: number; count: number } | null) => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // 输入回调里读到的是最新状态：标签已经结束时不再往远端发按键，否则那份输入只能由后端报错。
  const closedRef = useRef(tab.closed);
  closedRef.current = tab.closed;
  // 滚轮缩放要按「当前」字号算下一档，而 prop 那一份在连着滚两格时还是旧的。
  // 只在值真落地那一次同步，否则 2 秒一次的轮询会用旧读数把它盖回去。
  const fontSizeRef = useRef(clampFontSize(fontSize));
  useEffect(() => {
    fontSizeRef.current = clampFontSize(fontSize);
  }, [fontSize]);
  // 状态与写盘都在上层，这里只报「要变成多少」；effect 里读到的是最新那一份。
  const reportRef = useRef(onFontSizeChange);
  useEffect(() => {
    reportRef.current = onFontSizeChange;
  });
  // 工作区回调走 ref：挂载 effect 只按 tab.id 重建，回调换引用不该重建终端。
  const cbRef = useRef({ onFind, onFocus, onDims, registerSearch, onSearchResults });
  useEffect(() => {
    cbRef.current = { onFind, onFocus, onDims, registerSearch, onSearchResults };
  });
  const activeRef = useRef(active);
  useEffect(() => {
    activeRef.current = active;
  }, [active]);

  async function paste() {
    const term = termRef.current;
    if (!term) return;
    const text = await readFromClipboard();
    if (text === null) {
      // 读不到剪贴板是环境不让读，不是「剪贴板是空的」：说清楚，让用户走 Ctrl+V（那条由浏览器自己递）。
      // 这句走 i18n.t 而不是渲染作用域里的 t：这个回调被 effect 捕获，换语言后老引用还会说旧文案。
      toast("error", i18n.t("terminal.pasteBlocked"));
      return;
    }
    if (text) term.paste(text);
  }

  async function copySelection() {
    const term = termRef.current;
    if (!term || !term.hasSelection()) return;
    await copyToClipboard(term.getSelection());
  }

  function menuRows(): ContextMenuRow[] {
    return [
      {
        kind: "item",
        key: "copy",
        label: t("terminal.menuCopy"),
        icon: <Copy className="size-3.5" />,
        onClick: () => void copySelection(),
      },
      {
        kind: "item",
        key: "paste",
        label: t("terminal.menuPaste"),
        icon: <ClipboardPaste className="size-3.5" />,
        disabled: tab.closed,
        onClick: () => void paste(),
      },
      {
        kind: "item",
        key: "selectAll",
        label: t("terminal.menuSelectAll"),
        icon: <SquareDashedMousePointer className="size-3.5" />,
        onClick: () => termRef.current?.selectAll(),
      },
      { kind: "divider", key: "d" },
      {
        kind: "item",
        key: "clear",
        label: t("terminal.menuClear"),
        icon: <Eraser className="size-3.5" />,
        onClick: () => termRef.current?.clear(),
      },
    ];
  }

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      cursorBlink: true,
      // 搜索高亮走 registerDecoration，xterm 6 仍把它归在 proposed API 后面，不开就抛异常。
      allowProposedApi: true,
      // 一次构建日志动辄上万行，3000 行滚回去半截就没了；20000 行是主流客户端的量级，
      // 内存换的是「出错时能翻回第一个报错」。
      scrollback: 20000,
      // 等宽字体直接取这个元素算出来的 font-family：容器上挂着 .font-mono，取到的就是界面
      // 现在真正在用的那一套，不必再抄一份字体栈进来。
      fontFamily: getComputedStyle(host).fontFamily,
      fontSize: fontSizeRef.current,
      theme: terminalTheme(readToken),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const search = new SearchAddon();
    term.loadAddon(search);
    // 装上还不够：默认宽度表仍是 Unicode 6，中文与 emoji 按旧表算成半角，vim / tmux 里
    // 光标与文本会错列。
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    // 链接交给系统浏览器，不用 addon 默认的 window.open：本应用没设 on_new_window，
    // wry 对没有处理器的新窗口请求直接吞掉，点了毫无反应。
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        void api.openExternalUrl(uri).catch((err) => toast("error", String(err)));
      }),
    );
    term.open(host);
    termRef.current = term;
    fitRef.current = fit;

    search.onDidChangeResults((event) => {
      cbRef.current.onSearchResults?.(
        tab.id,
        event ? { index: event.resultIndex, count: event.resultCount } : null,
      );
    });
    cbRef.current.registerSearch?.(tab.id, {
      // decorations 每次现取：切主题后下一次搜索就用新色，且它必须存在——不传 addon 不画
      // 高亮也不报结果数（见 lib/terminal.ts 的 searchDecorations）。
      findNext: (query, caseSensitive, incremental) => {
        search.findNext(query, {
          caseSensitive,
          incremental: !!incremental,
          decorations: searchDecorations(readToken),
        });
      },
      findPrevious: (query, caseSensitive) => {
        search.findPrevious(query, { caseSensitive, decorations: searchDecorations(readToken) });
      },
      clear: () => search.clearDecorations(),
      focus: () => term.focus(),
    });

    term.onData((data) => {
      if (closedRef.current) return;
      // 发送失败要弹出来：粘贴太大被拒时，静默吞掉等于用户看到「粘了半天什么都没进去」。
      void api.sendTerminalInput(tab.id, data).catch((err) => toast("error", String(err)));
    });
    // 前端量到的行列变化是唯一来源：fit 之后由这里把 window-change 报给远端。
    term.onResize(({ cols, rows }) => {
      void api.resizeTerminal(tab.id, cols, rows).catch(() => undefined);
      // 状态栏的行列数只报可见 pane 的：隐藏标签量出来是 0，报上去就骗人。
      if (activeRef.current) cbRef.current.onDims?.(cols, rows);
    });
    // 选中即复制：拖完就进剪贴板，不用再找一次「到底复制了没有」。
    term.onSelectionChange(() => {
      if (!term.hasSelection()) return;
      void copyToClipboard(term.getSelection());
    });

    // 命中的快捷键一律不让 xterm 处理，否则切一次标签就往远端 shell 里塞一串转义序列。
    // 复制/粘贴/全选在这里做（要拿 term 实例），切标签与关标签交给工作区的窗口监听。
    term.attachCustomKeyEventHandler((event) => {
      const action = resolveShortcut(event);
      if (event.type !== "keydown") return action === null;
      if (!action) return true;
      event.preventDefault();
      if (action === "copy") void copySelection();
      if (action === "paste") void paste();
      if (action === "selectAll") term.selectAll();
      if (action === "find") cbRef.current.onFind?.();
      return false;
    });

    // Ctrl+滚轮调字号：xterm 自己会把滚轮当滚动处理，这里返回 false 把它拦下来。
    // 值只报给上层，落盘与生效都在那一条路上（两处各写一份就会有两个真相）。
    term.attachCustomWheelEventHandler((event) => {
      if (!event.ctrlKey) return true;
      event.preventDefault();
      const next = nextFontSize(fontSizeRef.current, event.deltaY > 0 ? -FONT_SIZE_STEP : FONT_SIZE_STEP);
      if (next !== fontSizeRef.current) {
        fontSizeRef.current = next;
        reportRef.current(next);
      }
      return false;
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

    const onContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      setMenu({ x: event.clientX, y: event.clientY });
    };
    host.addEventListener("contextmenu", onContextMenu);

    // xterm 6 的 Terminal 没有 onFocus 事件；内部 textarea 的 focus 不冒泡，用 focusin 接。
    const onFocusIn = () => cbRef.current.onFocus?.(tab.id);
    host.addEventListener("focusin", onFocusIn);

    // 中键粘贴是终端用户的肌肉记忆；不拦的话浏览器会把它当成「打开新标签」的前奏。
    const onAuxClick = (event: MouseEvent) => {
      if (event.button !== 1) return;
      event.preventDefault();
      void paste();
    };
    host.addEventListener("auxclick", onAuxClick);

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
      host.removeEventListener("contextmenu", onContextMenu);
      host.removeEventListener("auxclick", onAuxClick);
      host.removeEventListener("focusin", onFocusIn);
      cbRef.current.registerSearch?.(tab.id, null);
      detach(tab.id);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // 每个标签一个终端实例：只有 id 变了才重建，切标签不销毁（否则输出缓冲跟着没）。
  }, [tab.id, attach, detach, toast]);

  // 字号是唯一的视图级偏好：改的是实例上的 option，不重建终端（重建会把已经打出来的输出丢掉）。
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    const next = clampFontSize(fontSize);
    if (term.options.fontSize === next) return;
    term.options.fontSize = next;
    const host = hostRef.current;
    if (active && host && host.clientWidth > 0 && host.clientHeight > 0) fitRef.current?.fit();
  }, [fontSize, active]);

  useEffect(() => {
    if (!active) return;
    // 切回来时容器尺寸可能已经变过（隐藏期间 ResizeObserver 被 0 尺寸挡掉了），补一次。
    fitRef.current?.fit();
    termRef.current?.focus();
  }, [active]);

  return (
    <>
      {/* 内边距挂在外层：fit 量的是 host 的内容盒，padding 放 host 上会被算进行列里。 */}
      <div className="h-full w-full p-2">
        <div ref={hostRef} className="font-mono h-full w-full overflow-hidden" />
      </div>
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} rows={menuRows()} onClose={() => setMenu(null)} />
      )}
    </>
  );
}
