import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";

import { RepoTabsBar } from "./components/RepoTabsBar";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Toasts } from "./components/ui";
import { useApp } from "./lib/store";
import type { BackupEvent, DeployEvent, PagesEvent } from "./lib/types";
import BackupsPage from "./pages/BackupsPage";
import DeployPage from "./pages/DeployPage";
import HistoryPage from "./pages/HistoryPage";
import PagesPage from "./pages/PagesPage";
import RepoDetailPage from "./pages/RepoDetailPage";
import ReposPage from "./pages/ReposPage";
import ServersPage from "./pages/ServersPage";
import SettingsPage from "./pages/SettingsPage";

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
  const location = useLocation();
  const repoRouteId = location.pathname.match(/^\/repos\/([^/]+)/)?.[1] ?? null;
  const showTabs = repoRouteId !== null || location.pathname.startsWith("/deploy");
  return (
    <div className="flex min-w-0 flex-1 flex-col">
      {showTabs && <RepoTabsBar activeRepoId={repoRouteId ?? ""} />}
      <main className="min-h-0 flex-1 overflow-hidden">
        <Routes>
          <Route path="/" element={<Navigate to="/repos" replace />} />
          <Route path="/repos" element={<ReposEntry />} />
          <Route path="/repos/:repoId" element={<RepoDetailPage />} />
          <Route path="/deploy" element={<DeployPage />} />
          <Route path="/servers" element={<ServersPage />} />
          <Route path="/backups" element={<BackupsPage />} />
          <Route path="/pages" element={<PagesPage />} />
          <Route path="/history" element={<HistoryPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="*" element={<Navigate to="/repos" replace />} />
        </Routes>
      </main>
    </div>
  );
}

export default function App() {
  const loadAll = useApp((state) => state.loadAll);
  const handleDeployEvent = useApp((state) => state.handleDeployEvent);
  const handleBackupEvent = useApp((state) => state.handleBackupEvent);
  const handlePagesEvent = useApp((state) => state.handlePagesEvent);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen<DeployEvent>("deploy://event", (event) => {
      handleDeployEvent(event.payload);
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleDeployEvent]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen<BackupEvent>("backup://event", (event) => {
      handleBackupEvent(event.payload);
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleBackupEvent]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listen<PagesEvent>("pages://event", (event) => {
      handlePagesEvent(event.payload);
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handlePagesEvent]);

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
