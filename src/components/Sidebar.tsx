import { useState } from "react";
import {
  ChevronsLeft,
  ChevronsRight,
  Container,
  Database,
  FolderOpen,
  GitBranch,
  History,
  Monitor,
  Moon,
  Network,
  Rocket,
  Server,
  Settings,
  Sun,
  Webhook,
  type LucideIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { NavLink, useNavigate } from "react-router-dom";

import { NAV_WIDTH_COLLAPSED, NAV_WIDTH_EXPANDED, readStoredNavCollapsedSafe, writeStoredNavCollapsed } from "../lib/nav";
import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import { useTheme, type ThemeMode } from "../lib/theme";
import { runGuarded } from "../lib/unsavedGuard";
import { cn } from "../lib/utils";
import { ThemeToggle } from "./ThemeToggle";

type NavItem = { to: string; labelKey: string; icon: LucideIcon };

/** 分组顺序按使用顺序排：先管代码与发布，再管线上那台机器，然后是备份与到点自己跑的任务，最后是这台机器自己的东西。 */
const NAV_GROUPS: { labelKey: string; items: NavItem[] }[] = [
  {
    labelKey: "nav.group.release",
    items: [
      { to: "/repos", labelKey: "nav.repos", icon: GitBranch },
      { to: "/deploy", labelKey: "nav.deploy", icon: Rocket },
    ],
  },
  {
    labelKey: "nav.group.servers",
    items: [
      { to: "/servers", labelKey: "nav.servers", icon: Server },
      { to: "/nginx", labelKey: "nav.nginx", icon: Network },
    ],
  },
  {
    labelKey: "nav.group.scheduled",
    items: [
      { to: "/backups", labelKey: "nav.backups", icon: Database },
      { to: "/containers", labelKey: "nav.containers", icon: Container },
      { to: "/cron-jobs", labelKey: "nav.cronJobs", icon: Webhook },
    ],
  },
  {
    labelKey: "nav.group.local",
    items: [
      { to: "/history", labelKey: "nav.history", icon: History },
      { to: "/settings", labelKey: "nav.settings", icon: Settings },
    ],
  },
];

/** 收起后只剩图标，主题切换也不能还是三段：一个按钮按 浅色 → 深色 → 跟随系统 循环。 */
const THEME_CYCLE: Array<{ mode: ThemeMode; titleKey: string; icon: typeof Sun }> = [
  { mode: "light", titleKey: "theme.lightTitle", icon: Sun },
  { mode: "dark", titleKey: "theme.darkTitle", icon: Moon },
  { mode: "system", titleKey: "theme.systemTitle", icon: Monitor },
];

export function Sidebar() {
  const { t } = useTranslation();
  const liveRunning = useApp((state) => state.live?.status === "running");
  const backupRunning = useApp((state) => state.liveBackup?.status === "running");
  const pagesRunning = useApp((state) => state.livePages?.status === "running");
  const containerRunning = useApp((state) => state.liveContainer?.status === "running");
  const navigate = useNavigate();
  // 侧栏宽度是本机视图偏好：默认 192px，收起成 56px 图标栏，与字号 / 树宽同例存 localStorage。
  const [collapsed, setCollapsed] = useState(readStoredNavCollapsedSafe);

  function toggleCollapsed() {
    const next = !collapsed;
    writeStoredNavCollapsed(next);
    setCollapsed(next);
  }

  // Pages 与部署共用 /deploy 这一页，所以它的在跑状态并到部署那一格。
  const runningPaths: Record<string, boolean> = {
    "/deploy": liveRunning || pagesRunning,
    "/backups": backupRunning,
    "/containers": containerRunning,
  };

  return (
    <aside
      className="ui-sidebar flex shrink-0 flex-col overflow-hidden border-r border-line transition-[width] duration-150"
      style={{ width: collapsed ? NAV_WIDTH_COLLAPSED : NAV_WIDTH_EXPANDED }}
    >
      <div className={cn("flex items-center gap-1 px-2 pt-3 pb-2", collapsed && "flex-col")}>
        <button
          type="button"
          title={t("nav.openRepo")}
          aria-label={t("nav.openRepo")}
          onClick={() => void openRepoFolder(navigate)}
          className={cn(
            "ui-btn ui-btn-primary rounded-md",
            collapsed
              ? "grid size-9 place-items-center"
              : "flex h-9 min-w-0 flex-1 items-center justify-center gap-2 text-[13px] font-medium",
          )}
        >
          <FolderOpen className="size-4 shrink-0" />
          {!collapsed && <span className="truncate">{t("nav.openRepo")}</span>}
        </button>
        <button
          type="button"
          onClick={toggleCollapsed}
          title={collapsed ? t("nav.expand") : t("nav.collapse")}
          aria-label={collapsed ? t("nav.expand") : t("nav.collapse")}
          aria-expanded={!collapsed}
          className="grid size-9 shrink-0 place-items-center rounded-md text-ink-faint transition-colors hover:bg-hover hover:text-ink"
        >
          {collapsed ? <ChevronsRight className="size-4" /> : <ChevronsLeft className="size-4" />}
        </button>
      </div>

      <nav
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto",
          collapsed ? "items-center px-1.5" : "px-3",
        )}
      >
        {NAV_GROUPS.map((group, groupIndex) => (
          <div
            key={group.labelKey}
            className={cn(
              "flex shrink-0 flex-col gap-0.5",
              collapsed && "w-full items-center",
              // 收起后没有组标题，用一条细线与上组隔开。
              collapsed && groupIndex > 0 && "mt-2 border-t border-line pt-2",
            )}
          >
            {!collapsed && (
              <p className="px-2.5 pt-3 pb-1 text-[10.5px] font-medium tracking-widest text-ink-faint select-none">
                {t(group.labelKey)}
              </p>
            )}
            {group.items.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                title={collapsed ? t(item.labelKey) : undefined}
                aria-label={collapsed ? t(item.labelKey) : undefined}
                onClick={(event) => {
                  // 离开当前页面可能丢弃编辑器草稿，统一走未保存守卫。
                  event.preventDefault();
                  runGuarded(() => navigate(item.to));
                }}
                className={({ isActive }) =>
                  cn(
                    "ui-nav-item relative flex items-center",
                    collapsed
                      ? "size-9 justify-center rounded-md"
                      : "h-8 gap-2.5 px-2.5 text-[13px] font-medium",
                    isActive && "ui-nav-item-active",
                  )
                }
              >
                <item.icon className="size-4 shrink-0" />
                {!collapsed && <span className="truncate">{t(item.labelKey)}</span>}
                {runningPaths[item.to] && (
                  <span
                    className={cn(
                      "size-1.5 shrink-0 animate-pulse rounded-full bg-pos",
                      collapsed ? "absolute top-1.5 right-1.5" : "ml-auto",
                    )}
                  />
                )}
              </NavLink>
            ))}
          </div>
        ))}
      </nav>

      <div className={cn("pb-3", collapsed ? "flex justify-center" : "px-3")}>
        {collapsed ? <CollapsedThemeButton /> : <ThemeToggle className="w-full" />}
      </div>
    </aside>
  );
}

function CollapsedThemeButton() {
  const { t } = useTranslation();
  const [mode, setMode] = useTheme();
  const current = THEME_CYCLE.find((item) => item.mode === mode) ?? THEME_CYCLE[0];
  const next = THEME_CYCLE[(THEME_CYCLE.indexOf(current) + 1) % THEME_CYCLE.length];
  const Icon = current.icon;
  return (
    <button
      type="button"
      onClick={() => setMode(next.mode)}
      title={`${t("nav.themeCycle")} · ${t(current.titleKey)}`}
      aria-label={t("nav.themeCycle")}
      className="grid size-9 place-items-center rounded-md text-ink-faint transition-colors hover:bg-hover hover:text-ink"
    >
      <Icon className="size-4" />
    </button>
  );
}
