import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  CaseSensitive,
  ChevronDown,
  ChevronUp,
  Columns2,
  Download,
  Keyboard,
  ListChecks,
  Minus,
  Plus,
  RotateCw,
  Search,
  TerminalSquare,
  Upload,
  X,
} from "lucide-react";

import {
  Button,
  ContextMenu,
  EmptyState,
  Input,
  Modal,
  type ContextMenuRow,
} from "../../components/ui";
import i18n from "../../lib/i18n";
import {
  CPU_WARN_PERCENT,
  MEMORY_WARN_PERCENT,
  hostLink,
  hostLinkClass,
  memoryPercent,
  metricsView,
  usageTone,
  usageTextColor,
} from "../../lib/metrics";
import {
  FONT_SIZE_STEP,
  nextFontSize,
  normalizeSplit,
  pickSplitTarget,
  resolveShortcut,
  tabDisplayName,
  type TerminalSink,
  type TerminalTab,
} from "../../lib/terminal";
import type { TransferEntry } from "../../lib/transfer";
import type {
  ServerConfig,
  ServerHostStatus,
  TransferDirection,
} from "../../lib/types";
import { cn } from "../../lib/utils";
import { TerminalView, type TerminalSearchHandle } from "./TerminalView";
import { TransferStrip } from "./TransferStrip";

/**
 * 右半边的终端区：标签栏 + 搜索条 + 传输条带 + xterm（可分屏）+ 底部状态栏。
 *
 * 页头去掉了——那行原来白吃约 50px 的终端高度，而终端要的正是纵向空间。服务器信息改挂
 * 底部状态栏（Xshell 同位置）：user@host、连接与占用、行列数，任何窗口宽度都可见。
 */
export function TerminalWorkspace({
  tabs,
  activeId,
  busy,
  activityIds,
  clearActivity,
  servers,
  statuses,
  fontSize,
  selectedServerId,
  onFontSizeChange,
  onSelect,
  onOpen,
  onClose,
  onCloseOthers,
  attach,
  detach,
  transfers,
  zmodemHint,
  onPickTransfer,
  onCancelTransfer,
  onDismissZmodem,
  onReconnect,
  reconnectingId,
}: {
  tabs: TerminalTab[];
  activeId: string;
  busy: boolean;
  activityIds: string[];
  clearActivity: (id: string) => void;
  servers: ServerConfig[];
  statuses: ServerHostStatus[];
  fontSize: number;
  selectedServerId: string;
  onFontSizeChange: (value: number) => void;
  onSelect: (id: string) => void;
  onOpen: (serverId: string) => void;
  onClose: (id: string) => Promise<void>;
  onCloseOthers: (id: string) => void;
  attach: (id: string, sink: TerminalSink) => void;
  detach: (id: string) => void;
  transfers: TransferEntry[];
  /** 当前这个标签里的远端跑过 `rz` / `sz`：那套协议我们不做，条带指路到上传 / 下载。 */
  zmodemHint: boolean;
  onPickTransfer: (serverId: string, mode: TransferDirection) => void;
  onCancelTransfer: (id: string) => void;
  onDismissZmodem: () => void;
  /** 状态栏的重连入口：会话断开时就地重连，不用再开隧道弹窗。 */
  onReconnect: (serverId: string) => void;
  /** 正在重连的那一台（转圈用），与树上的行内按钮共用一个状态。 */
  reconnectingId: string | null;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{ x: number; y: number; tab: TerminalTab } | null>(null);
  // 分屏：左=activeId，右=splitId。归一（右侧被关/与左重合）走 lib 的纯逻辑。
  const [splitId, setSplitId] = useState<string | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [results, setResults] = useState<{ index: number; count: number } | null>(null);
  // 搜索作用于焦点所在的 pane（分屏时两边都可见，只有焦点那边该被搜）。
  const [focusedId, setFocusedId] = useState("");
  const [dims, setDims] = useState<{ cols: number; rows: number } | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchHandles = useRef(new Map<string, TerminalSearchHandle>());
  const focusedIdRef = useRef("");
  focusedIdRef.current = focusedId;
  // 字号的「最新请求值」：只在落地那一次同步。渲染时直接赋值会被 2 秒一次的轮询用旧值盖回去，
  // 那一轮点击就白按了。
  const fontSizeRef = useRef(fontSize);
  useEffect(() => {
    fontSizeRef.current = fontSize;
  }, [fontSize]);

  const active = tabs.find((tab) => tab.id === activeId) ?? null;
  const activeServer = active ? servers.find((item) => item.id === active.serverId) : undefined;
  const activeStatus = active
    ? statuses.find((item) => item.serverId === active.serverId)
    : undefined;
  // 空状态要显示「当前选中了哪台」并把上传 / 下载入口摆出来：以前这两个动作只在有标签时
  // 才出现在工具栏里，想传个文件得先开一个终端。
  const selectedServer = servers.find((item) => item.id === selectedServerId);
  const activeLink = hostLink(activeStatus).link;
  const activeLinkLabel =
    activeLink === "online"
      ? t("servers.linkOnline", { time: activeStatus?.connectedAt ?? "-" })
      : activeLink === "retrying"
        ? t("servers.linkRetrying", { retries: activeStatus?.retries ?? 0 })
        : activeLink === "down"
          ? t("servers.linkDown")
          : t("servers.linkUnknown");

  // 「+」给谁开标签：有当前标签就跟着它，否则跟着树上选中的那台。
  const plusTarget = active?.serverId || selectedServerId;

  useEffect(() => {
    setSplitId((prev) => normalizeSplit(tabs, activeId, prev));
  }, [tabs, activeId]);

  useEffect(() => {
    setFocusedId(activeId);
  }, [activeId]);

  // 焦点不能停在一个已经看不见的 pane 上：关分屏、或关掉正被聚焦的那个标签之后，`focusedId`
  // 还指着那个 `hidden` 的终端——浏览器把焦点丢回 body（字母键静默丢失，用户以为「终端不响应」），
  // 而搜索条仍只对看不见的那一个生效（可见端没有高亮、却报着它的命中数）。回落到当前标签并
  // 把键盘交还给它。
  useEffect(() => {
    if (focusedId === activeId || (!!splitId && focusedId === splitId)) return;
    setFocusedId(activeId);
    if (!activeId) return;
    searchHandles.current.get(activeId)?.focus();
  }, [focusedId, activeId, splitId]);

  // 分屏右侧那个 pane 正被看着，不算「后台来过输出」：显示侧已抑制，这里在收拢分屏时
  // 把它可见期间攒下的标记清掉，否则一分屏关闭就冒出一个假活动点。
  useEffect(() => {
    if (!splitId) return;
    return () => clearActivity(splitId);
  }, [splitId, clearActivity]);

  useEffect(() => {
    if (searchOpen) searchInputRef.current?.focus();
  }, [searchOpen]);

  const registerSearch = useCallback((id: string, handle: TerminalSearchHandle | null) => {
    if (handle) searchHandles.current.set(id, handle);
    else searchHandles.current.delete(id);
  }, []);

  const handleSearchResults = useCallback(
    (id: string, result: { index: number; count: number } | null) => {
      if (id === focusedIdRef.current) setResults(result);
    },
    [],
  );

  const handleFocus = useCallback((id: string) => setFocusedId(id), []);
  const handleDims = useCallback((cols: number, rows: number) => setDims({ cols, rows }), []);
  const openSearch = useCallback(() => setSearchOpen(true), []);

  // 边输边高亮（incremental）；回车才真正跳。空串清掉装饰，不留上一轮的高亮。
  useEffect(() => {
    if (!searchOpen) return;
    // 焦点换过 pane 时把另一边的高亮收掉：留着就是「两个 pane 都在命中」的假象。
    for (const [id, handle] of searchHandles.current) {
      if (id !== focusedId) handle.clear();
    }
    const handle = searchHandles.current.get(focusedId);
    if (!handle) return;
    if (!query) {
      handle.clear();
      setResults(null);
      return;
    }
    handle.findNext(query, caseSensitive, true);
  }, [query, caseSensitive, searchOpen, focusedId]);

  function closeSearch() {
    setSearchOpen(false);
    for (const handle of searchHandles.current.values()) handle.clear();
    setResults(null);
    // 输入框随搜索条一起卸载，焦点不还回去的话接下来敲的键全落在 body 上。
    searchHandles.current.get(focusedId)?.focus();
  }

  function jump(direction: "next" | "previous") {
    if (!query) return;
    const handle = searchHandles.current.get(focusedId);
    if (!handle) return;
    if (direction === "next") handle.findNext(query, caseSensitive);
    else handle.findPrevious(query, caseSensitive);
  }

  function changeFontSize(delta: number) {
    // 用 ref 而不是渲染作用域里的 fontSize：连着按两次时那一份还是旧值，两次都会算出同一档，
    // 结果就是「按了两下只大了一号」。Ctrl+滚轮那一路本来就走 ref。
    const next = nextFontSize(fontSizeRef.current, delta);
    if (next === fontSizeRef.current) return;
    fontSizeRef.current = next;
    onFontSizeChange(next);
  }

  function reconnect(tab: TerminalTab) {
    // 先关再开：旧通道还挂在登记表上就占着一个名额，而重连要的正是那一个名额。
    void onClose(tab.id).then(() => onOpen(tab.serverId));
  }

  /** 右键「在右侧分屏显示」：分的就是当前标签时，把它换到右边、左边让给另一个。 */
  function splitWith(id: string) {
    if (id === activeId) {
      const other = pickSplitTarget(tabs, id);
      if (!other) return;
      onSelect(other);
    }
    setSplitId(id);
  }

  // 切标签 / 关标签 / 搜索的快捷键。复制/粘贴/全选在 TerminalView 里做（要拿 xterm 实例），
  // 这里只处理工作区级的四条，两边动作集不重叠，同一个按键不会被做两次。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const action = resolveShortcut(event);
      if (action !== "prevTab" && action !== "nextTab" && action !== "closeTab" && action !== "find")
        return;
      if (!event.defaultPrevented) {
        // 焦点在输入框（搜索框、弹窗表单）里时，这几个组合键归输入框，不动标签。
        // 终端里发出来的那一路已经被 xterm 的按键钩子 preventDefault 过了。
        const target = event.target as HTMLElement | null;
        if (target?.closest("input, textarea, select, [contenteditable]")) return;
      }
      event.preventDefault();
      if (action === "find") {
        setSearchOpen(true);
        return;
      }
      if (tabs.length === 0) return;
      if (action === "closeTab") {
        if (active) void onClose(active.id);
        return;
      }
      const index = tabs.findIndex((tab) => tab.id === activeId);
      const step = action === "prevTab" ? -1 : 1;
      const next = tabs[(index + step + tabs.length) % tabs.length];
      if (next) onSelect(next.id);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, activeId, onClose, onSelect, tabs]);

  function tabMenuRows(tab: TerminalTab): ContextMenuRow[] {
    return [
      {
        kind: "item",
        key: "reconnect",
        label: tab.closed ? t("terminal.reconnect") : t("terminal.newTabSameServer"),
        icon: <RotateCw className="size-3.5" />,
        onClick: () => (tab.closed ? reconnect(tab) : onOpen(tab.serverId)),
      },
      {
        kind: "item",
        key: "split",
        label: t("terminal.splitRight"),
        icon: <Columns2 className="size-3.5" />,
        disabled: tabs.length < 2 || tab.id === splitId,
        onClick: () => splitWith(tab.id),
      },
      {
        kind: "item",
        key: "closeOthers",
        label: t("terminal.closeOthers"),
        icon: <ListChecks className="size-3.5" />,
        disabled: tabs.length <= 1,
        onClick: () => void onCloseOthers(tab.id),
      },
      {
        kind: "item",
        key: "close",
        label: t("terminal.closeTab"),
        icon: <X className="size-3.5" />,
        danger: true,
        onClick: () => void onClose(tab.id),
      },
    ];
  }

  if (tabs.length === 0) {
    return (
      <div className="flex h-full min-h-0 flex-1 flex-col">
        {/* 传输不随标签结束：关掉最后一个标签时它还在后台跑，进度与取消入口不能跟着消失。
            这里不给 zmodem 提示——没有标签就没有 active，那两个指路按钮点了也没处落。 */}
        <TransferStrip
          entries={transfers}
          zmodemHint={false}
          onPick={() => undefined}
          onCancel={onCancelTransfer}
          onDismissZmodem={() => undefined}
        />
        <div className="flex min-h-0 flex-1 flex-col justify-center px-4 py-5">
          <EmptyState
            icon={<TerminalSquare className="size-4.5" />}
            title={t("terminal.emptyTitle")}
            description={
              selectedServer
                ? t("terminal.emptySelected", {
                    name: selectedServer.name,
                    target: `${selectedServer.username}@${selectedServer.host}:${selectedServer.port}`,
                  })
                : t("terminal.emptyDesc")
            }
            action={
              <div className="flex flex-col items-center gap-2">
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <Button
                    icon={<Plus className="size-4" />}
                    disabled={!plusTarget || busy}
                    onClick={() => onOpen(plusTarget)}
                  >
                    {t("terminal.openFor")}
                  </Button>
                  <Button
                    variant="secondary"
                    disabled={!selectedServer}
                    icon={<Upload className="size-4" />}
                    onClick={() => selectedServer && onPickTransfer(selectedServer.id, "upload")}
                  >
                    {t("transfer.uploadButton")}
                  </Button>
                  <Button
                    variant="secondary"
                    disabled={!selectedServer}
                    icon={<Download className="size-4" />}
                    onClick={() => selectedServer && onPickTransfer(selectedServer.id, "download")}
                  >
                    {t("transfer.downloadButton")}
                  </Button>
                </div>
                <button
                  type="button"
                  className="text-[11px] text-ink-faint underline underline-offset-2 hover:text-ink"
                  onClick={() => setHelpOpen(true)}
                >
                  {t("terminal.shortcuts")}
                </button>
              </div>
            }
          />
        </div>
        <ShortcutsModal open={helpOpen} onClose={() => setHelpOpen(false)} />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-line px-2 py-1.5">
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {tabs.map((tab) => {
            const isActive = tab.id === activeId;
            const hasActivity = !isActive && tab.id !== splitId && activityIds.includes(tab.id);
            return (
              <div
                key={tab.id}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ x: event.clientX, y: event.clientY, tab });
                }}
                onMouseDown={(event) => {
                  // 中键关标签：与浏览器、与其他终端客户端一致。
                  if (event.button !== 1) return;
                  event.preventDefault();
                  void onClose(tab.id);
                }}
                className={cn(
                  "flex shrink-0 items-center rounded-md border text-xs transition-colors select-none",
                  isActive
                    ? "border-line-strong bg-chip text-ink"
                    : "border-transparent text-ink-dim hover:bg-hover",
                )}
              >
                <button
                  type="button"
                  className="flex max-w-44 items-center gap-1.5 py-1 pr-1 pl-2.5"
                  onClick={() => onSelect(tab.id)}
                  title={`${tab.serverName} · ${
                    tab.closed ? t("terminal.closedBadge") : t("terminal.aliveBadge")
                  }`}
                >
                  <span
                    className={cn(
                      "size-1.5 shrink-0 rounded-full",
                      tab.closed ? "bg-neg" : hasActivity ? "animate-pulse bg-brand" : "bg-pos",
                    )}
                    aria-hidden
                  />
                  <span className="truncate">{tabDisplayName(tabs, tab)}</span>
                  {tab.id === splitId && (
                    <Columns2 className="size-3 shrink-0 text-ink-faint" aria-hidden />
                  )}
                </button>
                <button
                  type="button"
                  className="grid size-5 shrink-0 place-items-center rounded text-ink-faint transition-colors hover:bg-hover hover:text-ink"
                  onClick={() => void onClose(tab.id)}
                  title={t("terminal.closeTab")}
                  aria-label={t("terminal.closeTab")}
                >
                  <X className="size-3" />
                </button>
              </div>
            );
          })}

          <button
            type="button"
            className="grid size-6 shrink-0 place-items-center rounded-md text-ink-faint transition-colors hover:bg-hover hover:text-ink disabled:pointer-events-none disabled:opacity-40"
            onClick={() => onOpen(plusTarget)}
            disabled={!plusTarget || busy}
            title={t("terminal.newTab")}
            aria-label={t("terminal.newTab")}
          >
            <Plus className="size-3.5" />
          </button>
        </div>

        <div className="flex shrink-0 items-center gap-0.5">
          {/* 上传/下载走 SFTP（复用那一台已经在线的会话），与终端本身两不相涉：没有标签时
              这两格按不住——路径要落到「哪一台」是由当前标签决定的。 */}
          <BarButton
            label={t("transfer.uploadButton")}
            disabled={!active}
            onClick={() => {
              if (active) onPickTransfer(active.serverId, "upload");
            }}
          >
            <Upload className="size-3.5" />
          </BarButton>
          <BarButton
            label={t("transfer.downloadButton")}
            disabled={!active}
            onClick={() => {
              if (active) onPickTransfer(active.serverId, "download");
            }}
          >
            <Download className="size-3.5" />
          </BarButton>
          <BarButton
            label={splitId ? t("terminal.splitOff") : t("terminal.splitOn")}
            active={!!splitId}
            disabled={!splitId && !pickSplitTarget(tabs, activeId)}
            onClick={() => setSplitId(splitId ? null : pickSplitTarget(tabs, activeId))}
          >
            <Columns2 className="size-3.5" />
          </BarButton>
          <BarButton
            label={t("terminal.findToggle")}
            active={searchOpen}
            onClick={() => (searchOpen ? closeSearch() : setSearchOpen(true))}
          >
            <Search className="size-3.5" />
          </BarButton>
          <div className="ml-1 flex items-center gap-0.5 rounded-md border border-line bg-field px-0.5">
            <FontButton label={t("terminal.fontSmaller")} onClick={() => changeFontSize(-FONT_SIZE_STEP)}>
              <Minus className="size-3" />
            </FontButton>
            <span className="w-6 text-center font-mono text-[10.5px] text-ink-dim tabular-nums">
              {fontSize}
            </span>
            <FontButton label={t("terminal.fontBigger")} onClick={() => changeFontSize(FONT_SIZE_STEP)}>
              <span className="text-[10px] leading-none">A</span>
            </FontButton>
          </div>
          <BarButton label={t("terminal.shortcuts")} onClick={() => setHelpOpen(true)}>
            <Keyboard className="size-3.5" />
          </BarButton>
        </div>
      </div>

      {searchOpen && (
        <div className="flex shrink-0 items-center gap-1 border-b border-line bg-panel px-2 py-1">
          <Search className="size-3.5 shrink-0 text-ink-faint" aria-hidden />
          <Input
            ref={searchInputRef}
            className="h-6 w-56 text-xs"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                jump(event.shiftKey ? "previous" : "next");
              }
              if (event.key === "Escape") {
                event.preventDefault();
                closeSearch();
              }
            }}
            placeholder={t("terminal.findPlaceholder")}
            aria-label={t("terminal.findPlaceholder")}
          />
          <span className="min-w-14 text-[11px] text-ink-faint tabular-nums">
            {query && results
              ? results.count === 0
                ? t("terminal.findNoMatch")
                : results.index < 0
                  ? // 命中数超过 addon 的上限时没有「当前第几个」，只报总数。
                    String(results.count)
                  : t("terminal.findCount", { index: results.index + 1, count: results.count })
              : ""}
          </span>
          <BarButton label={t("terminal.findPrevious")} onClick={() => jump("previous")}>
            <ChevronUp className="size-3.5" />
          </BarButton>
          <BarButton label={t("terminal.findNext")} onClick={() => jump("next")}>
            <ChevronDown className="size-3.5" />
          </BarButton>
          <BarButton
            label={t("terminal.findCase")}
            active={caseSensitive}
            onClick={() => setCaseSensitive((prev) => !prev)}
          >
            <CaseSensitive className="size-3.5" />
          </BarButton>
          <BarButton label={t("terminal.findClose")} onClick={closeSearch}>
            <X className="size-3.5" />
          </BarButton>
        </div>
      )}

      {active?.closed && (
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-warn/25 bg-warn-soft px-3 py-1.5 text-[11.5px] text-warn">
          <span className="min-w-0 truncate">
            {active.reason
              ? t("terminal.closedReason", { reason: active.reason })
              : t("terminal.closedPlain")}
          </span>
          <Button
            size="sm"
            variant="secondary"
            icon={<RotateCw className="size-3.5" />}
            onClick={() => reconnect(active)}
          >
            {t("terminal.reconnect")}
          </Button>
        </div>
      )}

      {/* 传输条带：正在跑的进度 +（这个标签里）那句 ZMODEM 指路。它必须在 pane 之外——
          那一段是 `flex min-h-0 flex-1`，条带不带 shrink-0 就会被终端挤掉。 */}
      <TransferStrip
        entries={transfers}
        zmodemHint={zmodemHint}
        onPick={(mode) => {
          if (active) onPickTransfer(active.serverId, mode);
        }}
        onCancel={onCancelTransfer}
        onDismissZmodem={onDismissZmodem}
      />

      {/* 每个标签都挂着，只是不显示：切走时销毁实例会把已经打出来的输出一起丢掉。
          分屏不另挂一份实例——同一个 map 里改可见性，两个 pane 各占一半宽。
          渲染顺序按「左=active、右=split」排，不按 tabs 数组：数组里 split 那个排前面时，
          照原序渲染就把它画到了左边，与状态栏/边框的左右口径整个反过来（key 不变，挪动不重挂载）。 */}
      <div className="flex min-h-0 flex-1 overflow-hidden bg-sunken">
        {[
          ...tabs.filter((tab) => tab.id === activeId),
          ...tabs.filter((tab) => tab.id === splitId && tab.id !== activeId),
          ...tabs.filter((tab) => tab.id !== activeId && tab.id !== splitId),
        ].map((tab) => {
          const isLeft = tab.id === activeId;
          const isRight = tab.id === splitId && splitId !== activeId;
          const visible = isLeft || isRight;
          return (
            <div
              key={tab.id}
              className={cn(
                "h-full min-w-0",
                visible ? "flex-1" : "hidden",
                isRight && "border-l border-line-strong",
              )}
            >
              <TerminalView
                tab={tab}
                active={visible}
                fontSize={fontSize}
                onFontSizeChange={onFontSizeChange}
                attach={attach}
                detach={detach}
                onFind={openSearch}
                onFocus={handleFocus}
                onDims={handleDims}
                registerSearch={registerSearch}
                onSearchResults={handleSearchResults}
              />
            </div>
          );
        })}
      </div>

      <div className="flex shrink-0 items-center gap-2.5 border-t border-line bg-panel px-3 py-1 text-[11px] text-ink-dim">
        <span
          className={cn("size-1.5 shrink-0 rounded-full", hostLinkClass[activeLink])}
          title={activeLinkLabel}
          aria-label={activeLinkLabel}
        />
        <span className="min-w-0 truncate font-mono">
          {activeServer
            ? `${activeServer.username}@${activeServer.host}:${activeServer.port}`
            : (active?.serverName ?? "")}
        </span>
        {active?.closed && <span className="shrink-0 text-neg">{t("terminal.closedBadge")}</span>}
        {/* 断了就地说清楚并给一个重连按钮：以前这里只有一个红点，原因得回左边树上悬停。 */}
        {activeServer && activeLink !== "online" && (
          <>
            <span
              className={cn(
                "max-w-48 shrink-0 truncate",
                activeLink === "retrying" ? "text-warn" : "text-neg",
              )}
              title={activeLinkLabel}
            >
              {activeLinkLabel}
            </span>
            <BarButton
              label={t("terminal.reconnect")}
              disabled={reconnectingId === activeServer.id}
              onClick={() => onReconnect(activeServer.id)}
            >
              <RotateCw
                className={cn("size-3.5", reconnectingId === activeServer.id && "animate-spin")}
              />
            </BarButton>
          </>
        )}
        <span className="min-w-0 flex-1" />
        <Vitals status={activeStatus} />
        {dims && (
          <span className="shrink-0 font-mono text-ink-faint tabular-nums">
            {dims.cols}×{dims.rows}
          </span>
        )}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          rows={tabMenuRows(menu.tab)}
          onClose={() => setMenu(null)}
        />
      )}

      <ShortcutsModal open={helpOpen} onClose={() => setHelpOpen(false)} />
    </div>
  );
}

/**
 * 快捷键帮助：这些组合键以前只存在于文档和 title 提示里，用户遇到的第一反应是「怎么复制」。
 * 这里列的都是 `resolveShortcut` 真正接住的那几条，改动键表时两边一起改。
 */
function ShortcutsModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const rows: { keys: string; labelKey: string }[] = [
    { keys: "Ctrl+Shift+C", labelKey: "terminal.shortcutCopy" },
    { keys: "Ctrl+Shift+V", labelKey: "terminal.shortcutPaste" },
    { keys: "Ctrl+Shift+A", labelKey: "terminal.shortcutSelectAll" },
    { keys: "Ctrl+Shift+F", labelKey: "terminal.shortcutFind" },
    { keys: "Ctrl+PageUp / Ctrl+PageDown", labelKey: "terminal.shortcutTabs" },
    { keys: "Ctrl+Shift+W", labelKey: "terminal.shortcutCloseTab" },
    { keys: t("terminal.shortcutFontKeys"), labelKey: "terminal.shortcutFont" },
  ];
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("terminal.shortcuts")}
      subtitle={t("terminal.shortcutsSubtitle")}
      width="max-w-md"
    >
      <ul className="flex flex-col gap-2">
        {rows.map((row) => (
          <li
            key={row.labelKey}
            className="flex items-center justify-between gap-3 rounded-md border border-line bg-field px-3 py-1.5"
          >
            <span className="text-[12px] text-ink-dim">{t(row.labelKey)}</span>
            <kbd className="shrink-0 rounded border border-line bg-panel px-1.5 py-0.5 font-mono text-[11px] text-ink">
              {row.keys}
            </kbd>
          </li>
        ))}
      </ul>
    </Modal>
  );
}

function BarButton({
  label,
  onClick,
  active,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className={cn(
        "grid size-6 place-items-center rounded-md transition-colors disabled:pointer-events-none disabled:opacity-40",
        active ? "bg-chip text-ink" : "text-ink-faint hover:bg-hover hover:text-ink",
      )}
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
    >
      {children}
    </button>
  );
}

function FontButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className="grid size-5 place-items-center rounded text-ink-faint transition-colors hover:bg-hover hover:text-ink"
      onClick={onClick}
      title={label}
      aria-label={label}
    >
      {children}
    </button>
  );
}

/**
 * 当前标签这一台的资源状态。
 *
 * 采不到就说采不到（原因挂在悬停上），不退回上一次的数字：这一栏每几秒自己变，
 * 一个停在原地的百分比会被读成「实时但没变化」。
 */
function Vitals({ status }: { status: ServerHostStatus | undefined }) {
  const { t } = useTranslation();
  const view = metricsView(status);

  return (
    <div className="flex shrink-0 items-center gap-2.5 text-[11px]" title={sampledTip(status)}>
      {view.kind === "values" ? (
        <>
          <Metric
            label="CPU"
            percent={view.metrics.cpuPercent}
            color={usageTextColor[usageTone(view.metrics.cpuPercent, CPU_WARN_PERCENT)]}
          />
          <Metric
            label="MEM"
            percent={memoryPercent(view.metrics)}
            color={usageTextColor[usageTone(memoryPercent(view.metrics), MEMORY_WARN_PERCENT)]}
          />
        </>
      ) : view.kind === "failed" ? (
        <span className="text-neg" title={view.reason}>
          {t("servers.metricsFailed")}
        </span>
      ) : (
        <span className="text-ink-faint">{t("servers.metricsPending")}</span>
      )}
    </div>
  );
}

function Metric({ label, percent, color }: { label: string; percent: number; color: string }) {
  return (
    <span className={cn("font-mono tabular-nums", color)}>
      <span className="mr-1 font-sans text-ink-faint">{label}</span>
      {percent}%
    </span>
  );
}

function sampledTip(status: ServerHostStatus | undefined): string | undefined {
  if (!status?.sampledAt) return undefined;
  return `${i18n.t("servers.sampledAt")}: ${status.sampledAt}`;
}
