import { useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Cloud,
  History,
  Pencil,
  Plus,
  Rocket,
  Square,
  Terminal,
  Timer,
  Trash2,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";

import { BindRemoteModal } from "../components/BindRemoteModal";
import { LogConsole } from "../components/LogConsole";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  ConfirmModal,
  EmptyState,
  Modal,
  Page,
  SectionTitle,
} from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type {
  DeployConfig,
  DeployRecord,
  PagesConfigEntry,
  PagesDeployRecord,
  RepoInfo,
} from "../lib/types";
import { cn, deployStatusClass, deployStatusLabel, formatDuration } from "../lib/utils";
import { DeployConfigModal, type DeployPrefill } from "./deploy/DeployConfigModal";
import { PagesConfigModal } from "./deploy/PagesConfigModal";

type EditingState =
  | { kind: "server"; config: DeployConfig | null; prefill?: DeployPrefill }
  | { kind: "pages"; entry: PagesConfigEntry | null; prefillRepoId?: string };

type DeletingState =
  | { kind: "server"; id: string; name: string }
  | { kind: "pages"; repoId: string; name: string };

/** 统一列表项：服务器部署配置与 Pages 配置（按仓库一份）合并展示。 */
type ConfigRow =
  | { kind: "server"; key: string; name: string; repoName: string; config: DeployConfig }
  | { kind: "pages"; key: string; name: string; repoName: string; entry: PagesConfigEntry };

/** Pages 配置的一行摘要（列表与部署确认弹窗共用）。 */
function pagesSummary(entry: PagesConfigEntry, fallback: string): string {
  const config = entry.config;
  return config.provider === "github"
    ? `GitHub Pages · ${config.publishBranch || "gh-pages"}`
    : `Cloudflare Pages · ${config.projectName || fallback}`;
}

export default function DeployPage() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const toast = useApp((state) => state.toast);
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const history = useApp((state) => state.history);
  const live = useApp((state) => state.live);
  const livePages = useApp((state) => state.livePages);
  const deployConfigs = useApp((state) => state.deployConfigs);
  const pagesConfigs = useApp((state) => state.pagesConfigs);
  const pagesRecords = useApp((state) => state.pagesRecords);
  const startDeployConfig = useApp((state) => state.startDeployConfig);
  const redeploy = useApp((state) => state.redeploy);
  const cancelDeploy = useApp((state) => state.cancelDeploy);
  const startPagesDeploy = useApp((state) => state.startPagesDeploy);
  const refreshDeployConfigs = useApp((state) => state.refreshDeployConfigs);
  const refreshPagesConfigs = useApp((state) => state.refreshPagesConfigs);
  const refreshPagesRecords = useApp((state) => state.refreshPagesRecords);
  const refreshHistory = useApp((state) => state.refreshHistory);
  const refreshRepos = useApp((state) => state.refreshRepos);

  const [editing, setEditing] = useState<EditingState | null>(null);
  const [deleting, setDeleting] = useState<DeletingState | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [pagesRun, setPagesRun] = useState<PagesConfigEntry | null>(null);
  const [skipBuild, setSkipBuild] = useState(false);
  const [recordsTab, setRecordsTab] = useState<"server" | "pages">("server");
  const [expandedRecord, setExpandedRecord] = useState<string | null>(null);
  const [confirmClearPages, setConfirmClearPages] = useState(false);
  const [bindRepo, setBindRepo] = useState<RepoInfo | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [viewKind, setViewKind] = useState<"server" | "pages">("server");

  // 配置与记录可能被 CLI 或其它页面（删除服务器 / 仓库）改动，进入页面时拉取一次最新数据。
  useEffect(() => {
    void refreshDeployConfigs();
    void refreshPagesConfigs();
    void refreshHistory();
    void refreshPagesRecords();
  }, [refreshDeployConfigs, refreshPagesConfigs, refreshHistory, refreshPagesRecords]);

  const serverRunning = live?.status === "running";
  const pagesRunning = livePages?.status === "running";
  // 任意一种部署进行中时都锁定操作，避免两种任务并发争用同一工作区。
  const running = serverRunning || pagesRunning;

  // 从其他页面（如仓库页）带参数进入时直接打开新增弹窗，并把参数从地址栏清掉。
  // 清空后必须把记录重置，否则下一次同样的深链（如再次点「部署」）会因 key 相同而被忽略。
  const appliedQuery = useRef("");
  useEffect(() => {
    const key = `${searchParams.get("repo") ?? ""}|${searchParams.get("rev") ?? ""}|${searchParams.get("target") ?? ""}`;
    if (key === "||") {
      appliedQuery.current = "";
      return;
    }
    if (key === appliedQuery.current) return;
    appliedQuery.current = key;
    const repoId = searchParams.get("repo") ?? undefined;
    const rev = searchParams.get("rev") ?? undefined;
    if (searchParams.get("target") === "pages") {
      setEditing({ kind: "pages", entry: null, prefillRepoId: repoId });
    } else {
      setEditing({ kind: "server", config: null, prefill: { repoId, rev } });
    }
    navigate("/deploy", { replace: true });
  }, [searchParams, navigate]);

  // 任务运行时自动切到对应的视图，保证日志与状态卡一致。
  useEffect(() => {
    if (pagesRunning && !serverRunning) setViewKind("pages");
    if (serverRunning && !pagesRunning) setViewKind("server");
  }, [pagesRunning, serverRunning]);

  const rows = useMemo<ConfigRow[]>(() => {
    const repoName = (repoId: string) =>
      repos.find((repo) => repo.id === repoId)?.name ?? t("deploy.unknownRepo");
    const serverRows: ConfigRow[] = deployConfigs
      .map((config) => ({
        kind: "server" as const,
        key: config.id,
        name: config.name,
        repoName: repoName(config.repoId),
        config,
      }))
      .sort((a, b) => a.name.localeCompare(b.name));
    const pagesRows: ConfigRow[] = pagesConfigs
      .map((entry) => ({
        kind: "pages" as const,
        key: entry.repoId,
        name: entry.repoName,
        repoName: repoName(entry.repoId),
        entry,
      }))
      .sort((a, b) => a.name.localeCompare(b.name));
    return [...serverRows, ...pagesRows];
  }, [deployConfigs, pagesConfigs, repos, t]);

  const activeKind = viewKind;
  const activeLive = activeKind === "pages" ? livePages : live;
  const activeRunning = activeKind === "pages" ? pagesRunning : serverRunning;
  const recentDeploys = history.slice(0, 8);
  const recentPagesDeploys = pagesRecords.slice(0, 8);

  async function handleDeployServer(config: DeployConfig) {
    if (running) return;
    setViewKind("server");
    setRecordsTab("server");
    try {
      await startDeployConfig(config.id);
    } catch {
      // store 已提示错误
    }
  }

  function openPagesRun(entry: PagesConfigEntry) {
    if (running) return;
    setSkipBuild(false);
    setPagesRun(entry);
  }

  async function handleDeployPages() {
    if (!pagesRun || running) return;
    const entry = pagesRun;
    setViewKind("pages");
    setRecordsTab("pages");
    setPagesRun(null);
    // 不在这里清空 livePages：startPagesDeploy 会写入新的 running 状态并替换旧记录。
    try {
      await startPagesDeploy({ repoId: entry.repoId, skipBuild });
    } catch {
      // store 已提示错误
    }
  }

  async function handleDelete() {
    if (!deleting || deleteBusy) return;
    setDeleteBusy(true);
    try {
      const removed =
        deleting.kind === "server"
          ? await api.deleteDeployConfig(deleting.id)
          : await api.deletePagesConfig(deleting.repoId);
      if (deleting.kind === "server") {
        await refreshDeployConfigs();
      } else {
        await refreshPagesConfigs();
      }
      if (removed) {
        toast("success", t("deploy.configDeleted", { name: deleting.name }));
      } else {
        // 配置可能已被 CLI / 其它页面删除：只刷新列表，不误报成功。
        toast("info", t("deploy.configMissing"));
      }
    } catch (error) {
      toast("error", String(error));
    } finally {
      setDeleteBusy(false);
      setDeleting(null);
    }
  }

  async function handleDeletePagesRecord(recordId: string) {
    try {
      await api.deletePagesRecord(recordId);
      await refreshPagesRecords();
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleClearPagesRecords() {
    try {
      await api.clearPagesRecords();
      await refreshPagesRecords();
      toast("success", t("pages.cleared"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClearPages(false);
    }
  }

  async function copyUrl(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      toast("success", t("pages.urlCopied"));
    } catch {
      toast("info", url);
    }
  }

  function configSummary(row: ConfigRow): string {
    if (row.kind === "server") {
      const server = servers.find((item) => item.id === row.config.serverId);
      const rev = row.config.rev.trim() || t("deploy.worktree");
      return `${server?.name ?? t("deploy.unknownServer")} → ${
        row.config.targetDir || t("deploy.targetDirEmpty")
      } · ${rev}`;
    }
    return pagesSummary(row.entry, t("pages.projectNameEmpty"));
  }

  return (
    <Page
      title={t("nav.deploy")}
      subtitle={t("deploy.subtitle")}
      actions={
        <Button
          variant="secondary"
          icon={<History className="size-4" />}
          onClick={() => navigate("/history")}
        >
          {t("history.title")}
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-5 xl:grid-cols-[minmax(0,1fr)_400px]">
        <div className="flex min-w-0 flex-col gap-5">
          <section>
            <SectionTitle
              title={t("deploy.configSection")}
              description={t("deploy.configSectionDescription")}
              actions={
                <div className="flex items-center gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    icon={<Cloud className="size-3.5" />}
                    onClick={() => setEditing({ kind: "pages", entry: null })}
                  >
                    {t("pages.newConfig")}
                  </Button>
                  <Button
                    size="sm"
                    icon={<Plus className="size-3.5" />}
                    onClick={() => setEditing({ kind: "server", config: null })}
                  >
                    {t("deploy.newConfig")}
                  </Button>
                </div>
              }
            />
            {rows.length === 0 ? (
              <EmptyState
                icon={<Rocket className="size-4.5" />}
                title={t("deploy.noConfigs")}
                description={t("deploy.noConfigsDescription")}
                action={
                  <div className="flex items-center gap-2">
                    <Button
                      variant="secondary"
                      icon={<Cloud className="size-4" />}
                      onClick={() => setEditing({ kind: "pages", entry: null })}
                    >
                      {t("pages.newConfig")}
                    </Button>
                    <Button
                      icon={<Plus className="size-4" />}
                      onClick={() => setEditing({ kind: "server", config: null })}
                    >
                      {t("deploy.newConfig")}
                    </Button>
                  </div>
                }
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {rows.map((row) => (
                  <div key={`${row.kind}-${row.key}`} className="flex items-center gap-3 px-4 py-3">
                    <span
                      className={cn(
                        "grid size-8 shrink-0 place-items-center rounded-md border",
                        row.kind === "server"
                          ? "border-brand-line bg-brand-soft text-brand"
                          : "border-line bg-field text-ink-dim",
                      )}
                    >
                      {row.kind === "server" ? (
                        <Rocket className="size-4" />
                      ) : (
                        <Cloud className="size-4" />
                      )}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="flex items-center gap-2 truncate text-[13px] font-medium text-ink">
                        {row.name}
                        <span className="shrink-0 rounded bg-hover px-1.5 py-0.5 text-[10px] font-normal text-ink-dim">
                          {row.kind === "server"
                            ? t("deploy.kindServer")
                            : t("deploy.kindPages")}
                        </span>
                      </p>
                      <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                        {row.repoName} · {configSummary(row)}
                      </p>
                    </div>
                    <Button
                      size="sm"
                      disabled={running}
                      icon={<Rocket className="size-3.5" />}
                      onClick={() =>
                        row.kind === "server"
                          ? void handleDeployServer(row.config)
                          : openPagesRun(row.entry)
                      }
                    >
                      {t("deploy.deployNow")}
                    </Button>
                    <Button
                      variant="secondary"
                      size="sm"
                      title={t("deploy.editConfig")}
                      disabled={running}
                      onClick={() =>
                        setEditing(
                          row.kind === "server"
                            ? { kind: "server", config: row.config }
                            : { kind: "pages", entry: row.entry },
                        )
                      }
                    >
                      <Pencil className="size-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      title={t("deploy.deleteConfig")}
                      disabled={running}
                      onClick={() =>
                        setDeleting(
                          row.kind === "server"
                            ? { kind: "server", id: row.config.id, name: row.config.name }
                            : { kind: "pages", repoId: row.entry.repoId, name: row.name },
                        )
                      }
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                ))}
              </Card>
            )}
          </section>

          <section>
            <SectionTitle
              title={t("deploy.records")}
              description={
                recordsTab === "server"
                  ? t("deploy.recordsServerHint")
                  : t("deploy.recordsPagesHint")
              }
              actions={
                <div className="flex items-center gap-1">
                  <div className="flex rounded-md border border-line p-0.5">
                    {(["server", "pages"] as const).map((tab) => (
                      <button
                        key={tab}
                        type="button"
                        onClick={() => setRecordsTab(tab)}
                        className={cn(
                          "rounded px-2.5 py-1 text-[11px] font-medium transition-colors",
                          recordsTab === tab
                            ? "bg-brand-soft text-brand"
                            : "text-ink-dim hover:bg-hover hover:text-ink",
                        )}
                      >
                        {tab === "server" ? t("deploy.recordsServer") : t("deploy.recordsPages")}
                      </button>
                    ))}
                  </div>
                  {recordsTab === "pages" && pagesRecords.length > 0 && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setConfirmClearPages(true)}
                    >
                      {t("backup.clearRecords")}
                    </Button>
                  )}
                </div>
              }
            />
            {recordsTab === "server" ? (
              recentDeploys.length === 0 ? (
                <EmptyState
                  icon={<History className="size-4.5" />}
                  title={t("deploy.noRecords")}
                  description={t("deploy.noRecordsDescription")}
                />
              ) : (
                <Card className="divide-y divide-line overflow-hidden">
                  {recentDeploys.map((record) => (
                    <RecentDeployRow
                      key={record.id}
                      record={record}
                      disabled={running}
                      onRedeploy={() => {
                        setViewKind("server");
                        void redeploy(record.id).catch(() => {
                          // store 已提示错误
                        });
                      }}
                    />
                  ))}
                </Card>
              )
            ) : recentPagesDeploys.length === 0 ? (
              <EmptyState
                icon={<Cloud className="size-4.5" />}
                title={t("deploy.noPagesRecords")}
                description={t("deploy.noPagesRecordsDescription")}
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {recentPagesDeploys.map((record) => (
                  <PagesRecordRow
                    key={record.id}
                    record={record}
                    expanded={expandedRecord === record.id}
                    onToggle={() =>
                      setExpandedRecord(expandedRecord === record.id ? null : record.id)
                    }
                    onCopy={() => void copyUrl(record.url ?? "")}
                    onDelete={() => void handleDeletePagesRecord(record.id)}
                  />
                ))}
              </Card>
            )}
          </section>
        </div>

        <div className="flex min-h-0 flex-col gap-5">
          <Card className="p-5">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <h2 className="flex items-center gap-2.5 text-sm font-semibold tracking-tight text-ink">
                  <span className="grid size-7 place-items-center rounded-md border border-brand-line bg-brand-soft">
                    <Terminal className="size-3.5 text-brand" />
                  </span>
                  {t("deploy.status")}
                </h2>
                <p className="mt-1 text-xs text-ink-dim">
                  {activeLive
                    ? activeRunning
                      ? t("deploy.statusRunning")
                      : activeLive.status === "success"
                        ? t("deploy.statusSuccess")
                        : t("deploy.statusFailed")
                    : t("deploy.statusIdle")}
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {activeLive && (
                  <Badge className={cn("border", deployStatusClass(activeLive.status))}>
                    {activeRunning && (
                      <span className="size-1.5 animate-pulse rounded-full bg-current" />
                    )}
                    {deployStatusLabel(activeLive.status)}
                  </Badge>
                )}
                {activeRunning && activeKind === "server" && (
                  <Button
                    variant="secondary"
                    size="sm"
                    loading={cancelling}
                    disabled={!live?.recordId}
                    icon={<Square className="size-3" />}
                    onClick={() => {
                      setCancelling(true);
                      void cancelDeploy().finally(() => setCancelling(false));
                    }}
                  >
                    {t("deploy.cancel")}
                  </Button>
                )}
              </div>
            </div>

            {activeRunning && activeKind === "server" && (
              <div className="mt-4">
                <div className="h-1.5 overflow-hidden rounded-full bg-hover">
                  <div
                    className="h-full rounded-full bg-linear-to-r from-[var(--btn-primary-top)] to-[var(--btn-primary-bottom)] transition-all duration-300"
                    style={{ width: `${Math.max(live?.progress ?? 0, 3)}%` }}
                  />
                </div>
                <p className="mt-2 text-right text-[11px] text-ink-dim">{live?.progress ?? 0}%</p>
              </div>
            )}

            {activeLive?.record && (
              <div
                className={cn(
                  "mt-4 flex flex-wrap items-center gap-3 rounded-md border px-4 py-3 text-xs",
                  activeLive.record.status === "success"
                    ? "border-pos/30 bg-pos-soft text-pos"
                    : "border-neg/30 bg-neg-soft text-neg",
                )}
              >
                {activeLive.record.status === "success" ? (
                  <CheckCircle2 className="size-4 shrink-0" />
                ) : (
                  <XCircle className="size-4 shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate">
                  {activeKind === "server" && live?.record
                    ? `${live.record.repoName} · ${
                        live.record.worktree ? t("deploy.worktree") : live.record.commitShort
                      } · ${live.record.serverName} · ${formatDuration(live.record.durationMs)}`
                    : livePages?.record
                      ? `${livePages.record.repoName} · ${livePages.record.commitShort} · ${
                          livePages.record.projectName
                        } · ${formatDuration(livePages.record.durationMs)}`
                      : ""}
                </span>
                {activeKind === "server" && live?.record && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => navigate(`/history?record=${live.record?.id ?? ""}`)}
                  >
                    {t("deploy.viewRecord")}
                  </Button>
                )}
                {activeKind === "pages" && livePages?.record?.url && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => void copyUrl(livePages.record?.url ?? "")}
                  >
                    {t("pages.copyUrl")}
                  </Button>
                )}
              </div>
            )}
          </Card>

          <LogConsole
            lines={activeLive?.lines ?? []}
            className="h-[420px] xl:h-[520px]"
            emptyText={
              activeKind === "pages" ? t("pages.logEmpty") : t("deploy.logEmpty")
            }
          />
        </div>
      </div>

      {editing?.kind === "server" && (
        <DeployConfigModal
          initial={editing.config}
          prefill={editing.prefill ?? null}
          onClose={() => setEditing(null)}
          onRefresh={() => void refreshDeployConfigs()}
          onSaved={(config) => {
            setEditing(null);
            void refreshDeployConfigs();
            toast("success", t("deploy.configSaved", { name: config.name }));
          }}
        />
      )}

      {editing?.kind === "pages" && (
        <PagesConfigModal
          entry={editing.entry}
          prefillRepoId={editing.prefillRepoId}
          onClose={() => setEditing(null)}
          onSaved={(repoId) => {
            setEditing(null);
            void refreshPagesConfigs();
            const name = repos.find((repo) => repo.id === repoId)?.name ?? "";
            toast("success", t("pages.configSaved", { name }));
          }}
          onRequestBindRemote={(repo) => setBindRepo(repo)}
        />
      )}

      <Modal
        open={pagesRun !== null}
        onClose={() => setPagesRun(null)}
        title={t("deploy.pagesDeployTitle")}
        subtitle={pagesRun ? `${pagesRun.repoName} · ${pagesSummary(pagesRun, t("pages.projectNameEmpty"))}` : undefined}        width="max-w-md"
        footer={
          <>
            <Button variant="secondary" onClick={() => setPagesRun(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              icon={<Rocket className="size-4" />}
              disabled={running}
              onClick={() => void handleDeployPages()}
            >
              {t("deploy.startDeploy")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <Checkbox checked={skipBuild} onChange={setSkipBuild}>
            {t("pages.skipBuild")}
          </Checkbox>
          <p className="text-[11px] leading-relaxed text-ink-faint">
            {pagesRun?.config.provider === "github"
              ? t("pages.deployDescriptionGithub")
              : t("pages.deployDescriptionCloudflare")}
          </p>
        </div>
      </Modal>

      <ConfirmModal
        open={deleting !== null}
        danger
        loading={deleteBusy}
        title={t("deploy.configDeleteTitle")}
        confirmText={t("common.delete")}
        description={
          deleting?.kind === "pages"
            ? t("pages.deleteDescription", { name: deleting.name })
            : t("deploy.configDeleteDescription", { name: deleting?.name ?? "" })
        }
        onCancel={() => setDeleting(null)}
        onConfirm={() => void handleDelete()}
      />

      <ConfirmModal
        open={confirmClearPages}
        danger
        title={t("pages.clearTitle")}
        confirmText={t("common.clear")}
        description={t("pages.clearDescription")}
        onCancel={() => setConfirmClearPages(false)}
        onConfirm={() => void handleClearPagesRecords()}
      />

      <BindRemoteModal
        repo={bindRepo}
        onClose={() => setBindRepo(null)}
        onSaved={() => void refreshRepos()}
      />
    </Page>
  );
}

function RecentDeployRow({
  record,
  disabled,
  onRedeploy,
}: {
  record: DeployRecord;
  disabled: boolean;
  onRedeploy: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3 px-4 py-2.5">
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          record.status === "success"
            ? "bg-pos"
            : record.status === "failed"
              ? "bg-neg"
              : "bg-warn",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[11px] text-ink">
          {record.repoName} · {record.branch} ·{" "}
          {record.worktree ? t("deploy.worktree") : record.commitShort}
        </p>
        <p className="mt-0.5 flex items-center gap-1 text-[10px] text-ink-faint">
          <Timer className="size-2.5" />
          {record.startedAt} · {record.serverName}
        </p>
      </div>
      <Button
        size="sm"
        variant="ghost"
        disabled={disabled || record.status === "running"}
        onClick={onRedeploy}
      >
        {t("history.redeploy")}
      </Button>
    </div>
  );
}

function PagesRecordRow({
  record,
  expanded,
  onToggle,
  onCopy,
  onDelete,
}: {
  record: PagesDeployRecord;
  expanded: boolean;
  onToggle: () => void;
  onCopy: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div>
      <div className="flex items-center gap-3 px-4 py-3">
        <button
          className="flex min-w-0 flex-1 items-center gap-3 text-left"
          onClick={onToggle}
        >
          {expanded ? (
            <ChevronDown className="size-3.5 shrink-0 text-ink-faint" />
          ) : (
            <ChevronRight className="size-3.5 shrink-0 text-ink-faint" />
          )}
          <div className="min-w-0 flex-1">
            <p className="truncate text-[13px] font-medium text-ink">
              {record.repoName} → {record.projectName}
              <span className="ml-2 rounded bg-hover px-1.5 py-0.5 text-[10px] font-normal text-ink-dim">
                {record.provider === "github" ? "GitHub" : "Cloudflare"}
              </span>
              <span className="ml-2 text-[11px] font-normal text-ink-faint">
                {record.branch}
                {record.commitShort ? ` · ${record.commitShort}` : ""}
              </span>
            </p>
            <p className="mt-0.5 truncate text-[11px] text-ink-faint">
              {record.startedAt}
              {record.url ? ` · ${record.url}` : ""}
            </p>
          </div>
          <Badge
            kind={
              record.status === "success" ? "green" : record.status === "failed" ? "red" : "amber"
            }
          >
            {deployStatusLabel(record.status)}
          </Badge>
        </button>
        {record.url && (
          <Button variant="ghost" size="sm" onClick={onCopy}>
            {t("pages.copyUrl")}
          </Button>
        )}
        <Button variant="ghost" size="sm" title={t("backup.deleteRecord")} onClick={onDelete}>
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {expanded && (
        <pre className="max-h-80 overflow-auto border-t border-line bg-sunken px-4 py-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-ink-dim">
          {record.error ? `${t("backup.errorPrefix", { error: record.error })}\n\n` : ""}
          {record.log || t("backup.noLog")}
        </pre>
      )}
    </div>
  );
}
