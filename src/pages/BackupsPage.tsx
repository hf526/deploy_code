import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Database,
  Play,
  Plus,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { LogConsole } from "../components/LogConsole";
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
import i18n from "../lib/i18n";
import { useApp } from "../lib/store";
import type { BackupConfig, BackupRecord, BackupRequest, DbBackupSource } from "../lib/types";
import { maskUrlPassword } from "../lib/utils";

function defaultSource(): DbBackupSource {
  return {
    mode: "docker",
    container: "postgres",
    database: "",
    username: "postgres",
    password: "",
    schema: "public",
  };
}

function normalized(source: DbBackupSource): DbBackupSource {
  return {
    ...source,
    container: source.container.trim(),
    database: source.database.trim(),
    username: source.username.trim(),
    schema: source.schema.trim() || "public",
  };
}

function statusBadge(record: BackupRecord) {
  if (record.status === "success") return <Badge kind="green">{i18n.t("status.success")}</Badge>;
  if (record.status === "failed") return <Badge kind="red">{i18n.t("status.failed")}</Badge>;
  return <Badge kind="amber">{i18n.t("status.running")}</Badge>;
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
  const clearLiveBackup = useApp((state) => state.clearLiveBackup);
  const toast = useApp((state) => state.toast);

  const [configId, setConfigId] = useState("");
  // configId 为空时区分「新建配置」与「尚未加载配置列表」，避免刷新列表时覆盖正在填写的表单。
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [serverId, setServerId] = useState("");
  const [draft, setDraft] = useState<DbBackupSource>(defaultSource());
  const [targetId, setTargetId] = useState("");
  const [overrideUrl, setOverrideUrl] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [starting, setStarting] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [confirmDeleteConfig, setConfirmDeleteConfig] = useState(false);

  const selectedConfig =
    backupConfigs.find((config) => config.id === configId) ?? null;

  // 配置列表变化时保持选中项有效：首次加载自动选中第一条，选中的配置被删除时切换。
  useEffect(() => {
    if (creating) return;
    if (configId && backupConfigs.some((config) => config.id === configId)) return;
    if (backupConfigs.length > 0) {
      setConfigId(backupConfigs[0].id);
      return;
    }
    setConfigId("");
    setName("");
    setDraft(defaultSource());
    setTargetId("");
    setOverrideUrl("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backupConfigs, creating]);

  // 选中配置后把配置内容填充到表单。
  useEffect(() => {
    const config = backupConfigs.find((item) => item.id === configId);
    if (!config) return;
    setName(config.name);
    setServerId(config.serverId);
    setDraft(config.source);
    setTargetId(config.targetId ?? "");
    setOverrideUrl(config.supabaseUrl ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [configId, backupConfigs]);

  useEffect(() => {
    if (!serverId && servers.length > 0) {
      setServerId(servers[0].id);
    }
  }, [servers, serverId]);

  const running = liveBackup?.status === "running";
  const selectedServer = servers.find((server) => server.id === serverId) ?? null;

  const targetById = (id: string | null | undefined) =>
    backupTargets.find((target) => target.id === id) ?? null;
  // 服务器 / 配置可能绑定了已被删除的目标：悬空 id 一律按未绑定处理。
  const validTargetId = backupTargets.some((target) => target.id === targetId)
    ? targetId
    : "";
  // 与后端 resolve_target 的优先级保持一致：
  // 表单连接串 > 表单目标 > 服务器绑定目标 > 服务器自定义连接串 > 全局默认 > 旧版连接串。
  const effectiveTarget = useMemo(() => {
    if (overrideUrl.trim()) {
      return { name: t("backup.customUrl"), url: overrideUrl.trim() };
    }
    const bound = targetById(validTargetId);
    if (bound) return { name: bound.name, url: bound.url };
    const serverBound = targetById(selectedServer?.backupTargetId);
    if (serverBound) return { name: serverBound.name, url: serverBound.url };
    if (selectedServer?.supabaseUrl?.trim()) {
      return { name: t("backup.serverCustomUrl"), url: selectedServer.supabaseUrl.trim() };
    }
    const global = targetById(settings.defaultBackupTargetId);
    if (global) return { name: global.name, url: global.url };
    if (settings.supabaseUrl.trim()) {
      return { name: t("settings.backupTargets.legacyName"), url: settings.supabaseUrl.trim() };
    }
    return null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overrideUrl, validTargetId, selectedServer, settings, backupTargets, t]);

  /** 测试与实际备份使用同一份来源与目标。 */
  function buildRequest(): BackupRequest {
    return {
      serverId,
      backupConfigId: selectedConfig?.id ?? null,
      source: normalized(draft),
      targetId: validTargetId || null,
      supabaseUrl: overrideUrl.trim() || null,
    };
  }

  function handleNew() {
    setCreating(true);
    setConfigId("");
    setName("");
    setServerId(servers[0]?.id ?? "");
    setDraft(defaultSource());
    setTargetId("");
    setOverrideUrl("");
  }

  /** 保存当前表单；通过时返回保存后的配置。 */
  async function saveForm(): Promise<BackupConfig | null> {
    if (!name.trim()) {
      toast("error", t("backup.configNameRequired"));
      return null;
    }
    if (!serverId) {
      toast("error", t("backup.serverRequired"));
      return null;
    }
    const saved = await api.saveBackupConfig({
      id: selectedConfig?.id ?? "",
      name: name.trim(),
      serverId,
      source: normalized(draft),
      targetId: validTargetId || null,
      supabaseUrl: overrideUrl.trim() || null,
    });
    await refreshBackupConfigs();
    setCreating(false);
    setConfigId(saved.id);
    return saved;
  }

  async function handleSave() {
    setSaving(true);
    try {
      const saved = await saveForm();
      if (saved) toast("success", t("backup.configSaved", { name: saved.name }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  async function handleTest() {
    if (testing || starting) return;
    if (!serverId) {
      toast("error", t("backup.serverRequired"));
      return;
    }
    setTesting(true);
    try {
      // 已保存的配置：先保存当前表单再测试，否则后端会回退到配置里的旧目标（与 Start 不一致）。
      if (selectedConfig) {
        const saved = await saveForm();
        if (!saved) return;
      }
      const message = await api.testBackup(buildRequest());
      toast("success", message || t("backup.testPassed"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTesting(false);
    }
  }

  async function handleStart() {
    if (starting || testing) return;
    if (!draft.database.trim()) {
      toast("error", t("backup.databaseRequired"));
      return;
    }
    setStarting(true);
    try {
      // 先保存当前配置，保证 CLI / 后续备份使用同一份参数。
      const saved = await saveForm();
      if (!saved) return;
      clearLiveBackup();
      await startBackup({
        serverId,
        backupConfigId: saved.id,
        targetId: validTargetId || null,
        supabaseUrl: overrideUrl.trim() || null,
      });
    } catch (error) {
      toast("error", String(error));
    } finally {
      setStarting(false);
    }
  }

  async function handleDeleteConfig() {
    if (!selectedConfig) return;
    try {
      await api.deleteBackupConfig(selectedConfig.id);
      await refreshBackupConfigs();
      toast("success", t("backup.configDeleted", { name: selectedConfig.name }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmDeleteConfig(false);
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
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-6">
          <section>
            <SectionTitle
              title={t("backup.configSection")}
              description={t("backup.configDescription")}
            />
            <Card className="flex flex-col gap-4 p-5">
              <div className="block">
                <span className="mb-1 flex items-baseline gap-1 text-xs font-medium text-ink-dim">
                  {t("backup.savedConfig")}
                  <span className="ml-auto text-[11px] font-normal text-ink-faint">
                    {t("backup.savedConfigHint")}
                  </span>
                </span>
                <div className="flex items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <Select
                      value={configId}
                      onChange={(event) => {
                        const value = event.target.value;
                        setConfigId(value);
                        setCreating(value === "");
                      }}
                    >
                      <option value="">{t("backup.newConfig")}</option>
                      {backupConfigs.map((config) => (
                        <option key={config.id} value={config.id}>
                          {config.name} · {config.source.database}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <Button
                    variant="secondary"
                    size="sm"
                    title={t("backup.newConfig")}
                    onClick={handleNew}
                  >
                    <Plus className="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={!selectedConfig}
                    title={t("backup.deleteConfig")}
                    onClick={() => setConfirmDeleteConfig(true)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
                {backupConfigs.length === 0 && (
                  <p className="mt-2 text-[11px] text-ink-faint">{t("backup.noConfigs")}</p>
                )}
              </div>

              <Field label={t("backup.configName")} required>
                <Input
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder={t("backup.configNamePlaceholder")}
                />
              </Field>

              <Field label={t("backup.server")} required>
                <Select
                  value={serverId}
                  onChange={(event) => setServerId(event.target.value)}
                  disabled={servers.length === 0}
                >
                  {servers.length === 0 && <option value="">{t("backup.noServers")}</option>}
                  {servers.map((server) => (
                    <option key={server.id} value={server.id}>
                      {server.name} ({server.username}@{server.host})
                    </option>
                  ))}
                </Select>
              </Field>

              <div className="grid grid-cols-2 gap-4">
                <Field label={t("backup.mode")}>
                  <Select
                    value={draft.mode}
                    onChange={(event) =>
                      setDraft({ ...draft, mode: event.target.value as DbBackupSource["mode"] })
                    }
                  >
                    <option value="docker">{t("backup.modeDocker")}</option>
                    <option value="system">{t("backup.modeSystem")}</option>
                  </Select>
                </Field>
                <Field label="Schema">
                  <Input
                    value={draft.schema}
                    onChange={(event) => setDraft({ ...draft, schema: event.target.value })}
                    placeholder="public"
                  />
                </Field>
              </div>

              {draft.mode === "docker" && (
                <Field label={t("backup.container")} hint={t("backup.containerHint")} required>
                  <Input
                    value={draft.container}
                    onChange={(event) => setDraft({ ...draft, container: event.target.value })}
                    placeholder="postgres"
                  />
                </Field>
              )}

              <div className="grid grid-cols-2 gap-4">
                <Field label={t("backup.database")} required>
                  <Input
                    value={draft.database}
                    onChange={(event) => setDraft({ ...draft, database: event.target.value })}
                    placeholder="app"
                  />
                </Field>
                <Field label={t("backup.username")} required>
                  <Input
                    value={draft.username}
                    onChange={(event) => setDraft({ ...draft, username: event.target.value })}
                    placeholder="postgres"
                  />
                </Field>
              </div>

              <Field label={t("backup.password")} hint={t("backup.passwordHint")}>
                <Input
                  type="password"
                  value={draft.password}
                  onChange={(event) => setDraft({ ...draft, password: event.target.value })}
                  placeholder={t("backup.passwordPlaceholder")}
                />
              </Field>

              <div className="flex justify-end gap-2 border-t border-line pt-4">
                <Button
                  variant="secondary"
                  loading={testing}
                  disabled={!serverId || starting}
                  onClick={() => void handleTest()}
                >
                  <ShieldCheck className="size-4" />
                  {t("common.testEnvironment")}
                </Button>
                <Button
                  variant="secondary"
                  loading={saving}
                  disabled={testing || starting}
                  onClick={() => void handleSave()}
                >
                  <Save className="size-4" />
                  {t("common.save")}
                </Button>
              </div>
            </Card>
          </section>

          <section>
            <SectionTitle title={t("backup.targetSection")} description={t("backup.targetDescription")} />
            <Card className="flex flex-col gap-4 p-5">
              <Field label={t("backup.useTarget")} hint={t("backup.useTargetHint")}>
                <Select
                  value={validTargetId}
                  onChange={(event) => setTargetId(event.target.value)}
                >
                  <option value="">
                    {t("backup.globalDefault")}
                    {targetById(settings.defaultBackupTargetId)
                      ? ` (${targetById(settings.defaultBackupTargetId)?.name})`
                      : ""}
                  </option>
                  {backupTargets.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label={t("backup.overrideUrl")} hint={t("backup.overrideUrlHint")}>
                <Input
                  value={overrideUrl}
                  onChange={(event) => setOverrideUrl(event.target.value)}
                  placeholder="postgresql://user:password@host:5432/postgres"
                />
              </Field>
              <p className="text-[11px] leading-relaxed text-ink-faint">
                {effectiveTarget ? (
                  <>
                    {t("backup.writingTo")}
                    <span className="ml-1 font-medium text-ink-dim">{effectiveTarget.name}</span>
                    <span className="ml-1 font-mono text-ink-faint">
                      {maskUrlPassword(effectiveTarget.url)}
                    </span>
                  </>
                ) : (
                  <span className="text-warn">{t("backup.noTarget")}</span>
                )}
              </p>
              <Button
                size="lg"
                loading={running || starting}
                disabled={!serverId || !effectiveTarget || testing}
                onClick={() => void handleStart()}
              >
                <Play className="size-4" />
                {running ? t("backup.backupRunning") : t("backup.startBackup")}
              </Button>
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
                  <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                    {t("backup.clearRecords")}
                  </Button>
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
                  <div key={record.id}>
                    <div className="flex items-center gap-3 px-4 py-3">
                      <button
                        className="flex min-w-0 flex-1 items-center gap-3 text-left"
                        onClick={() => setExpanded(expanded === record.id ? null : record.id)}
                      >
                        {expanded === record.id ? (
                          <ChevronDown className="size-3.5 shrink-0 text-ink-faint" />
                        ) : (
                          <ChevronRight className="size-3.5 shrink-0 text-ink-faint" />
                        )}
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-[13px] font-medium text-ink">
                            {record.serverName} · {record.database}
                            <span className="ml-2 text-[11px] font-normal text-ink-faint">
                              schema {record.schema}
                            </span>
                          </p>
                          <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                            {record.startedAt} · {humanSize(record.dumpSize)} ·{" "}
                            {record.targetName || t("backup.targetFallback")} · {record.target}
                          </p>
                        </div>
                        {statusBadge(record)}
                      </button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void handleDelete(record.id)}
                        title={t("backup.deleteRecord")}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                    {expanded === record.id && (
                      <pre className="max-h-80 overflow-auto border-t border-line bg-sunken px-4 py-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-ink-dim">
                        {record.error ? `${t("backup.errorPrefix", { error: record.error })}\n\n` : ""}
                        {record.log || t("backup.noLog")}
                      </pre>
                    )}
                  </div>
                ))}
              </Card>
            )}
          </section>
        </div>
      </div>

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
        open={confirmDeleteConfig}
        danger
        title={t("backup.configDeleteTitle")}
        confirmText={t("common.delete")}
        description={t("backup.configDeleteDescription")}
        onCancel={() => setConfirmDeleteConfig(false)}
        onConfirm={() => void handleDeleteConfig()}
      />
    </Page>
  );
}
