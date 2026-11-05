import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useNavigate } from "react-router-dom";
import { Plus, TerminalSquare, X } from "lucide-react";

import { Badge, Button, EmptyState, Page, Select } from "../components/ui";
import { useApp } from "../lib/store";
import { TerminalView } from "./terminal/TerminalView";
import { useTerminalTabs } from "./terminal/useTerminalTabs";

export default function TerminalPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const location = useLocation();
  const navigate = useNavigate();
  const { tabs, activeId, setActiveId, busy, openFor, closeTab, attach, detach } = useTerminalTabs();

  const [picked, setPicked] = useState("");

  // 服务器页「打开终端」跳过来时带的参数：进来就开一个标签，然后把参数清掉——
  // 不清的话刷新一次就多开一条同样的会话。
  const requested = useMemo(
    () => new URLSearchParams(location.search).get("server") ?? "",
    [location.search],
  );
  useEffect(() => {
    if (!requested) return;
    void openFor(requested);
    navigate({ pathname: "/terminal" }, { replace: true });
  }, [requested, openFor, navigate]);

  useEffect(() => {
    if (!picked && servers.length > 0) setPicked(servers[0].id);
  }, [picked, servers]);

  const active = tabs.find((tab) => tab.id === activeId) ?? null;

  return (
    <Page
      title={t("terminal.title")}
      subtitle={t("terminal.subtitle")}
      scroll={false}
      actions={
        <>
          <Select
            value={picked}
            onChange={(event) => setPicked(event.target.value)}
            className="w-52"
            disabled={servers.length === 0}
          >
            {servers.map((server) => (
              <option key={server.id} value={server.id}>
                {server.name}
              </option>
            ))}
          </Select>
          <Button
            icon={<Plus className="size-4" />}
            disabled={!picked || busy}
            onClick={() => void openFor(picked)}
          >
            {t("terminal.open")}
          </Button>
        </>
      }
    >
      <div className="flex h-full min-h-0 flex-col gap-2">
        {servers.length === 0 ? (
          <EmptyState
            icon={<TerminalSquare className="size-4" />}
            title={t("terminal.noServerTitle")}
            description={t("terminal.noServerDesc")}
          />
        ) : tabs.length === 0 ? (
          <EmptyState
            icon={<TerminalSquare className="size-4" />}
            title={t("terminal.emptyTitle")}
            description={t("terminal.emptyDesc")}
          />
        ) : (
          <>
            <div className="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-line pb-1">
              {tabs.map((tab) => (
                <div
                  key={tab.id}
                  className={
                    tab.id === activeId
                      ? "flex shrink-0 items-center gap-1.5 rounded-md bg-chip px-2.5 py-1 text-xs text-ink"
                      : "flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-xs text-ink-dim hover:bg-hover"
                  }
                >
                  <button
                    type="button"
                    className="max-w-40 truncate"
                    onClick={() => setActiveId(tab.id)}
                    title={tab.serverName}
                  >
                    {tab.serverName}
                  </button>
                  {tab.closed && <Badge kind="gray">{t("terminal.closedBadge")}</Badge>}
                  <button
                    type="button"
                    className="grid size-4 place-items-center rounded text-ink-faint hover:text-ink"
                    onClick={() => void closeTab(tab.id)}
                    title={t("terminal.closeTab")}
                    aria-label={t("terminal.closeTab")}
                  >
                    <X className="size-3" />
                  </button>
                </div>
              ))}
            </div>

            {active?.closed && (
              <div className="flex shrink-0 items-center justify-between gap-3 rounded-md border border-line bg-field px-3 py-2 text-xs text-ink-dim">
                <span className="truncate">
                  {active.reason
                    ? t("terminal.closedReason", { reason: active.reason })
                    : t("terminal.closedPlain")}
                </span>
                <Button
                  variant="ghost"
                  onClick={() => {
                    const serverId = active.serverId;
                    void closeTab(active.id).then(() => openFor(serverId));
                  }}
                >
                  {t("terminal.reopen")}
                </Button>
              </div>
            )}

            {/* 每个标签都挂着，只是不显示：切走时销毁实例会把已经打出来的输出一起丢掉。 */}
            <div className="min-h-0 flex-1 overflow-hidden rounded-md border border-line bg-sunken">
              {tabs.map((tab) => (
                <div key={tab.id} className={tab.id === activeId ? "h-full" : "hidden"}>
                  <TerminalView tab={tab} active={tab.id === activeId} attach={attach} detach={detach} />
                </div>
              ))}
            </div>

            <p className="shrink-0 text-[11px] text-ink-faint">{t("terminal.hint")}</p>
          </>
        )}
      </div>
    </Page>
  );
}
