import { Container, Database, FolderOpen, GitBranch, History, Network, Rocket, Server, Settings, Webhook } from "lucide-react";
import { useTranslation } from "react-i18next";
import { NavLink, useNavigate } from "react-router-dom";

import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import { runGuarded } from "../lib/unsavedGuard";
import { cn } from "../lib/utils";
import { ThemeToggle } from "./ThemeToggle";

const NAV_ITEMS = [
  { to: "/repos", labelKey: "nav.repos", icon: GitBranch },
  { to: "/deploy", labelKey: "nav.deploy", icon: Rocket },
  { to: "/servers", labelKey: "nav.servers", icon: Server },
  { to: "/nginx", labelKey: "nav.nginx", icon: Network },
  { to: "/containers", labelKey: "nav.containers", icon: Container },
  { to: "/cron-jobs", labelKey: "nav.cronJobs", icon: Webhook },
  { to: "/backups", labelKey: "nav.backups", icon: Database },
  { to: "/history", labelKey: "nav.history", icon: History },
  { to: "/settings", labelKey: "nav.settings", icon: Settings },
];

export function Sidebar() {
  const { t } = useTranslation();
  const liveRunning = useApp((state) => state.live?.status === "running");
  const backupRunning = useApp((state) => state.liveBackup?.status === "running");
  const pagesRunning = useApp((state) => state.livePages?.status === "running");
  const containerRunning = useApp((state) => state.liveContainer?.status === "running");
  const navigate = useNavigate();

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

      <nav className="flex flex-1 flex-col gap-0.5 px-3">
        <p className="px-2.5 pt-3 pb-1 text-[10.5px] font-medium tracking-widest text-ink-faint select-none">
          {t("nav.section")}
        </p>
        {NAV_ITEMS.map((item) => (
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
            {item.to === "/deploy" && (liveRunning || pagesRunning) && (
              <span className="ml-auto size-1.5 shrink-0 animate-pulse rounded-full bg-pos" />
            )}
            {item.to === "/backups" && backupRunning && (
              <span className="ml-auto size-1.5 shrink-0 animate-pulse rounded-full bg-pos" />
            )}
            {item.to === "/containers" && containerRunning && (
              <span className="ml-auto size-1.5 shrink-0 animate-pulse rounded-full bg-pos" />
            )}
          </NavLink>
        ))}
      </nav>

      <div className="px-3 pb-3">
        <ThemeToggle className="w-full" />
      </div>
    </aside>
  );
}
