import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Eraser, History, Layers, RefreshCw, RotateCcw, ScrollText, Trash2 } from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";

import { RecordLog } from "../components/RecordRows";
import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Modal,
  Page,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { DeployRecord, RemoteRelease } from "../lib/types";
import {
  cn,
  deployStatusClass,
  deployStatusLabel,
  formatDuration,
  shortPath,
} from "../lib/utils";

export default function HistoryPage() {
  const { t } = useTranslation();
  const history = useApp((state) => state.history);
  const repos = useApp((state) => state.repos);
  const refreshHistory = useApp((state) => state.refreshHistory);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const redeploy = useApp((state) => state.redeploy);
  const toast = useApp((state) => state.toast);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  const [repoFilter, setRepoFilter] = useState("");
  const [viewing, setViewing] = useState<DeployRecord | null>(null);
  const [removing, setRemoving] = useState<DeployRecord | null>(null);
  const [rollingBack, setRollingBack] = useState<DeployRecord | null>(null);
  const [clearing, setClearing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [releaseRecord, setReleaseRecord] = useState<DeployRecord | null>(null);
  const [releases, setReleases] = useState<RemoteRelease[] | null>(null);
  const [releaseError, setReleaseError] = useState<string | null>(null);
  const [selectedRelease, setSelectedRelease] = useState("");
  const [switchingRelease, setSwitchingRelease] = useState(false);
  // 请求序号：快速切换记录 / 关闭弹窗后，旧请求的响应必须丢弃，避免把版本切错服务器。
  const releaseSeq = useRef(0);
  // 切换请求序号：关闭弹窗会让在途切换的 finally 失效，避免误清新一轮切换的 loading。
  const switchSeq = useRef(0);

  const closeReleases = () => {
    releaseSeq.current += 1;
    switchSeq.current += 1;
    setSwitchingRelease(false);
    setReleaseRecord(null);
  };

  const live = useApp((state) => state.live);
  const livePages = useApp((state) => state.livePages);
  // 任意一种部署进行中都不允许重新部署 / 回滚，避免争用工作区或 reset 掉正在构建的内容。
  const running = live?.status === "running" || livePages?.status === "running";

  // 过滤放在前端做，避免筛选结果与后台刷新（部署完成）互相覆盖。
  const filtered = useMemo(
    () => (repoFilter ? history.filter((record) => record.repoId === repoFilter) : history),
    [history, repoFilter],
  );

  useEffect(() => {
    void refreshHistory();
  }, [refreshHistory]);

  useEffect(() => {
    const recordId = searchParams.get("record");
    if (!recordId) return;
    let cancelled = false;
    void api
      .getRecord(recordId)
      .then((record) => {
        if (!cancelled) setViewing(record);
      })
      .catch((error) => {
        if (!cancelled) toast("error", String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [searchParams, toast]);

  async function handleRedeploy(record: DeployRecord) {
    try {
      await redeploy(record.id);
      navigate("/deploy");
    } catch {
      // store 已提示
    }
  }

  async function handleRollback() {
    if (!rollingBack || running) return;
    setBusy(true);
    try {
      const message = await api.resetHard(rollingBack.repoId, rollingBack.commit);
      toast("success", message.trim() || t("history.rollbackDone"));
      setRollingBack(null);
      await refreshRepos();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteRecord(removing.id);
      toast("success", t("history.deleted"));
      setRemoving(null);
      await refreshHistory();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  /** 打开原子发布的版本列表（需要连接服务器读取 releases/）。 */
  async function openReleases(record: DeployRecord) {
    const seq = ++releaseSeq.current;
    setReleaseRecord(record);
    setReleases(null);
    setReleaseError(null);
    setSelectedRelease("");
    try {
      const list = await api.listReleases(record.serverId, record.targetDir);
      if (releaseSeq.current !== seq) return;
      setReleases(list);
      setSelectedRelease(list.find((item) => item.current)?.name ?? "");
    } catch (error) {
      if (releaseSeq.current !== seq) return;
      setReleaseError(String(error));
    }
  }

  async function handleSwitchRelease() {
    if (!releaseRecord || !selectedRelease) return;
    const seq = releaseSeq.current;
    const mySwitch = ++switchSeq.current;
    setSwitchingRelease(true);
    try {
      const message = await api.rollbackRelease(
        releaseRecord.serverId,
        releaseRecord.targetDir,
        selectedRelease,
      );
      if (releaseSeq.current !== seq) return;
      toast("success", message.trim() || t("history.releaseSwitched"));
      const list = await api.listReleases(releaseRecord.serverId, releaseRecord.targetDir);
      if (releaseSeq.current !== seq) return;
      setReleases(list);
      setSelectedRelease(list.find((item) => item.current)?.name ?? "");
      await refreshHistory();
    } catch (error) {
      if (releaseSeq.current === seq) toast("error", String(error));
    } finally {
      if (switchSeq.current === mySwitch) setSwitchingRelease(false);
    }
  }

  const selectedIsCurrent = useMemo(
    () => releases?.find((item) => item.name === selectedRelease)?.current ?? false,
    [releases, selectedRelease],
  );

  async function handleClear() {
    setBusy(true);
    try {
      await api.clearHistory();
      toast("success", t("history.cleared"));
      setClearing(false);
      await refreshHistory();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Page
      title={t("history.title")}
      subtitle={t("history.subtitle")}
      actions={
        <>
          <div className="w-48">
            <Select
              value={repoFilter}
              onChange={(event) => setRepoFilter(event.target.value)}
            >
              <option value="">{t("history.allRepos")}</option>
              {repos.map((repo) => (
                <option key={repo.id} value={repo.id}>
                  {repo.name}
                </option>
              ))}
            </Select>
          </div>
          <Button
            variant="secondary"
            icon={<RefreshCw className="size-4" />}
            onClick={() => void refreshHistory()}
          >
            {t("common.refresh")}
          </Button>
          <Button
            variant="secondary"
            icon={<Eraser className="size-4" />}
            disabled={history.length === 0}
            onClick={() => setClearing(true)}
          >
            {t("common.clear")}
          </Button>
        </>
      }
    >
      {filtered.length === 0 ? (
        <EmptyState
          icon={<History className="size-5" />}
          title={t("history.emptyTitle")}
          description={t("history.emptyDescription")}
          action={
            <Button onClick={() => navigate("/deploy")} icon={<History className="size-4" />}>
              {t("history.goDeploy")}
            </Button>
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <table className="w-full text-left text-xs">
            <thead>
              <tr className="border-b border-line bg-field/60 text-[11px] uppercase tracking-wide text-ink-faint">
                <th className="px-4 py-2.5 font-medium">{t("history.columns.status")}</th>
                <th className="px-4 py-2.5 font-medium">{t("history.columns.time")}</th>
                <th className="px-4 py-2.5 font-medium">{t("history.columns.repo")}</th>
                <th className="px-4 py-2.5 font-medium">{t("history.columns.revision")}</th>
                <th className="px-4 py-2.5 font-medium">{t("history.columns.server")}</th>
                <th className="px-4 py-2.5 font-medium">{t("history.columns.duration")}</th>
                <th className="px-4 py-2.5 text-right font-medium">
                  {t("history.columns.actions")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {filtered.map((record) => (
                <tr key={record.id} className="group transition-colors hover:bg-field">
                  <td className="px-4 py-2.5">
                    <Badge className={cn("border", deployStatusClass(record.status))}>
                      {deployStatusLabel(record.status)}
                    </Badge>
                  </td>
                  <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                    {record.startedAt}
                  </td>
                  <td className="px-4 py-2.5 text-ink">{record.repoName}</td>
                  <td className="px-4 py-2.5">
                    <div className="text-ink">{record.branch}</div>
                    <div className="mt-0.5 font-mono text-[10px] text-ink-faint">
                      {record.worktree
                        ? `${t("deploy.worktree")}${record.commitShort ? ` · ${record.commitShort}` : ""}`
                        : `${record.commitShort} · ${shortPath(record.commitSubject, 28)}`}
                    </div>
                  </td>
                  <td className="px-4 py-2.5">
                    <div className="text-ink">{record.serverName}</div>
                    <div className="mt-0.5 max-w-56 truncate font-mono text-[10px] text-ink-faint" title={record.targetDir}>
                      {record.targetDir}
                    </div>
                  </td>
                  <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                    {record.status === "running" ? "-" : formatDuration(record.durationMs)}
                  </td>
                  <td className="px-4 py-2.5">
                    <div className="flex items-center justify-end gap-1">
                      {record.error && (
                        <span
                          className="mr-1 max-w-40 truncate text-[10px] text-neg"
                          title={record.error}
                        >
                          {record.error}
                        </span>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<ScrollText className="size-3.5" />}
                        onClick={() => setViewing(record)}
                      >
                        {t("history.viewLog")}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<RefreshCw className="size-3.5" />}
                        disabled={record.status === "running" || running}
                        onClick={() => void handleRedeploy(record)}
                      >
                        {t("history.redeploy")}
                      </Button>
                      {record.status === "success" && !record.worktree && (
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={<RotateCcw className="size-3.5" />}
                          title={t("history.rollbackHint")}
                          disabled={running}
                          onClick={() => setRollingBack(record)}
                        >
                          {t("history.rollback")}
                        </Button>
                      )}
                      {record.atomicRelease && record.status !== "running" && (
                        <Button
                          size="sm"
                          variant="ghost"
                          title={t("history.releases")}
                          disabled={running}
                          icon={<Layers className="size-3.5" />}
                          onClick={() => void openReleases(record)}
                        />
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        className="text-neg hover:bg-neg-soft hover:text-neg"
                        icon={<Trash2 className="size-3.5" />}
                        onClick={() => setRemoving(record)}
                      />
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}

      <Modal
        open={!!viewing}
        onClose={() => setViewing(null)}
        title={t("history.logTitle")}
        subtitle={
          viewing
            ? `${viewing.repoName} · ${viewing.branch} · ${
                viewing.worktree ? t("deploy.worktree") : viewing.commitShort
              }`
            : undefined
        }
        width="max-w-4xl"
        footer={
          <>
            <Button variant="secondary" onClick={() => setViewing(null)}>
              {t("common.close")}
            </Button>
            {viewing && (
              <Button
                icon={<RefreshCw className="size-4" />}
                disabled={running || viewing.status === "running"}
                onClick={() => {
                  const record = viewing;
                  setViewing(null);
                  void handleRedeploy(record);
                }}
              >
                {t("history.redeployVersion")}
              </Button>
            )}
          </>
        }
      >
        {viewing && (
          <div className="flex flex-col gap-4">
            <div className="grid grid-cols-2 gap-3 text-xs md:grid-cols-4">
              <InfoCell label={t("history.info.status")}>
                <Badge className={cn("border", deployStatusClass(viewing.status))}>
                  {deployStatusLabel(viewing.status)}
                </Badge>
              </InfoCell>
              <InfoCell label={t("history.info.server")}>{viewing.serverName}</InfoCell>
              <InfoCell label={t("history.info.dir")}>{viewing.targetDir}</InfoCell>
              <InfoCell label={t("history.info.duration")}>
                {viewing.status === "running" ? "-" : formatDuration(viewing.durationMs)}
              </InfoCell>
              {viewing.releaseDir && (
                <InfoCell label={t("history.info.release")}>{viewing.releaseDir}</InfoCell>
              )}
            </div>
            {viewing.error && (
              <div className="rounded-md border border-neg/30 bg-neg-soft px-4 py-2.5 text-xs text-neg">
                {viewing.error}
              </div>
            )}
            <RecordLog
              log={viewing.log}
              emptyText={t("history.noLog")}
              className="max-h-[46vh] overflow-y-auto rounded-md border border-line bg-sunken px-4 py-2.5 font-mono text-[11.5px] leading-[1.7] text-ink-dim"
            />
          </div>
        )}
      </Modal>

      <Modal
        open={!!releaseRecord}
        onClose={closeReleases}
        title={t("history.releasesTitle")}
        subtitle={
          releaseRecord
            ? `${releaseRecord.repoName} → ${releaseRecord.targetDir}/releases`
            : undefined
        }
        width="max-w-xl"
        footer={
          <>
            <Button variant="secondary" onClick={closeReleases}>
              {t("common.close")}
            </Button>
            <Button
              icon={<RotateCcw className="size-4" />}
              loading={switchingRelease}
              disabled={!selectedRelease || selectedIsCurrent}
              onClick={() => void handleSwitchRelease()}
            >
              {t("history.releaseSwitch")}
            </Button>
          </>
        }
      >
        {releaseError ? (
          <div className="flex items-center justify-between gap-3 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-[11px] text-warn">
            <span className="min-w-0 break-all">{releaseError}</span>
            <Button
              size="sm"
              variant="secondary"
              disabled={!releaseRecord}
              onClick={() => releaseRecord && void openReleases(releaseRecord)}
            >
              {t("common.retry")}
            </Button>
          </div>
        ) : releases === null ? (
          <p className="text-xs text-ink-faint">{t("common.loading")}</p>
        ) : releases.length === 0 ? (
          <p className="text-xs text-ink-faint">{t("history.releasesEmpty")}</p>
        ) : (
          <div className="flex max-h-80 flex-col gap-1 overflow-y-auto">
            <p className="mb-1 text-[11px] leading-relaxed text-ink-faint">
              {t("history.releaseHint")}
            </p>
            {releases.map((item) => (
              <button
                key={item.name}
                type="button"
                disabled={switchingRelease}
                onClick={() => setSelectedRelease(item.name)}
                className={cn(
                  "flex items-center gap-3 rounded-md border px-3 py-2 text-left text-xs transition-colors disabled:opacity-60",
                  item.name === selectedRelease
                    ? "border-brand bg-brand-soft text-ink"
                    : "border-line bg-field text-ink-dim hover:bg-hover",
                )}
              >
                <span className="min-w-0 flex-1 truncate font-mono">{item.name}</span>
                {item.current && <Badge kind="green">{t("history.releaseCurrent")}</Badge>}
                <span className="shrink-0 text-[10px] text-ink-faint">{item.modified}</span>
              </button>
            ))}
          </div>
        )}
      </Modal>

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title={t("history.deleteTitle")}
        confirmText={t("common.delete")}
        description={
          <Trans
            i18nKey="history.deleteDescription"
            values={{ repo: removing?.repoName, date: removing?.startedAt }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleDelete()}
      />

      <ConfirmModal
        open={!!rollingBack}
        danger
        loading={busy}
        title={t("history.rollbackTitle")}
        confirmText={t("history.rollback")}
        description={
          <Trans
            i18nKey="history.rollbackDescription"
            values={{ repo: rollingBack?.repoName, commit: rollingBack?.commitShort, date: rollingBack?.startedAt }}
            components={{ b: <b className="text-ink" />, code: <code className="text-neg" /> }}
          />
        }
        onCancel={() => setRollingBack(null)}
        onConfirm={() => void handleRollback()}
      />

      <ConfirmModal
        open={clearing}
        danger
        loading={busy}
        title={t("history.clearTitle")}
        confirmText={t("history.clearConfirmText")}
        description={t("history.clearDescription")}
        onCancel={() => setClearing(false)}
        onConfirm={() => void handleClear()}
      />
    </Page>
  );
}

function InfoCell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="rounded-md border border-line bg-field px-3 py-2">
      <p className="text-[10px] uppercase tracking-wide text-ink-faint">{label}</p>
      <div className="mt-1 truncate text-ink">{children}</div>
    </div>
  );
}
