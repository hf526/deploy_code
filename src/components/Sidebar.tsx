import { FolderOpen, GitBranch, History, Rocket, Server, Settings } from "lucide-react";
import { NavLink, useNavigate } from "react-router-dom";

import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import { cn } from "../lib/utils";
import { ThemeToggle } from "./ThemeToggle";

const NAV_ITEMS = [
  { to: "/repos", label: "仓库", icon: GitBranch },
  { to: "/deploy", label: "部署", icon: Rocket },
  { to: "/servers", label: "服务器", icon: Server },
  { to: "/history", label: "记录", icon: History },
  { to: "/settings", label: "设置", icon: Settings },
];

export function Sidebar() {
  const liveRunning = useApp((state) => state.live?.status === "running");
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
          打开仓库
        </button>
      </div>

      <nav className="flex flex-1 flex-col gap-0.5 px-3">
        <p className="px-2.5 pt-3 pb-1 text-[10.5px] font-medium tracking-widest text-ink-faint select-none">
          导航
        </p>
        {NAV_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }) =>
              cn(
                "ui-nav-item flex h-8 items-center gap-2.5 px-2.5 text-[13px] font-medium",
                isActive && "ui-nav-item-active",
              )
            }
          >
            <item.icon className="size-4" />
            <span>{item.label}</span>
            {item.to === "/deploy" && liveRunning && (
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
