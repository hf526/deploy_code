import { useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  AlignJustify,
  Cable,
  Copy,
  Download,
  EllipsisVertical,
  KeyRound,
  Lock,
  Plus,
  RotateCw,
  Settings,
  ShieldCheck,
  TerminalSquare,
  Trash2,
  Upload,
  Wifi,
} from "lucide-react";

import { Button, ContextMenu, Input, type ContextMenuRow } from "../../components/ui";
import i18n from "../../lib/i18n";
import { hostLink, hostLinkClass, metricsView } from "../../lib/metrics";
import { stepIndex } from "../../lib/serverTree";
import { activeTunnelCount, findHostStatus, tunnelTone } from "../../lib/tunnel";
import { authSummary, cn } from "../../lib/utils";
import type { ServerConfig, ServerHostStatus, TransferDirection } from "../../lib/types";
import { MetricsBars } from "./MetricsBars";

/**
 * 会话树：一台一个节点，双击即开终端。
 *
 * 高频动作（打开终端 / 重连）在悬停时露出行内按钮，低频的（配置 / 删除 / 测试 / 隧道 / 安全 /
 * 上传下载）收进右键菜单；`⋯` 是给不知道可以右键的人留的入口。搜索框支持 ↑↓ 选行、回车直接
 * 打开第一台，树本身也可以用方向键上下移动。
 */
export function ServerTree({
  servers,
  statuses,
  selectedId,
  tabCounts,
  testingId,
  compact,
  width,
  reconnectingId,
  onSelect,
  onOpenTerminal,
  onTest,
  onTunnels,
  onSecurity,
  onEdit,
  onDelete,
  onTransfer,
  onCopyAddress,
  onReconnect,
  onToggleCompact,
  onAdd,
}: {
  servers: ServerConfig[];
  statuses: ServerHostStatus[];
  selectedId: string;
  tabCounts: Record<string, number>;
  testingId: string | null;
  compact: boolean;
  width: number;
  reconnectingId: string | null;
  onSelect: (serverId: string) => void;
  onOpenTerminal: (server: ServerConfig) => void;
  onTest: (server: ServerConfig) => void;
  onTunnels: (server: ServerConfig) => void;
  onSecurity: (server: ServerConfig) => void;
  onEdit: (server: ServerConfig) => void;
  onDelete: (server: ServerConfig) => void;
  onTransfer: (server: ServerConfig, mode: TransferDirection) => void;
  onCopyAddress: (server: ServerConfig) => void;
  onReconnect: (server: ServerConfig) => void;
  onToggleCompact: () => void;
  onAdd: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  // 搜索结果里的「光标」：↑↓ 移动，回车打开它（还没移过就打开第一台）。
  const [cursor, setCursor] = useState(-1);
  const [menu, setMenu] = useState<{ x: number; y: number; server: ServerConfig } | null>(null);

  useEffect(() => {
    setCursor(-1);
  }, [query]);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return servers;
    return servers.filter((server) =>
      [server.name, server.host, server.username, `${server.host}:${server.port}`]
        .join("\n")
        .toLowerCase()
        .includes(needle),
    );
  }, [query, servers]);

  function openMenu(event: MouseEvent<HTMLElement>, server: ServerConfig) {
    event.preventDefault();
    event.stopPropagation();
    onSelect(server.id);
    setMenu({ x: event.clientX, y: event.clientY, server });
  }

  /** 行与行之间的方向键：焦点跟着走，选中态跟焦点一致。 */
  function moveRowFocus(event: ReactKeyboardEvent<HTMLElement>, delta: number) {
    const list = event.currentTarget.closest("ul");
    if (!list) return;
    const rows = Array.from(list.querySelectorAll<HTMLElement>("[data-server-row]"));
    const next = rows[rows.indexOf(event.currentTarget) + delta];
    if (!next) return;
    onSelect(next.dataset.serverRow ?? "");
    next.focus();
  }

  function menuRows(server: ServerConfig): ContextMenuRow[] {
    return [
      {
        kind: "item",
        key: "terminal",
        label: t("servers.terminalButton"),
        icon: <TerminalSquare className="size-3.5" />,
        onClick: () => onOpenTerminal(server),
      },
      {
        kind: "item",
        key: "upload",
        label: t("transfer.uploadButton"),
        icon: <Upload className="size-3.5" />,
        onClick: () => onTransfer(server, "upload"),
      },
      {
        kind: "item",
        key: "download",
        label: t("transfer.downloadButton"),
        icon: <Download className="size-3.5" />,
        onClick: () => onTransfer(server, "download"),
      },
      { kind: "divider", key: "d1" },
      {
        kind: "item",
        key: "test",
        label: t("servers.testConnection"),
        icon: <Wifi className="size-3.5" />,
        disabled: testingId !== null,
        onClick: () => onTest(server),
      },
      {
        kind: "item",
        key: "tunnel",
        label: t("servers.tunnelButton"),
        icon: <Cable className="size-3.5" />,
        onClick: () => onTunnels(server),
      },
      {
        kind: "item",
        key: "security",
        label: t("servers.securityButton"),
        icon: <ShieldCheck className="size-3.5" />,
        onClick: () => onSecurity(server),
      },
      { kind: "divider", key: "d2" },
      {
        kind: "item",
        key: "copy",
        label: t("servers.copySshAddress"),
        icon: <Copy className="size-3.5" />,
        onClick: () => onCopyAddress(server),
      },
      {
        kind: "item",
        key: "edit",
        label: t("servers.configure"),
        icon: <Settings className="size-3.5" />,
        onClick: () => onEdit(server),
      },
      {
        kind: "item",
        key: "delete",
        label: t("common.delete"),
        icon: <Trash2 className="size-3.5" />,
        danger: true,
        onClick: () => onDelete(server),
      },
    ];
  }

  return (
    <div className="flex h-full min-h-0 shrink-0 flex-col border-r border-line" style={{ width }}>
      <div className="flex shrink-0 items-center gap-2 px-3 pt-3 pb-2">
        <p className="min-w-0 flex-1 truncate text-[11px] font-medium tracking-widest text-ink-faint select-none">
          {t("servers.treeTitle")}
        </p>
        <span className="shrink-0 text-[11px] text-ink-faint tabular-nums">{servers.length}</span>
        <Button
          size="sm"
          variant="ghost"
          className={compact ? "bg-chip text-ink" : undefined}
          title={compact ? t("servers.detailedTree") : t("servers.compactTree")}
          aria-label={compact ? t("servers.detailedTree") : t("servers.compactTree")}
          aria-pressed={compact}
          icon={<AlignJustify className="size-3.5" />}
          onClick={onToggleCompact}
        />
        <Button
          size="sm"
          variant="ghost"
          title={t("servers.add")}
          aria-label={t("servers.add")}
          icon={<Plus className="size-3.5" />}
          onClick={onAdd}
        />
      </div>

      <div className="shrink-0 px-3 pb-2">
        <Input
          className="h-7 text-xs"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              setCursor((prev) => stepIndex(prev, event.key === "ArrowDown" ? 1 : -1, visible.length));
              return;
            }
            if (event.key === "Enter") {
              event.preventDefault();
              // 箭头移动后过滤条件可能又变短了：落点夹回最后一行，别让回车空按。
              const index = Math.min(cursor, visible.length - 1);
              const target = visible[index >= 0 ? index : 0];
              if (target) onOpenTerminal(target);
            }
          }}
          placeholder={t("servers.searchPlaceholder")}
          aria-label={t("servers.searchPlaceholder")}
        />
      </div>

      <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {visible.map((server, index) => {
          const status = findHostStatus(statuses, server.id);
          const { link } = hostLink(status);
          const enabledTunnels = activeTunnelCount(server.tunnels);
          const tabs = tabCounts[server.id] ?? 0;
          const selected = server.id === selectedId;
          const reconnecting = reconnectingId === server.id;
          return (
            <li key={server.id} className="mb-0.5">
              <div
                role="button"
                tabIndex={0}
                data-server-row={server.id}
                aria-current={selected}
                title={linkTip(status)}
                onClick={() => onSelect(server.id)}
                onFocus={() => onSelect(server.id)}
                onDoubleClick={() => onOpenTerminal(server)}
                onContextMenu={(event) => openMenu(event, server)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    onOpenTerminal(server);
                    return;
                  }
                  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    event.preventDefault();
                    moveRowFocus(event, event.key === "ArrowDown" ? 1 : -1);
                  }
                }}
                className={cn(
                  "group cursor-pointer rounded-md border transition-colors outline-none",
                  "focus-visible:ring-2 focus-visible:ring-brand-line",
                  compact ? "px-2.5 py-1.5" : "px-2.5 py-2",
                  selected
                    ? "border-brand-line bg-brand-soft"
                    : "border-transparent hover:bg-hover",
                  index === cursor && "ring-1 ring-brand-line",
                )}
              >
                <div className="flex items-center gap-1.5">
                  <span
                    className={cn("size-2 shrink-0 rounded-full", hostLinkClass[link])}
                    aria-hidden
                  />
                  <p className="min-w-0 flex-1 truncate text-[13px] font-medium tracking-tight text-ink">
                    {server.name}
                  </p>
                  {/* 隧道用图标、标签数用数字：两个都写成数字时界面上分不出「1 条隧道」和
                      「1 个标签」，而那正是这一行最需要各自看清的两件事。 */}
                  {enabledTunnels > 0 && (
                    <span
                      className={cn(
                        "grid size-4 shrink-0 place-items-center rounded",
                        tunnelBadgeClass(tunnelTone(status, enabledTunnels)),
                      )}
                      title={t("servers.tunnelBadge", { count: enabledTunnels })}
                    >
                      <Cable className="size-3" />
                    </span>
                  )}
                  {tabs > 0 && (
                    <span className="shrink-0 rounded bg-chip px-1.5 py-0.5 font-mono text-[10px] text-ink-dim tabular-nums">
                      {tabs}
                    </span>
                  )}
                  {/* 悬停才露出的两个高频动作：打开终端 / 重连。平时不占视觉，也不挤占标题。 */}
                  <button
                    type="button"
                    className="grid size-5 shrink-0 place-items-center rounded text-ink-faint opacity-0 transition-opacity group-hover:opacity-100 hover:bg-hover hover:text-ink focus-visible:opacity-100"
                    aria-label={t("servers.openTerminal")}
                    title={t("servers.openTerminal")}
                    onClick={(event) => {
                      event.stopPropagation();
                      onOpenTerminal(server);
                    }}
                  >
                    <TerminalSquare className="size-3.5" />
                  </button>
                  {(link === "retrying" || link === "down") && (
                    <button
                      type="button"
                      className="grid size-5 shrink-0 place-items-center rounded text-ink-faint opacity-0 transition-opacity group-hover:opacity-100 hover:bg-hover hover:text-ink focus-visible:opacity-100"
                      aria-label={t("servers.tunnelReconnect")}
                      title={t("servers.tunnelReconnect")}
                      disabled={reconnecting}
                      onClick={(event) => {
                        event.stopPropagation();
                        onReconnect(server);
                      }}
                    >
                      <RotateCw className={cn("size-3.5", reconnecting && "animate-spin")} />
                    </button>
                  )}
                  <button
                    type="button"
                    className="grid size-5 shrink-0 place-items-center rounded text-ink-faint transition-colors hover:bg-hover hover:text-ink"
                    aria-label={t("servers.rowMenu")}
                    title={t("servers.rowMenu")}
                    onClick={(event) => openMenu(event, server)}
                  >
                    <EllipsisVertical className="size-3.5" />
                  </button>
                </div>

                <p className="mt-0.5 flex items-center gap-1.5 truncate font-mono text-[11px] text-ink-dim">
                  {/* 认证方式用一个小图标表示，悬停给的是完整说法：「密钥: id_ed25519」/「密码认证」。 */}
                  <span
                    className="shrink-0 text-ink-faint"
                    title={authSummary(server)}
                    aria-label={authSummary(server)}
                  >
                    {server.auth.type === "password" ? (
                      <Lock className="size-3" />
                    ) : (
                      <KeyRound className="size-3" />
                    )}
                  </span>
                  <span className="min-w-0 truncate">
                    {server.username}@{server.host}:{server.port}
                  </span>
                  {(link === "retrying" || link === "down") && (
                    <span
                      className={cn(
                        "ml-auto shrink-0 font-sans text-[10px]",
                        link === "retrying" ? "text-warn" : "text-neg",
                      )}
                      title={linkTip(status)}
                    >
                      {link === "retrying"
                        ? t("servers.linkRetryingShort", { retries: status?.retries ?? 0 })
                        : t("servers.linkDownShort")}
                    </span>
                  )}
                </p>

                {!compact && <MetricsBars view={metricsView(status)} />}

                {!compact && server.defaultTargetDir && (
                  <p
                    className="mt-0.5 truncate font-mono text-[10px] text-ink-faint"
                    title={`${t("servers.defaultTargetDir")} ${server.defaultTargetDir}`}
                  >
                    {server.defaultTargetDir}
                  </p>
                )}
              </div>
            </li>
          );
        })}

        {servers.length === 0 ? (
          <li className="px-2 py-6 text-center text-[11px] leading-relaxed text-ink-faint">
            {t("servers.treeEmpty")}
          </li>
        ) : visible.length === 0 ? (
          <li className="px-2 py-6 text-center text-[11px] text-ink-faint">
            {t("servers.treeNoMatch", { query })}
          </li>
        ) : null}
      </ul>

      <p className="shrink-0 border-t border-line px-3 py-2 text-[10.5px] leading-relaxed text-ink-faint select-none">
        {t("servers.treeHint")}
      </p>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          rows={menuRows(menu.server)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

/** 徽章配色沿用 tunnelTone 的结论，只是换成行内的小方块。 */
function tunnelBadgeClass(tone: "green" | "amber" | "red" | "gray"): string {
  if (tone === "green") return "bg-pos-soft text-pos";
  if (tone === "red") return "bg-neg-soft text-neg";
  if (tone === "amber") return "bg-warn-soft text-warn";
  return "bg-field text-ink-faint";
}

/**
 * 悬停说明：在线给连接时刻，重连中带上下一次也说不清的「第几次」与后端那句原因，
 * 断开只说断开加原因。没有状态（后端这轮没给这一台）不谎报。
 */
function linkTip(status: ServerHostStatus | undefined): string {
  // 与状态点共用 `hostLink` 那一份判定：两套判法会让「刚登记、还没连过」的那几秒出现
  // 点是灰的（还不知道）、悬停却说「已断开」。
  const link = hostLink(status).link;
  if (!status || link === "unknown") return i18n.t("servers.linkUnknown");
  if (link === "online") return i18n.t("servers.linkOnline", { time: status.connectedAt ?? "-" });
  if (link === "retrying") {
    return status.lastError
      ? i18n.t("servers.linkRetryingReason", { retries: status.retries, reason: status.lastError })
      : i18n.t("servers.linkRetrying", { retries: status.retries });
  }
  return status.lastError
    ? i18n.t("servers.linkDownReason", { reason: status.lastError })
    : i18n.t("servers.linkDown");
}
