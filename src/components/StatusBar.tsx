import { useEffect, useState } from "react";
import { Power } from "lucide-react";
import { useTranslation } from "react-i18next";

import { formatCountdown, isImminent, remainingSecs } from "../lib/shutdown";
import { useApp } from "../lib/store";
import { cn } from "../lib/utils";

/** 底部状态栏：与标题栏呼应，展示部署状态与环境信息。 */
export function StatusBar() {
  const { t } = useTranslation();
  const live = useApp((state) => state.live);
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const shutdownStatus = useApp((state) => state.shutdownStatus);
  const cancelShutdown = useApp((state) => state.cancelShutdown);
  const pending = shutdownStatus?.pending ?? null;

  const running = live?.status === "running";
  // 后端进度事件带的是当前步骤文案（如「上传压缩包」），光留百分比会把它丢掉。
  const progressNote = running ? live?.progressMessage.trim() : "";
  const statusText = running
    ? `${t("statusbar.deploying", { percent: live?.progress ?? 0 })}${
        progressNote ? ` · ${progressNote}` : ""
      }`
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

  // 排定关机后才按秒走表：没排定时不挂定时器，状态栏不做无谓的重渲染。
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    if (!pending) return;
    setNowMs(Date.now());
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [pending]);
  const remaining = remainingSecs(pending, nowMs);
  const imminent = shutdownStatus ? isImminent(shutdownStatus, nowMs) : false;

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
      {remaining !== null && (
        <>
          <span className="h-3 w-px bg-line" aria-hidden />
          <span
            className={cn(
              "flex items-center gap-1.5",
              imminent ? "text-neg" : "text-warn",
            )}
          >
            <Power className="size-3 shrink-0" aria-hidden />
            <span className="tabular-nums">
              {t("statusbar.shutdown", { countdown: formatCountdown(remaining) })}
            </span>
            <button
              className="rounded px-1 underline underline-offset-2 hover:bg-hover hover:text-ink"
              onClick={() => void cancelShutdown()}
            >
              {t("statusbar.shutdownCancel")}
            </button>
          </span>
        </>
      )}
      <span className="ml-auto text-ink-faint">DeployCode v0.1.0</span>
    </footer>
  );
}
