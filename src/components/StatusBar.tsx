import { useTranslation } from "react-i18next";

import { useApp } from "../lib/store";
import { cn } from "../lib/utils";

/** 底部状态栏：与标题栏呼应，展示部署状态与环境信息。 */
export function StatusBar() {
  const { t } = useTranslation();
  const live = useApp((state) => state.live);
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);

  const running = live?.status === "running";
  const statusText = running
    ? t("statusbar.deploying", { percent: live?.progress ?? 0 })
    : live?.status === "success"
      ? t("statusbar.lastSuccess")
      : live?.status === "failed"
        ? t("statusbar.lastFailed")
        : t("statusbar.ready");
  const statusTone = running
    ? "bg-brand"
    : live?.status === "success"
      ? "bg-pos"
      : live?.status === "failed"
        ? "bg-neg"
        : "bg-ink-faint";

  return (
    <footer className="ui-titlebar flex h-6 shrink-0 items-center gap-3 border-t border-line px-3 text-[11px] text-ink-dim select-none">
      <span className="flex items-center gap-1.5">
        <span className={cn("size-1.5 rounded-full", statusTone, running && "animate-pulse")} />
        <span className={running || live?.status === "failed" ? "text-ink" : undefined}>{statusText}</span>
      </span>
      <span className="h-3 w-px bg-line" aria-hidden />
      <span className="tabular-nums">
        {t("statusbar.summary", { repos: repos.length, servers: servers.length })}
      </span>
      <span className="ml-auto text-ink-faint">DeployCode v0.1.0</span>
    </footer>
  );
}
