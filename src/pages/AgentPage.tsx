import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import {
  RefreshCw,
  ServerCog,
  SquareTerminal,
  Trash2,
  Upload,
} from "lucide-react";

import {
  Badge,
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Page,
  SectionTitle,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { diskLevel, splitByLocation, syncNeeded } from "../lib/agent";
import { useApp } from "../lib/store";
import type { AgentStatus, AgentSyncReport } from "../lib/types";
import { cn, formatDuration, humanSize } from "../lib/utils";

/** 控制机卡片上的记录来源。 */
type RecordKind = "backup" | "container";

export default function AgentPage() {
  const { t } = useTranslation();
  const settings = useApp((state) => state.settings);
  const servers = useApp((state) => state.servers);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);
  // 配置列表只认 store 那一份：页面自己再存一份就会在「卸载 / 收回执行位」之后留着旧值，
  // 而备份与容器弹窗正是从 store 里取执行位的，旧值会被原样写回去。
  const backupConfigs = useApp((state) => state.backupConfigs);
  const containerConfigs = useApp((state) => state.containerConfigs);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const refreshContainerConfigs = useApp((state) => state.refreshContainerConfigs);

  const [status, setStatus] = useState<AgentStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<"install" | "sync" | "uninstall" | null>(null);
  const [report, setReport] = useState<AgentSyncReport | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsLoading, setLogsLoading] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [recordKind, setRecordKind] = useState<RecordKind>("backup");
  const [records, setRecords] = useState<
    { kind: RecordKind; items: Array<AgentRecordRow> } | null
  >(null);
  const [recordsLoading, setRecordsLoading] = useState(false);
  // 页面切换 / StrictMode 双挂载都会带来迟到的响应，用它把过期的丢弃。
  const seq = useRef(0);

  const agentServerId = settings.agentServerId.trim();
  const agentServer = servers.find((item) => item.id === agentServerId);

  const reload = useCallback(async (silent = false) => {
    const mine = seq.current + 1;
    seq.current = mine;
    if (!silent) setLoading(true);
    try {
      const next = await api.agentStatus();
      if (seq.current !== mine) return;
      setStatus(next);
    } catch (error) {
      if (seq.current === mine && !silent) toast("error", String(error));
      if (seq.current === mine) setStatus(null);
    } finally {
      if (seq.current === mine) setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    if (!agentServerId) {
      setStatus(null);
      return;
    }
    void reload(true);
    void refreshBackupConfigs();
    void refreshContainerConfigs();
  }, [agentServerId, reload, refreshBackupConfigs, refreshContainerConfigs]);

  // 换来源之后旧的记录先清掉，否则屏幕上会短暂留着另一类任务的表。
  useEffect(() => {
    setRecords(null);
  }, [recordKind]);

  async function handleInstall() {
    if (!agentServerId) {
      toast("error", t("agent.pickFirst"));
      return;
    }
    setBusy("install");
    try {
      const next = await api.installAgent(agentServerId);
      setStatus(next);
      toast("success", t("agent.installed", { version: next.version }));
      void reload(true);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleSync() {
    setBusy("sync");
    try {
      const result = await api.agentSync();
      setReport(result);
      setStatus(result.status);
      toast("success", t("agent.synced", { servers: result.servers }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(null);
    }
  }

  async function handleUninstall() {
    setBusy("uninstall");
    try {
      const message = await api.uninstallAgent(agentServerId);
      setStatus(null);
      setLogs([]);
      // 后端这时已经把执行位收回本机、清空了同步指纹。这几份副本不跟着刷，
      // 备份/容器弹窗就会拿旧的 "remote" 再写回去，那一晚两头都不跑。
      setSettings(await api.getSettings());
      await Promise.all([refreshBackupConfigs(), refreshContainerConfigs()]);
      toast("success", t("agent.uninstalled"));
      void message;
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(null);
      setRemoving(false);
    }
  }

  async function handleLogs() {
    setLogsLoading(true);
    try {
      setLogs(await api.agentLogs(200));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setLogsLoading(false);
    }
  }

  async function handleRecords() {
    setRecordsLoading(true);
    try {
      const items: AgentRecordRow[] =
        recordKind === "backup"
          ? (await api.agentBackupRecords(20)).map((record) => ({
              id: record.id,
              time: record.startedAt,
              name: `${record.serverName} · ${record.database}`,
              size: record.dumpSize,
              status: record.status,
              durationMs: record.durationMs,
              path: record.bundlePath,
            }))
          : (await api.agentContainerRecords(20)).map((record) => ({
              id: record.id,
              time: record.startedAt,
              name: `${record.project} · ${record.serverName}`,
              size: record.bundleSize,
              status: record.status,
              durationMs: record.durationMs,
              path: record.bundlePath,
            }));
      setRecords({ kind: recordKind, items });
    } catch (error) {
      toast("error", String(error));
    } finally {
      setRecordsLoading(false);
    }
  }

  async function handlePickServer(value: string) {
    const saved = await api.saveSettings({ ...settings, agentServerId: value });
    setSettings(saved);
    setStatus(null);
    setReport(null);
  }

  async function handleBinaryPath(value: string) {
    try {
      const saved = await api.saveSettings({ ...settings, agentBinaryPath: value });
      setSettings(saved);
    } catch (error) {
      toast("error", String(error));
    }
  }

  const remoteBackup = splitByLocation(backupConfigs).remote;
  const remoteContainer = splitByLocation(containerConfigs).remote;
  const needsSync = syncNeeded(status, {
    backup: backupConfigs,
    container: containerConfigs,
    servers: servers.length,
  });
  const room = status ? diskLevel(status) : "ok";

  if (!servers.length) {
    return (
      <Page title={t("agent.title")} subtitle={t("agent.subtitle")}>
        <EmptyState
          icon={<ServerCog size={22} />}
          title={t("agent.noServers")}
          description={t("agent.noServersHint")}
          action={
            <Link to="/servers">
              <Button variant="secondary">{t("agent.goServers")}</Button>
            </Link>
          }
        />
      </Page>
    );
  }

  return (
    <Page
      title={t("agent.title")}
      subtitle={agentServer ? `${agentServer.name} · ${agentServer.host}` : t("agent.subtitle")}
      actions={
        <>
          <Button variant="ghost" onClick={() => void reload()} disabled={loading}>
            <RefreshCw size={14} className={cn(loading && "animate-spin")} />
            {t("agent.refresh")}
          </Button>
          <Button variant="secondary" onClick={() => void handleSync()} disabled={busy !== null || !status}>
            <Upload size={14} />
            {t("agent.sync")}
          </Button>
          <Button variant="primary" onClick={() => void handleInstall()} disabled={busy !== null}>
            {busy === "install" ? t("agent.installing") : t("agent.install")}
          </Button>
        </>
      }
    >
      <div className="space-y-5">
        <Card className="p-4">
          <div className="grid gap-4 md:grid-cols-2">
            <Field label={t("agent.pickLabel")} hint={t("agent.pickHint")}>
              <Select
                value={agentServerId}
                onChange={(event) => void handlePickServer(event.target.value)}
              >
                <option value="">{t("agent.pickNone")}</option>
                {servers.map((server) => (
                  <option key={server.id} value={server.id}>
                    {server.name} · {server.host}
                  </option>
                ))}
              </Select>
            </Field>
            <Field label={t("agent.binaryLabel")} hint={t("agent.binaryHint")}>
              <Input
                value={settings.agentBinaryPath}
                placeholder={t("agent.binaryPlaceholder")}
                onChange={(event) => void handleBinaryPath(event.target.value)}
              />
            </Field>
          </div>
          <p className="mt-3 text-[12px] leading-relaxed text-ink-dim">{t("agent.portless")}</p>
        </Card>

        {!status ? (
          <EmptyState
            icon={<ServerCog size={22} />}
            title={agentServerId ? t("agent.notInstalled") : t("agent.pickFirst")}
            description={t("agent.notInstalledHint")}
          />
        ) : (
          <>
            <Card className="p-4">
              <SectionTitle title={t("agent.statusTitle")} />
              <div className="mt-3 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <Metric label={t("agent.metricVersion")} value={`${status.version} · proto ${status.proto}`} />
                <Metric
                  label={t("agent.metricClock")}
                  value={`${status.localTime} (${status.timezone})`}
                  hint={t("agent.metricClockHint")}
                />
                <Metric
                  label={t("agent.metricDisk")}
                  value={`${humanSize(status.freeBytes)} / ${humanSize(status.totalBytes)}`}
                  hint={t("agent.metricDiskHint", { floor: humanSize(status.floorBytes) })}
                  tone={room === "low" ? "neg" : room === "tight" ? "warn" : "pos"}
                />
                <Metric
                  label={t("agent.metricBundles")}
                  value={`${status.bundleCount} · ${humanSize(status.bundleBytes)}`}
                  hint={t("agent.metricBundlesHint")}
                />
              </div>
              <div className="mt-4 grid gap-3 sm:grid-cols-2">
                <ScheduleRow
                  label={t("agent.scheduleBackup")}
                  enabled={status.backupEnabled}
                  time={status.backupTime}
                  localTime={settings.scheduledBackupTime}
                  detail={status.backupConfigName || t("agent.noConfigSelected")}
                  lastRun={status.backupLastRun}
                />
                <ScheduleRow
                  label={t("agent.scheduleContainer")}
                  enabled={status.containerEnabled}
                  time={status.containerTime}
                  localTime={settings.scheduledContainerTime}
                  detail={t("agent.queueCount", { count: status.containerQueue })}
                  lastRun={status.containerLastRun}
                />
              </div>
              {room === "low" && (
                <p className="mt-3 text-[12px] text-neg">{t("agent.diskLowWarning")}</p>
              )}
            </Card>

            <Card className="p-4">
              <SectionTitle title={t("agent.ownedTitle")} />
              <p className="mt-2 text-[12.5px] text-ink-dim">{t("agent.ownedHint")}</p>
              <ConfigList
                title={t("agent.ownedBackup")}
                to="/backups"
                linkLabel={t("agent.goBackups")}
                items={remoteBackup.map((item) => `${item.name} · ${item.source.database}`)}
              />
              <ConfigList
                title={t("agent.ownedContainer")}
                to="/containers"
                linkLabel={t("agent.goContainers")}
                items={remoteContainer.map((item) => `${item.name} · ${item.project}`)}
              />
              {needsSync && (
                <p className="mt-3 text-[12px] text-warn">{t("agent.syncStale")}</p>
              )}
              {report && report.warnings.length > 0 && (
                <ul className="mt-3 space-y-1">
                  {report.warnings.map((warning) => (
                    <li key={warning} className="text-[12px] text-warn">
                      {warning}
                    </li>
                  ))}
                </ul>
              )}
            </Card>

            <Card className="p-4">
              <div className="flex items-center justify-between gap-3">
                <SectionTitle title={t("agent.recordsTitle")} />
                <div className="flex items-center gap-2">
                  <Select
                    className="w-36"
                    value={recordKind}
                    onChange={(event) => setRecordKind(event.target.value as RecordKind)}
                  >
                    <option value="backup">{t("agent.recordsBackup")}</option>
                    <option value="container">{t("agent.recordsContainer")}</option>
                  </Select>
                  <Button variant="ghost" onClick={() => void handleRecords()} disabled={recordsLoading}>
                    {recordsLoading ? t("agent.loading") : t("agent.recordsLoad")}
                  </Button>
                </div>
              </div>
              {records && records.kind === recordKind ? (
                records.items.length ? (
                  <div className="mt-3 overflow-x-auto">
                    <table className="w-full text-[12.5px]">
                      <thead className="text-left text-ink-dim">
                        <tr>
                          <th className="py-1 pr-3 font-medium">{t("agent.colId")}</th>
                          <th className="py-1 pr-3 font-medium">{t("agent.colTime")}</th>
                          <th className="py-1 pr-3 font-medium">{t("agent.colWhat")}</th>
                          <th className="py-1 pr-3 font-medium">{t("agent.colSize")}</th>
                          <th className="py-1 pr-3 font-medium">{t("agent.colStatus")}</th>
                          <th className="py-1 font-medium">{t("agent.colDuration")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {records.items.map((row) => (
                          <tr key={row.id} className="border-t border-line">
                            <td className="py-1.5 pr-3 font-mono text-ink-dim">{row.id.slice(0, 8)}</td>
                            <td className="py-1.5 pr-3 whitespace-nowrap">{row.time}</td>
                            <td className="py-1.5 pr-3">{row.name}</td>
                            <td className="py-1.5 pr-3 whitespace-nowrap">{humanSize(row.size)}</td>
                            <td className="py-1.5 pr-3">
                              <Badge kind={row.status === "success" ? "green" : row.status === "running" ? "brand" : "red"}>
                                {t(`status.${row.status}`)}
                              </Badge>
                            </td>
                            <td className="py-1.5 whitespace-nowrap">{formatDuration(row.durationMs)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    <p className="mt-2 text-[11.5px] text-ink-dim">{t("agent.recordsFootnote")}</p>
                  </div>
                ) : (
                  <p className="mt-3 text-[12.5px] text-ink-dim">{t("agent.recordsEmpty")}</p>
                )
              ) : (
                <p className="mt-3 text-[12.5px] text-ink-dim">{t("agent.recordsIntro")}</p>
              )}
            </Card>

            <Card className="p-4">
              <div className="flex items-center justify-between gap-3">
                <SectionTitle title={t("agent.logsTitle")} />
                <Button variant="ghost" onClick={() => void handleLogs()} disabled={logsLoading}>
                  <SquareTerminal size={14} />
                  {logsLoading ? t("agent.loading") : t("agent.logsLoad")}
                </Button>
              </div>
              {logs.length ? (
                <pre className="ui-scroll mt-3 max-h-72 overflow-auto rounded-md bg-inset p-3 font-mono text-[11.5px] leading-relaxed text-ink">
                  {logs.join("\n")}
                </pre>
              ) : (
                <p className="mt-3 text-[12.5px] text-ink-dim">{t("agent.logsIntro")}</p>
              )}
            </Card>

            <Card className="p-4">
              <SectionTitle title={t("agent.dangerTitle")} />
              <p className="mt-2 text-[12.5px] text-ink-dim">{t("agent.dangerHint")}</p>
              <Button
                className="mt-3"
                variant="danger"
                onClick={() => setRemoving(true)}
                disabled={busy !== null}
              >
                <Trash2 size={14} />
                {busy === "uninstall" ? t("agent.uninstalling") : t("agent.uninstall")}
              </Button>
            </Card>
          </>
        )}
      </div>

      <ConfirmModal
        open={removing}
        title={t("agent.uninstall")}
        description={t("agent.uninstallConfirm")}
        confirmText={t("common.confirm")}
        danger
        loading={busy === "uninstall"}
        onCancel={() => setRemoving(false)}
        onConfirm={() => void handleUninstall()}
      />
    </Page>
  );
}

/** 回读表格里的行：两类记录的字段名不同，先归一成同一形状再渲染。 */
interface AgentRecordRow {
  id: string;
  time: string;
  name: string;
  size: number;
  status: "running" | "success" | "failed";
  durationMs: number;
  path: string;
}

function Metric({
  label,
  value,
  hint,
  tone = "default",
}: {
  label: string;
  value: string;
  hint?: string;
  tone?: "default" | "pos" | "warn" | "neg";
}) {
  return (
    <div className="rounded-md border border-line bg-inset/40 p-3">
      <p className="text-[11.5px] text-ink-dim">{label}</p>
      <p
        className={cn(
          "mt-1 text-[13.5px] font-medium",
          tone === "neg" ? "text-neg" : tone === "warn" ? "text-warn" : tone === "pos" ? "text-ink" : "text-ink",
        )}
      >
        {value}
      </p>
      {hint && <p className="mt-1 text-[11px] leading-snug text-ink-dim">{hint}</p>}
    </div>
  );
}

function ScheduleRow({
  label,
  enabled,
  time,
  localTime,
  detail,
  lastRun,
}: {
  label: string;
  enabled: boolean;
  /** 控制机当地执行的那个 HH:MM（agent 按它自己的时区解释）。 */
  time: string;
  /** 设置里写的 HH:MM，是本机时区的意图时刻。 */
  localTime: string;
  detail: string;
  lastRun: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="rounded-md border border-line p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[12.5px] font-medium text-ink">{label}</span>
        <Badge kind={enabled ? "green" : "gray"}>
          {enabled ? t("agent.on") : t("agent.off")}
        </Badge>
      </div>
      <p className="mt-1 text-[12px] text-ink-dim">
        {enabled ? `${time} · ${detail}` : t("agent.scheduleOff")}
      </p>
      {/* 两机时区不同就要把两个时刻都摊开：只报控制机那份，用户会以为跑的是自己设的点。 */}
      {enabled && localTime.trim() !== time.trim() && (
        <p className="mt-1 text-[11.5px] text-warn">{t("agent.scheduleShifted", { local: localTime, agent: time })}</p>
      )}
      <p className="mt-1 text-[11.5px] text-ink-dim">
        {t("agent.lastRun", { date: lastRun || t("agent.never") })}
      </p>
    </div>
  );
}

function ConfigList({
  title,
  items,
  to,
  linkLabel,
}: {
  title: string;
  items: string[];
  to: string;
  linkLabel: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="mt-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[12px] font-medium text-ink">{title}</span>
        <Link className="text-[12px] text-accent hover:underline" to={to}>
          {linkLabel}
        </Link>
      </div>
      {items.length ? (
        <ul className="mt-1 space-y-0.5">
          {items.map((item) => (
            <li key={item} className="text-[12px] text-ink-dim">
              {item}
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-1 text-[12px] text-ink-dim">{t("agent.noneOwned")}</p>
      )}
    </div>
  );
}
