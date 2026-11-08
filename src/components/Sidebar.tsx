import { Container, Database, FolderOpen, GitBranch, History, Network, Rocket, Server, Settings, Webhook, type LucideIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { NavLink, useNavigate } from "react-router-dom";

import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
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

export function Sidebar() {
  const { t } = useTranslation();
  const liveRunning = useApp((state) => state.live?.status === "running");
  const backupRunning = useApp((state) => state.liveBackup?.status === "running");
  const pagesRunning = useApp((state) => state.livePages?.status === "running");
  const containerRunning = useApp((state) => state.liveContainer?.status === "running");
  const navigate = useNavigate();

  // Pages 与部署共用 /deploy 这一页，所以它的在跑状态并到部署那一格。
  const runningPaths: Record<string, boolean> = {
    "/deploy": liveRunning || pagesRunning,
    "/backups": backupRunning,
    "/containers": containerRunning,
  };

  return (
    <aside className="ui-sidebar flex w-56 shrink-0 flex-col border-r border-line">
      <div className="px-3 pt-3 pb-2">
        <button
          type="button"
          onClick={() => void openRepoFolder(navigate)}
          className="ui-btn ui-btn-primary flex h-9 w-full items-center justify-center gap-2 rounded-md text-[13px] font-medium"
        >
          <FolderOpen className="size-4 shrink-0" />
          {t("nav.openRepo")}
        </button>
      </div>

      <nav className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-3">
        {NAV_GROUPS.map((group) => (
          <div key={group.labelKey} className="flex shrink-0 flex-col gap-0.5">
            <p className="px-2.5 pt-3 pb-1 text-[10.5px] font-medium tracking-widest text-ink-faint select-none">
              {t(group.labelKey)}
            </p>
            {group.items.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                onClick={(event) => {
                  // 离开当前页面可能丢弃编辑器草稿，统一走未保存守卫。
                  event.preventDefault();
                  runGuarded(() => navigate(item.to));
                }}
                className={({ isActive }) =>
                  cn(
                    "ui-nav-item flex h-8 items-center gap-2.5 px-2.5 text-[13px] font-medium",
                    isActive && "ui-nav-item-active",
                  )
                }
              >
                <item.icon className="size-4" />
                <span>{t(item.labelKey)}</span>
                {runningPaths[item.to] && (
                  <span className="ml-auto size-1.5 shrink-0 animate-pulse rounded-full bg-pos" />
                )}
              </NavLink>
            ))}
          </div>
        ))}
      </nav>

      <div className="px-3 pb-3">
        <ThemeToggle className="w-full" />
      </div>
    </aside>
  );
}
