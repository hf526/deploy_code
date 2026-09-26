import { useCallback, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import {
  History,
  Pencil,
  Plus,
  Power,
  RefreshCw,
  Settings2,
  Trash2,
  Webhook,
} from "lucide-react";

import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Modal,
  Page,
} from "../components/ui";
import { api } from "../lib/api";
import { cronMethodLabel, cronSummary, formatUnixSeconds } from "../lib/cronJob";
import { useApp } from "../lib/store";
import type { CronJob, CronJobRun } from "../lib/types";
import { cn, formatDuration } from "../lib/utils";
import { CronJobModal } from "./cronJobs/CronJobModal";

interface HistoryState {
  job: CronJob;
  runs: CronJobRun[];
  loading: boolean;
}

export default function CronJobsPage() {
  const { t } = useTranslation();
  const settings = useApp((state) => state.settings);
  const toast = useApp((state) => state.toast);
  const navigate = useNavigate();
  const apiKey = settings.cronjobApiKey.trim();

  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<CronJob | null>(null);
  const [removing, setRemoving] = useState<CronJob | null>(null);
  const [history, setHistory] = useState<HistoryState | null>(null);
  const [busy, setBusy] = useState(false);

  // cron-job.org 的 API 默认只有 100 次/天：进入页面拉一次 + 手动刷新，绝不轮询。
  const requestSeq = useRef(0);
  // StrictMode 在开发期会把挂载跑两遍，配额按天计，所以首拉只放行一次。
  const bootstrapped = useRef(false);
  const reload = useCallback(async () => {
    const seq = requestSeq.current + 1;
    requestSeq.current = seq;
    setLoading(true);
    try {
      const next = await api.listCronJobs();
      if (seq !== requestSeq.current) return;
      setJobs(next);
      setError(null);
    } catch (requestError) {
      if (seq !== requestSeq.current) return;
      setError(String(requestError));
    } finally {
      if (seq === requestSeq.current) {
        setLoading(false);
        setLoaded(true);
      }
    }
  }, []);

  useEffect(() => {
    if (!apiKey || bootstrapped.current) return;
    bootstrapped.current = true;
    void reload();
  }, [apiKey, reload]);

  async function handleToggle(job: CronJob, enabled: boolean) {
    if (busy) return;
    setBusy(true);
    try {
      await api.setCronJobEnabled(job.jobId, enabled);
      // 本地改状态即可，开关不需要再花一次 API 配额。
      setJobs((current) =>
        current.map((item) => (item.jobId === job.jobId ? { ...item, enabled } : item)),
      );
    } catch (requestError) {
      toast("error", String(requestError));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    if (!removing || busy) return;
    setBusy(true);
    try {
      await api.deleteCronJob(removing.jobId);
      toast("success", t("cronJobs.deleted", { title: removing.title }));
      setRemoving(null);
      await reload();
    } catch (requestError) {
      toast("error", String(requestError));
    } finally {
      setBusy(false);
    }
  }

  async function openHistory(job: CronJob) {
    setHistory({ job, runs: [], loading: true });
    try {
      const runs = await api.cronJobHistory(job.jobId);
      setHistory((current) =>
        current?.job.jobId === job.jobId ? { job, runs, loading: false } : current,
      );
    } catch (requestError) {
      toast("error", String(requestError));
      setHistory((current) =>
        current?.job.jobId === job.jobId ? { ...current, loading: false } : current,
      );
    }
  }

  function openCreate() {
    setEditing(null);
    setModalOpen(true);
  }

  function openEdit(job: CronJob) {
    setEditing(job);
    setModalOpen(true);
  }

  const enabledCount = jobs.filter((job) => job.enabled).length;

  return (
    <Page
      title={t("nav.cronJobs")}
      subtitle={t("cronJobs.subtitle")}
      actions={
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            loading={loading}
            disabled={!apiKey}
            icon={<RefreshCw className={cn("size-4", loading && "animate-spin")} />}
            onClick={() => void reload()}
          >
            {t("common.refresh")}
          </Button>
          <Button disabled={!apiKey} icon={<Plus className="size-4" />} onClick={openCreate}>
            {t("cronJobs.create")}
          </Button>
        </div>
      }
    >
      {!apiKey ? (
        <EmptyState
          icon={<Webhook className="size-4" />}
          title={t("cronJobs.noKeyTitle")}
          description={t("cronJobs.noKeyDescription")}
          action={
            <Button icon={<Settings2 className="size-4" />} onClick={() => navigate("/settings")}>
              {t("cronJobs.goSettings")}
            </Button>
          }
        />
      ) : !loaded ? (
        <p className="py-10 text-center text-xs text-ink-faint">{t("common.loading")}</p>
      ) : error && jobs.length === 0 ? (
        <EmptyState
          icon={<Webhook className="size-4" />}
          title={t("cronJobs.loadFailedTitle")}
          description={error}
          action={
            <Button variant="secondary" loading={loading} onClick={() => void reload()}>
              {t("common.retry")}
            </Button>
          }
        />
      ) : jobs.length === 0 ? (
        <EmptyState
          icon={<Webhook className="size-4" />}
          title={t("cronJobs.emptyTitle")}
          description={t("cronJobs.emptyDescription")}
          action={
            <Button icon={<Plus className="size-4" />} onClick={openCreate}>
              {t("cronJobs.create")}
            </Button>
          }
        />
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <Badge kind="gray">{t("cronJobs.badgeCount", { count: jobs.length })}</Badge>
            <Badge kind="green">{t("cronJobs.badgeEnabled", { count: enabledCount })}</Badge>
            {error && <Badge kind="red">{t("cronJobs.badgeStale")}</Badge>}
          </div>
          <Card className="overflow-hidden">
            <table className="w-full text-left text-xs">
              <thead>
                <tr className="border-b border-line bg-field/60 text-[11px] uppercase tracking-wide text-ink-faint">
                  <th className="px-4 py-2.5 font-medium">{t("cronJobs.colTitle")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("cronJobs.colUrl")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("cronJobs.colSchedule")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("cronJobs.colNext")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("cronJobs.colLast")}</th>
                  <th className="px-4 py-2.5 text-right font-medium">{t("cronJobs.colActions")}</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-line">
                {jobs.map((job) => (
                  <tr key={job.jobId} className="group transition-colors hover:bg-field">
                    <td className="px-4 py-2.5">
                      <div className="flex items-center gap-2">
                        <span className="truncate font-medium text-ink">{job.title}</span>
                        {!job.enabled && (
                          <Badge kind="gray">{t("cronJobs.disabled")}</Badge>
                        )}
                      </div>
                    </td>
                    <td className="max-w-[18rem] px-4 py-2.5">
                      <div className="flex items-center gap-1.5">
                        <Badge kind="brand">{cronMethodLabel(job.requestMethod)}</Badge>
                        <span className="truncate font-mono text-[11px] text-ink-dim" title={job.url}>
                          {job.url}
                        </span>
                      </div>
                    </td>
                    <td className="whitespace-nowrap px-4 py-2.5">
                      <div className="text-ink" title={job.cron}>
                        {cronSummary(job.cron)}
                      </div>
                      <div className="mt-0.5 font-mono text-[10px] text-ink-faint">
                        {job.schedule.timezone}
                      </div>
                    </td>
                    <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                      {job.enabled ? formatUnixSeconds(job.nextExecution) : "-"}
                    </td>
                    <td className="whitespace-nowrap px-4 py-2.5 text-ink-dim tabular-nums">
                      {formatUnixSeconds(job.lastExecution)}
                      {/* lastDuration / duration 按毫秒处理（与 cron-job.org 日志里的 Duration 一致）。 */}
                      {job.lastExecution > 0 && job.lastDuration > 0
                        ? ` · ${formatDuration(job.lastDuration)}`
                        : ""}
                    </td>
                    <td className="px-4 py-2.5">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          size="sm"
                          variant="ghost"
                          title={job.enabled ? t("cronJobs.disable") : t("cronJobs.enable")}
                          disabled={busy}
                          onClick={() => void handleToggle(job, !job.enabled)}
                          icon={<Power className={cn("size-3.5", job.enabled && "text-pos")} />}
                        />
                        <Button
                          size="sm"
                          variant="ghost"
                          title={t("cronJobs.history")}
                          onClick={() => void openHistory(job)}
                          icon={<History className="size-3.5" />}
                        />
                        <Button
                          size="sm"
                          variant="ghost"
                          title={t("cronJobs.edit")}
                          onClick={() => openEdit(job)}
                          icon={<Pencil className="size-3.5" />}
                        />
                        <Button
                          size="sm"
                          variant="ghost"
                          className="text-neg opacity-0 hover:bg-neg-soft hover:text-neg group-hover:opacity-100"
                          title={t("cronJobs.remove")}
                          onClick={() => setRemoving(job)}
                          icon={<Trash2 className="size-3.5" />}
                        />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>
          <p className="mt-3 text-[11px] leading-relaxed text-ink-faint">
            {t("cronJobs.apiHint")}
          </p>
        </>
      )}

      <CronJobModal
        open={modalOpen}
        job={editing}
        onClose={() => setModalOpen(false)}
        onSaved={(title) => {
          toast("success", t("cronJobs.saved", { title }));
          void reload();
        }}
      />

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title={t("cronJobs.removeTitle")}
        confirmText={t("cronJobs.removeConfirmText")}
        description={
          <Trans
            i18nKey="cronJobs.removeDescription"
            values={{ title: removing?.title }}
            components={{ b: <b className="text-ink" />, br: <br /> }}
          />
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleRemove()}
      />

      <Modal
        open={!!history}
        onClose={() => setHistory(null)}
        title={t("cronJobs.historyTitle", { title: history?.job.title ?? "" })}
        subtitle={t("cronJobs.historySubtitle")}
        width="max-w-2xl"
        footer={
          <Button variant="secondary" onClick={() => setHistory(null)}>
            {t("common.close")}
          </Button>
        }
      >
        {history?.loading ? (
          <p className="py-6 text-center text-xs text-ink-faint">{t("common.loading")}</p>
        ) : history && history.runs.length > 0 ? (
          <ul className="divide-y divide-line">
            {history.runs.map((run) => (
              <li
                key={run.identifier || `${run.date}-${run.status}`}
                className="flex items-center gap-3 py-2 text-xs"
              >
                <span className="w-32 shrink-0 tabular-nums text-ink-dim">
                  {formatUnixSeconds(run.date)}
                </span>
                <Badge kind={run.httpStatus >= 200 && run.httpStatus < 300 ? "green" : "red"}>
                  {run.httpStatus || run.statusText || "-"}
                </Badge>
                <span className="truncate text-ink-faint">{run.statusText}</span>
                <span className="ml-auto shrink-0 tabular-nums text-ink-faint">
                  {formatDuration(run.duration)}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="py-6 text-center text-xs text-ink-faint">{t("cronJobs.historyEmpty")}</p>
        )}
      </Modal>
    </Page>
  );
}
