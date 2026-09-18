import { lazy, Suspense, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";

import { RepoTabsBar } from "./components/RepoTabsBar";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Toasts } from "./components/ui";
import i18n, { applyLanguage } from "./lib/i18n";
import { useApp } from "./lib/store";
import { useTauriEvent } from "./lib/useTauriEvent";
import type { BackupEvent, DeployEvent, PagesEvent, SchedulerNotice } from "./lib/types";

// 按页面分包：启动只加载首屏，其余页面首次访问时按需加载。
const BackupsPage = lazy(() => import("./pages/BackupsPage"));
const DeployPage = lazy(() => import("./pages/DeployPage"));
const HistoryPage = lazy(() => import("./pages/HistoryPage"));
const NginxPage = lazy(() => import("./pages/NginxPage"));
const RepoDetailPage = lazy(() => import("./pages/RepoDetailPage"));
const ReposPage = lazy(() => import("./pages/ReposPage"));
const ServersPage = lazy(() => import("./pages/ServersPage"));
const SettingsPage = lazy(() => import("./pages/SettingsPage"));

function ReposEntry() {
  const tabs = useApp((state) => state.tabs);
  const location = useLocation();
  // 有打开的仓库标签时，「仓库」默认回到最近查看的工作区；
  // 仅在从工作区左上角返回（?list=1）或没有标签时显示项目列表。
  const showList = new URLSearchParams(location.search).get("list") === "1";
  if (!showList && tabs.length > 0) {
    return <Navigate to={`/repos/${tabs[tabs.length - 1]}`} replace />;
  }
  return <ReposPage />;
}

function TabsAndRoutes() {
  const { t } = useTranslation();
  const location = useLocation();
  const repoRouteId = location.pathname.match(/^\/repos\/([^/]+)/)?.[1] ?? null;
  const showTabs = repoRouteId !== null || location.pathname.startsWith("/deploy");
  return (
    <div className="flex min-w-0 flex-1 flex-col">
      {showTabs && <RepoTabsBar activeRepoId={repoRouteId ?? ""} />}
      <main className="min-h-0 flex-1 overflow-hidden">
        <Suspense
          fallback={
            <div className="flex h-full items-center justify-center text-xs text-ink-faint">
              {t("common.loading")}
            </div>
          }
        >
          <Routes>
            <Route path="/" element={<Navigate to="/repos" replace />} />
            <Route path="/repos" element={<ReposEntry />} />
            <Route path="/repos/:repoId" element={<RepoDetailPage />} />
            <Route path="/deploy" element={<DeployPage />} />
            <Route path="/servers" element={<ServersPage />} />
            <Route path="/nginx" element={<NginxPage />} />
            <Route path="/backups" element={<BackupsPage />} />
            <Route path="/history" element={<HistoryPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/repos" replace />} />
          </Routes>
        </Suspense>
      </main>
    </div>
  );
}

export default function App() {
  const loadAll = useApp((state) => state.loadAll);
  const settingsLanguage = useApp((state) => state.settings.language);
  const handleDeployEvent = useApp((state) => state.handleDeployEvent);
  const handleBackupEvent = useApp((state) => state.handleBackupEvent);
  const handlePagesEvent = useApp((state) => state.handlePagesEvent);
  const toast = useApp((state) => state.toast);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  useEffect(() => {
    applyLanguage(settingsLanguage);
  }, [settingsLanguage]);

  useTauriEvent<DeployEvent>("deploy://event", handleDeployEvent);
  useTauriEvent<BackupEvent>("backup://event", handleBackupEvent);
  useTauriEvent<PagesEvent>("pages://event", handlePagesEvent);
  useTauriEvent<SchedulerNotice>("scheduler://notice", (notice) => {
    if (notice.kind === "started") {
      toast("success", i18n.t("backup.schedule.started"));
    } else if (notice.kind === "noConfig") {
      toast("error", i18n.t("backup.schedule.noConfig"));
    } else {
      toast("error", i18n.t("backup.schedule.failed", { error: notice.message ?? "" }));
    }
  });

  return (
    <HashRouter>
      <div className="flex h-full flex-col overflow-hidden bg-panel">
        <TitleBar />
        <div className="flex min-h-0 flex-1">
          <Sidebar />
          <TabsAndRoutes />
          <Toasts />
        </div>
        <StatusBar />
      </div>
    </HashRouter>
  );
}
