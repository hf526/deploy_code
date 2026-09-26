import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  Copy,
  Database,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { LogConsole } from "../components/LogConsole";
import { ExpandableRecordRow, RecordLog } from "../components/RecordRows";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Page,
  SectionTitle,
  Select,
  SelectBox,
} from "../components/ui";
import { api } from "../lib/api";
import { pruneSelection, selectionState, toggleAll, toggleId } from "../lib/selection";
import { useApp } from "../lib/store";
import type { BackupConfig, BackupRecord, Settings } from "../lib/types";
import { cn, deployStatusLabel, statusBadgeKind } from "../lib/utils";
import { BackupConfigModal } from "./backup/BackupConfigModal";

function statusBadge(record: BackupRecord) {
  return <Badge kind={statusBadgeKind(record.status)}>{deployStatusLabel(record.status)}</Badge>;
}

function humanSize(bytes: number): string {
  if (!bytes) return "-";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

export default function BackupsPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const backups = useApp((state) => state.backups);
  const backupTargets = useApp((state) => state.backupTargets);
  const backupConfigs = useApp((state) => state.backupConfigs);
  const settings = useApp((state) => state.settings);
  const liveBackup = useApp((state) => state.liveBackup);
  const startBackup = useApp((state) => state.startBackup);
  const refreshBackups = useApp((state) => state.refreshBackups);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const refreshBackupTargets = useApp((state) => state.refreshBackupTargets);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);

  const [editing, setEditing] = useState<{
    config: BackupConfig | null;
    /** 复制已有配置：仅预填参数，保存时新建（见 BackupConfigModal）。 */
    duplicate?: boolean;
  } | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [selectedRecords, setSelectedRecords] = useState<Set<string>>(() => new Set());
  const [bulkRemoving, setBulkRemoving] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<BackupConfig | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [runningConfigId, setRunningConfigId] = useState("");
  const [scheduleTime, setScheduleTime] = useState(settings.scheduledBackupTime);
  // 设置保存串行化：连续修改时按顺序提交，避免在途请求用旧快照互相覆盖。
  const scheduleQueue = useRef<Promise<void>>(Promise.resolve());

  // 配置 / 目标 / 设置可能被 CLI 改动，进入页面时拉取一次最新数据，避免展示与后端不一致。
  useEffect(() => {
    void refreshBackupConfigs();
    void refreshBackupTargets();
    void api
      .getSettings()
      .then((saved) => setSettings(saved))
      .catch(() => undefined);
  }, [refreshBackupConfigs, refreshBackupTargets, setSettings]);

  useEffect(() => {
    setScheduleTime(settings.scheduledBackupTime);
  }, [settings.scheduledBackupTime]);

  // 备份结束后清空列表项的进行中标记，使「立即备份」按钮恢复可用。
  useEffect(() => {
    if (liveBackup?.status !== "running") setRunningConfigId("");
  }, [liveBackup?.status]);

  // 进行中的记录不参与多选：删掉正在写入的条目，下一个事件又会把它写回来。
  const selectableRecordIds = useMemo(
    () => backups.filter((record) => record.status !== "running").map((record) => record.id),
    [backups],
  );

  // 后台刷新或 CLI 改过列表后，丢掉已经不存在的勾选。
  useEffect(() => {
    setSelectedRecords((current) => {
      const pruned = pruneSelection(current, selectableRecordIds);
      return pruned.size === current.size ? current : pruned;
    });
  }, [selectableRecordIds]);

  /** 定时备份设置即时保存（开关 / 时间 / 配置选择）。 */
  async function handleSaveSchedule(patch: Partial<Settings>) {
    const run = scheduleQueue.current.then(async () => {
      const current = useApp.getState().settings;
      const saved = await api.saveSettings({ ...current, ...patch });
      setSettings(saved);
      toast("success", t("backup.schedule.saved"));
    });
    scheduleQueue.current = run.catch(() => undefined);
    try {
      await run;
    } catch (error) {
      toast("error", String(error));
    }
  }

  const running = liveBackup?.status === "running";

  /** 列表展示用的目标（与后端 resolve_target 的优先级一致，仅用于展示）；未配置时为 null。 */
  function resolveTarget(config: BackupConfig): { name: string; url: string } | null {
    if (config.supabaseUrl?.trim()) {
      return { name: t("backup.customUrl"), url: config.supabaseUrl.trim() };
    }
    const bound = backupTargets.find((target) => target.id === config.targetId);
    if (bound) return bound;
    const server = servers.find((item) => item.id === config.serverId);
    const serverBound = backupTargets.find((target) => target.id === server?.backupTargetId);
    if (serverBound) return serverBound;
    if (server?.supabaseUrl?.trim()) {
      return { name: t("backup.serverCustomUrl"), url: server.supabaseUrl.trim() };
    }
    const global = backupTargets.find((target) => target.id === settings.defaultBackupTargetId);
    if (global) return global;
    if (settings.supabaseUrl.trim()) {
      return { name: t("settings.backupTargets.legacyName"), url: settings.supabaseUrl.trim() };
    }
    return null;
  }

  /** 列表内的一键备份：直接用已保存配置执行，不经过表单。 */
  async function handleRunConfig(config: BackupConfig) {
    if (running) return;
    if (!resolveTarget(config)) {
      toast("error", t("backup.noTarget"));
      return;
    }
    // 不在这里清空 live：startBackup 会写入新的 running 状态，
    // 提前清空会让列表项的 loading 标记立刻被 effect 重置，看不到进度。
    setRunningConfigId(config.id);
    try {
      await startBackup({ serverId: config.serverId, backupConfigId: config.id });
    } catch {
      // store 已提示错误：liveBackup 会被置空，下方 effect 会清掉行内 loading 标记。
    }
  }

  async function handleDeleteConfig() {
    if (!deleteTarget || deleting) return;
    setDeleting(true);
    try {
      await api.deleteBackupConfig(deleteTarget.id);
      await refreshBackupConfigs();
      // 后端会清掉指向该配置的定时备份引用，同步最新设置避免界面残留悬空选项。
      setSettings(await api.getSettings());
      toast("success", t("backup.configDeleted", { name: deleteTarget.name }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setDeleting(false);
      setDeleteTarget(null);
    }
  }

  async function handleDelete(recordId: string) {
    try {
      await api.deleteBackup(recordId);
      await refreshBackups();
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleBulkDeleteRecords() {
    const ids = [...selectedRecords];
    if (ids.length === 0) return;
    setBulkBusy(true);
    try {
      const removed = await api.deleteBackups(ids);
      toast("success", t("common.bulkDeleted", { count: removed }));
      setSelectedRecords(new Set());
      setBulkRemoving(false);
      await refreshBackups();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBulkBusy(false);
    }
  }

  async function handleClear() {
    try {
      await api.clearBackups();
      await refreshBackups();
      toast("success", t("backup.cleared"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClear(false);
    }
  }

  const selectAllRecords = selectionState(selectedRecords, selectableRecordIds);

  return (
    <Page
      title={t("backup.title")}
      subtitle={t("backup.subtitle")}
      actions={
        <Button
          icon={<RefreshCw className="size-4" />}
          variant="secondary"
          onClick={() => void refreshBackups()}
        >
          {t("backup.refreshRecords")}
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,1fr)_400px]">
        <div className="flex min-w-0 flex-col gap-6">
          <section>
            <SectionTitle
              title={t("backup.configSection")}
              description={t("backup.configDescription")}
              actions={
                <Button
                  size="sm"
                  icon={<Plus className="size-3.5" />}
                  onClick={() => setEditing({ config: null })}
                >
                  {t("backup.newConfig")}
                </Button>
              }
            />
            {backupConfigs.length === 0 ? (
              <EmptyState
                icon={<Database className="size-4.5" />}
                title={t("backup.noConfigs")}
                description={t("backup.noConfigsDescription")}
                action={
                  <Button
                    icon={<Plus className="size-4" />}
                    onClick={() => setEditing({ config: null })}
                  >
                    {t("backup.newConfig")}
                  </Button>
                }
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {backupConfigs.map((config) => {
                  const server = servers.find((item) => item.id === config.serverId) ?? null;
                  const target = resolveTarget(config);
                  return (
                    <div key={config.id} className="flex items-center gap-3 px-4 py-3">
                      <span className="grid size-8 shrink-0 place-items-center rounded-md border border-brand-line bg-brand-soft text-brand">
                        <Database className="size-4" />
                      </span>
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-[13px] font-medium text-ink">{config.name}</p>
                        <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                          {server?.name ?? t("backup.unknownServer")} · {config.source.database} ·
                          schema {config.source.schema || "public"} →{" "}
                          <span className={cn(!target && "text-warn")}>
                            {target?.name ?? t("backup.noTargetShort")}
                          </span>
                        </p>
                      </div>
                      <Button
                        size="sm"
                        icon={<Play className="size-3.5" />}
                        loading={runningConfigId === config.id && running}
                        disabled={running || !target}
                        title={!target ? t("backup.noTarget") : undefined}
                        onClick={() => void handleRunConfig(config)}
                      >
                        {t("backup.runConfig")}
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        title={t("backup.editConfig")}
                        disabled={running}
                        onClick={() => setEditing({ config })}
                      >
                        <Pencil className="size-3.5" />
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        title={t("backup.copyConfig")}
                        disabled={running}
                        onClick={() =>
                          setEditing({
                            // 清空 id：保存时新建一条；source 单独拷贝，避免与原配置共享对象。
                            config: {
                              ...config,
                              id: "",
                              name: t("backup.copyName", { name: config.name }),
                              source: { ...config.source },
                            },
                            duplicate: true,
                          })
                        }
                      >
                        <Copy className="size-3.5" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        title={t("backup.deleteConfig")}
                        disabled={running}
                        onClick={() => setDeleteTarget(config)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  );
                })}
              </Card>
            )}
          </section>

          <section>
            <SectionTitle
              title={t("backup.schedule.title")}
              description={t("backup.schedule.description")}
            />
            <Card className="flex flex-col gap-4 p-5">
              <Checkbox
                checked={settings.scheduledBackupEnabled}
                onChange={(checked) =>
                  void handleSaveSchedule({ scheduledBackupEnabled: checked })
                }
              >
                {t("backup.schedule.enable")}
              </Checkbox>
              <div className="grid grid-cols-2 gap-4">
                <Field label={t("backup.schedule.time")}>
                  <Input
                    type="time"
                    value={scheduleTime}
                    onChange={(event) => {
                      setScheduleTime(event.target.value);
                      // 时间选择完成即保存，避免改完直接关闭窗口导致修改丢失。
                      if (/^\d{1,2}:\d{2}$/.test(event.target.value)) {
                        void handleSaveSchedule({ scheduledBackupTime: event.target.value });
                      }
                    }}
                    onBlur={() => {
                      // 清空或非法值时回填已保存的时间，避免界面与摘要不一致。
                      if (!/^\d{1,2}:\d{2}$/.test(scheduleTime)) {
                        setScheduleTime(settings.scheduledBackupTime);
                      }
                    }}
                  />
                </Field>
                <Field label={t("backup.schedule.config")}>
                  <Select
                    value={settings.scheduledBackupConfigId ?? ""}
                    onChange={(event) =>
                      void handleSaveSchedule({
                        scheduledBackupConfigId: event.target.value || null,
                      })
                    }
                  >
                    <option value="">{t("backup.schedule.noConfigOption")}</option>
                    {backupConfigs.map((config) => (
                      <option key={config.id} value={config.id}>
                        {config.name} · {config.source.database}
                      </option>
                    ))}
                  </Select>
                </Field>
              </div>
              <p className="flex items-center gap-1.5 text-[11px] leading-relaxed text-ink-faint">
                <CalendarClock className="size-3.5 shrink-0" />
                {settings.scheduledBackupEnabled
                  ? t("backup.schedule.summary", { time: settings.scheduledBackupTime })
                  : t("backup.schedule.disabled")}
              </p>
            </Card>
          </section>
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          <section className="flex min-h-[320px] flex-col">
            <SectionTitle
              title={t("backup.liveLog")}
              description={running ? t("backup.liveLogRunning") : t("backup.liveLogIdle")}
            />
            {liveBackup && (
              <div className="mb-2 h-1 overflow-hidden rounded-full bg-line">
                <div
                  className="h-full rounded-full bg-brand transition-all"
                  style={{ width: `${liveBackup.progress}%` }}
                />
              </div>
            )}
            <LogConsole
              className="min-h-[280px] flex-1"
              title={t("backup.logTitle")}
              emptyText={t("backup.logEmpty")}
              lines={liveBackup?.lines ?? []}
            />
          </section>

          <section>
            <SectionTitle
              title={t("backup.records")}
              description={t("backup.recordsCount", { count: backups.length })}
              actions={
                backups.length > 0 ? (
                  <div className="flex items-center gap-2">
                    <SelectBox
                      label={t("common.selectAll")}
                      checked={selectAllRecords === "all"}
                      indeterminate={selectAllRecords === "some"}
                      disabled={selectableRecordIds.length === 0}
                      onChange={() =>
                        setSelectedRecords((current) =>
                          toggleAll(current, selectableRecordIds),
                        )
                      }
                    />
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={selectedRecords.size === 0}
                      onClick={() => setBulkRemoving(true)}
                    >
                      {t("common.deleteSelected", { count: selectedRecords.size })}
                    </Button>
                    <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                      {t("backup.clearRecords")}
                    </Button>
                  </div>
                ) : undefined
              }
            />
            {backups.length === 0 ? (
              <EmptyState
                icon={<Database className="size-4.5" />}
                title={t("backup.emptyTitle")}
                description={t("backup.emptyDescription")}
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {backups.map((record) => (
                  <ExpandableRecordRow
                    key={record.id}
                    expanded={expanded === record.id}
                    onToggle={() => setExpanded(expanded === record.id ? null : record.id)}
                    badge={statusBadge(record)}
                    deleteTitle={t("backup.deleteRecord")}
                    onDelete={() => void handleDelete(record.id)}
                    selected={selectedRecords.has(record.id)}
                    selectionDisabled={record.status === "running"}
                    selectionLabel={
                      record.status === "running"
                        ? t("common.runningNotDeletable")
                        : t("common.select")
                    }
                    onSelect={(on) =>
                      setSelectedRecords((current) => toggleId(current, record.id, on))
                    }
                    title={
                      <>
                        {record.serverName} · {record.database}
                        <span className="ml-2 text-[11px] font-normal text-ink-faint">
                          schema {record.schema}
                        </span>
                      </>
                    }
                    subtitle={
                      <>
                        {record.startedAt} · {humanSize(record.dumpSize)} ·{" "}
                        {record.targetName || t("backup.targetFallback")} · {record.target}
                      </>
                    }
                    log={
                      <RecordLog
                        error={record.error}
                        errorPrefix={(error) => t("backup.errorPrefix", { error })}
                        log={record.log}
                        emptyText={t("backup.noLog")}
                      />
                    }
                  />
                ))}
              </Card>
            )}
          </section>
        </div>
      </div>

      {editing && (
        <BackupConfigModal
          config={editing.config}
          duplicate={editing.duplicate ?? false}
          onClose={() => setEditing(null)}
          onRefresh={() => void refreshBackupConfigs()}
          onSaved={(saved) => {
            setEditing(null);
            void refreshBackupConfigs();
            toast("success", t("backup.configSaved", { name: saved.name }));
          }}
        />
      )}

      <ConfirmModal
        open={bulkRemoving}
        danger
        loading={bulkBusy}
        title={t("backup.bulkDeleteTitle")}
        confirmText={t("common.delete")}
        description={t("backup.bulkDeleteDescription", { count: selectedRecords.size })}
        onCancel={() => setBulkRemoving(false)}
        onConfirm={() => void handleBulkDeleteRecords()}
      />

      <ConfirmModal
        open={confirmClear}
        danger
        title={t("backup.clearTitle")}
        confirmText={t("common.clear")}
        description={t("backup.clearDescription")}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void handleClear()}
      />

      <ConfirmModal
        open={deleteTarget !== null}
        danger
        loading={deleting}
        title={t("backup.configDeleteTitle")}
        confirmText={t("common.delete")}
        description={t("backup.configDeleteDescription", { name: deleteTarget?.name ?? "" })}
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => void handleDeleteConfig()}
      />
    </Page>
  );
}
