import { useMemo, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  Cable,
  EllipsisVertical,
  KeyRound,
  Lock,
  Plus,
  Settings,
  ShieldCheck,
  TerminalSquare,
  Trash2,
  Wifi,
} from "lucide-react";

import { Button, ContextMenu, Input, type ContextMenuRow } from "../../components/ui";
import i18n from "../../lib/i18n";
import { hostLink, hostLinkClass, metricsView } from "../../lib/metrics";
import { activeTunnelCount, findHostStatus, tunnelTone } from "../../lib/tunnel";
import { authSummary, cn } from "../../lib/utils";
import type { ServerConfig, ServerHostStatus } from "../../lib/types";
import { MetricsBars } from "./MetricsBars";

/**
 * 会话树：一台一个节点，双击即开终端。
 *
 * 原来那一行六个按钮把「打开终端」和「删除」摆得一样重，中间还空着一大片；现在高频动作
 * 是双击与回车，低频的（配置 / 删除）收进菜单，行内只留一个 `⋯` 入口。
 */
export function ServerTree({
  servers,
  statuses,
  selectedId,
  tabCounts,
  testingId,
  width,
  onSelect,
  onOpenTerminal,
  onTest,
  onTunnels,
  onSecurity,
  onEdit,
  onDelete,
  onAdd,
}: {
  servers: ServerConfig[];
  statuses: ServerHostStatus[];
  selectedId: string;
  tabCounts: Record<string, number>;
  testingId: string | null;
  width: number;
  onSelect: (serverId: string) => void;
  onOpenTerminal: (server: ServerConfig) => void;
  onTest: (server: ServerConfig) => void;
  onTunnels: (server: ServerConfig) => void;
  onSecurity: (server: ServerConfig) => void;
  onEdit: (server: ServerConfig) => void;
  onDelete: (server: ServerConfig) => void;
  onAdd: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [menu, setMenu] = useState<{ x: number; y: number; server: ServerConfig } | null>(null);

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
      { kind: "divider", key: "d" },
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
          placeholder={t("servers.searchPlaceholder")}
          aria-label={t("servers.searchPlaceholder")}
        />
      </div>

      <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {visible.map((server) => {
          const status = findHostStatus(statuses, server.id);
          const { link } = hostLink(status);
          const enabledTunnels = activeTunnelCount(server.tunnels);
          const tabs = tabCounts[server.id] ?? 0;
          const selected = server.id === selectedId;
          return (
            <li key={server.id} className="mb-0.5">
              <div
                role="button"
                tabIndex={0}
                aria-current={selected}
                title={linkTip(status)}
                onClick={() => onSelect(server.id)}
                onDoubleClick={() => onOpenTerminal(server)}
                onContextMenu={(event) => openMenu(event, server)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") onOpenTerminal(server);
                }}
                className={cn(
                  "cursor-pointer rounded-md border px-2.5 py-2 transition-colors outline-none",
                  "focus-visible:ring-2 focus-visible:ring-brand-line",
                  selected
                    ? "border-brand-line bg-brand-soft"
                    : "border-transparent hover:bg-hover",
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
                  {/* 认证方式用一个小图标表示（原来那一格 36px 的图标方块在树里放不下），
                      悬停给的是完整说法：「密钥: id_ed25519」/「密码认证」。 */}
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
                  <span className="truncate">
                    {server.username}@{server.host}:{server.port}
                  </span>
                </p>

                <MetricsBars view={metricsView(status)} />

                {server.defaultTargetDir && (
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
