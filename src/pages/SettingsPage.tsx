import { useEffect, useRef, useState } from "react";
import { FileJson, FolderOpen, Power, Plus, Save, Server, Settings2, Trash2, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  Button,
  Card,
  Checkbox,
  Field,
  Input,
  Modal,
  Page,
  SectionTitle,
  Select,
} from "../components/ui";
import { BackupTargetFromServerModal } from "../components/BackupTargetFromServerModal";
import { ThemeToggle } from "../components/ThemeToggle";
import { api } from "../lib/api";
import { applyLanguage, normalizeLanguagePreference } from "../lib/i18n";
import { FALLBACK_CANCEL_WINDOW_SECS, formatClockTime } from "../lib/shutdown";
import { useApp } from "../lib/store";
import type { BackupTarget, ImportPreview, Settings } from "../lib/types";

export default function SettingsPage() {
  const { t } = useTranslation();
  const settings = useApp((state) => state.settings);
  const setSettings = useApp((state) => state.setSettings);
  const backupTargets = useApp((state) => state.backupTargets);
  const servers = useApp((state) => state.servers);
  const refreshBackupTargets = useApp((state) => state.refreshBackupTargets);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const refreshServers = useApp((state) => state.refreshServers);
  const toast = useApp((state) => state.toast);
  const loadAll = useApp((state) => state.loadAll);
  const shutdownStatus = useApp((state) => state.shutdownStatus);
  const setShutdownStatus = useApp((state) => state.setShutdownStatus);
  const cancelShutdown = useApp((state) => state.cancelShutdown);

  const [dataDir, setDataDir] = useState("");
  const [draft, setDraft] = useState<Settings>(settings);
  const [targetDraft, setTargetDraft] = useState<BackupTarget[]>(backupTargets);
  const [fromServerOpen, setFromServerOpen] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [shutdownTime, setShutdownTime] = useState(settings.scheduledShutdownTime);
  const [shutdownDelay, setShutdownDelay] = useState(30);
  const [shutdownBusy, setShutdownBusy] = useState(false);
  // 定时关机设置即时保存：连续改开关 / 时间时按顺序提交，避免在途请求用旧快照互相覆盖。
  const shutdownQueue = useRef<Promise<void>>(Promise.resolve());
  // 配置导入：先只读预览、用户确认后才落盘。导出文件必然已脱敏，直接应用会冲掉本机凭据。
  const [importDraft, setImportDraft] = useState<{
    text: string;
    preview: ImportPreview;
  } | null>(null);
  const [importBusy, setImportBusy] = useState(false);
  // 开机自启动当前仅 Windows 支持：其它平台不展示该开关。
  const isWindows =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

  useEffect(() => {
    void api
      .getDataDir()
      .then(setDataDir)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    void api
      .getAutostart()
      .then(setAutostart)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    setShutdownTime(settings.scheduledShutdownTime);
  }, [settings.scheduledShutdownTime]);

  /** 定时关机设置即时保存（摘要行读的就是这份已保存状态）。 */
  async function handleSaveSchedule(patch: Partial<Settings>) {
    const run = shutdownQueue.current.then(async () => {
      const current = useApp.getState().settings;
      const saved = await api.saveSettings({ ...current, ...patch });
      setSettings(saved);
    });
    shutdownQueue.current = run.catch(() => undefined);
    try {
      await run;
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleScheduleShutdown() {
    setShutdownBusy(true);
    try {
      setShutdownStatus(await api.scheduleShutdown(shutdownDelay));
      toast("success", t("settings.shutdown.started", { minutes: shutdownDelay }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setShutdownBusy(false);
    }
  }

  async function handleAutostart(enabled: boolean) {
    setAutostartBusy(true);
    try {
      await api.setAutostart(enabled);
      setAutostart(enabled);
      toast(
        "success",
        enabled ? t("settings.automation.autostartOn") : t("settings.automation.autostartOff"),
      );
    } catch (error) {
      toast("error", String(error));
    } finally {
      setAutostartBusy(false);
    }
  }

  async function handleExportConfig() {
    try {
      const json = await api.exportConfig();
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `deploycode-config-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast("success", t("settings.configBackup.exported"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  /** 选择文件后只做预览，不落盘。 */
  async function handleImportConfig(file: File) {
    try {
      const text = await file.text();
      const preview = await api.previewConfigImport(text);
      setImportDraft({ text, preview });
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleApplyImport() {
    if (!importDraft) return;
    setImportBusy(true);
    try {
      await api.importConfig(importDraft.text);
      setImportDraft(null);
      // 导入会改动服务器 / 仓库 / 各类配置，整份重载才能保证界面与落盘一致。
      await loadAll();
      toast("success", t("settings.configBackup.imported"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setImportBusy(false);
    }
  }

  const previousSettings = useRef(settings);

  // 只把真正发生变化的字段同步进草稿，避免保存目标等操作覆盖页面上其他未保存的编辑。
  useEffect(() => {
    const previous = previousSettings.current;
    previousSettings.current = settings;
    setDraft((current) => {
      const next: Settings = { ...current };
      const patch = next as Record<keyof Settings, Settings[keyof Settings]>;
      for (const key of Object.keys(settings) as (keyof Settings)[]) {
        if (settings[key] !== previous[key]) patch[key] = settings[key];
      }
      return next;
    });
  }, [settings]);

  useEffect(() => {
    setTargetDraft(backupTargets);
  }, [backupTargets]);

  /** 保存到后端的规范化：数值字段限幅、空值用默认值兜底。 */
  function normalizedSettings(source: Settings): Settings {
    // 0 是「不自动清理」的有效值，不能用 `|| 3` 兜底；只有填不进数字的才回落默认。
    const bundleKeep = Number(source.containerBundleKeep);
    return {
      ...source,
      scriptDir: source.scriptDir.trim() || "docker",
      connectTimeoutSecs: Math.max(3, Number(source.connectTimeoutSecs) || 15),
      scriptTimeoutSecs: Math.max(10, Number(source.scriptTimeoutSecs) || 1800),
      historyLimit: Math.max(20, Number(source.historyLimit) || 500),
      supabaseUrl: source.supabaseUrl.trim(),
      defaultBackupTargetId: source.defaultBackupTargetId || null,
      backupHistoryLimit: Math.max(10, Number(source.backupHistoryLimit) || 200),
      backupTimeoutSecs: Math.max(60, Number(source.backupTimeoutSecs) || 3600),
      cloudflareApiToken: source.cloudflareApiToken.trim(),
      cloudflareAccountId: source.cloudflareAccountId.trim(),
      githubToken: source.githubToken.trim(),
      cronjobApiKey: source.cronjobApiKey.trim(),
      pagesHistoryLimit: Math.max(10, Number(source.pagesHistoryLimit) || 200),
      containerHistoryLimit: Math.max(10, Number(source.containerHistoryLimit) || 200),
      containerTimeoutSecs: Math.max(300, Number(source.containerTimeoutSecs) || 7200),
      containerBundleKeep:
        Number.isFinite(bundleKeep) && bundleKeep >= 0 ? Math.min(999, Math.trunc(bundleKeep)) : 3,
      language: normalizeLanguagePreference(source.language),
      releaseKeep: Math.min(50, Math.max(1, Number(source.releaseKeep) || 5)),
      scheduledBackupTime: /^\d{1,2}:\d{2}$/.test(source.scheduledBackupTime.trim())
        ? source.scheduledBackupTime.trim()
        : "03:00",
      scheduledBackupConfigId: source.scheduledBackupConfigId || null,
      scheduledShutdownTime: /^\d{1,2}:\d{2}$/.test(source.scheduledShutdownTime.trim())
        ? source.scheduledShutdownTime.trim()
        : "04:00",
    };
  }

  async function handleSaveSettings() {
    try {
      const saved = await api.saveSettings(normalizedSettings(draft));
      setSettings(saved);
      toast("success", t("settings.saved"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleSaveTargets() {
    try {
      const saved = await api.saveBackupTargets(targetDraft);
      setTargetDraft(saved);
      await Promise.all([refreshBackupTargets(), refreshServers(), refreshBackupConfigs()]);
      // 后端可能清理指向已删除目标的默认设置，需要同步最新 settings。
      const fresh = await api.getSettings();
      setSettings(fresh);
      toast("success", t("settings.targetsSaved"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  /** 由「从服务器添加」生成的目标先进入草稿，点「保存目标」后持久化。 */
  function handleAddTargetFromServer(target: BackupTarget) {
    setTargetDraft((current) => [...current, target]);
  }

  /** 把旧版单连接串转成正式备份目标，并立即持久化，避免只改本地状态导致数据丢失。 */
  async function handleConvertLegacy(url: string) {
    const next = [...targetDraft, { id: "", name: t("settings.backupTargets.legacyName"), url }];
    try {
      const saved = await api.saveBackupTargets(next);
      setTargetDraft(saved);
      await refreshBackupTargets();
      const savedSettings = await api.saveSettings(
        normalizedSettings({ ...draft, supabaseUrl: "" }),
      );
      setSettings(savedSettings);
      toast("success", t("settings.legacyConverted"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  return (
    <Page title={t("settings.title")} subtitle={t("settings.subtitle")}>
      <div className="flex max-w-4xl flex-col gap-8">
        <section>
          <SectionTitle
            title={t("settings.deploy.title")}
            description={t("settings.deploy.description")}
            actions={
              <Button
                icon={<Settings2 className="size-4" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.deploy.save")}
              </Button>
            }
          />
          <Card className="p-5">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label={t("settings.deploy.scriptDir")} hint={t("settings.deploy.scriptDirHint")}>
                <Input
                  value={draft.scriptDir}
                  onChange={(event) => setDraft({ ...draft, scriptDir: event.target.value })}
                  placeholder="docker"
                />
              </Field>
              <Field label={t("settings.deploy.historyLimit")}>
                <Input
                  type="number"
                  value={draft.historyLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, historyLimit: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label={t("settings.deploy.connectTimeout")}>
                <Input
                  type="number"
                  value={draft.connectTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, connectTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label={t("settings.deploy.scriptTimeout")}>
                <Input
                  type="number"
                  value={draft.scriptTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, scriptTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
            </div>

            <div className="mt-5 flex flex-col gap-3 border-t border-line pt-5">
              <Checkbox
                checked={draft.runScripts}
                onChange={(checked) => setDraft({ ...draft, runScripts: checked })}
              >
                {t("settings.deploy.runScripts")}
              </Checkbox>
              <Checkbox
                checked={draft.keepRemoteArchive}
                onChange={(checked) => setDraft({ ...draft, keepRemoteArchive: checked })}
              >
                {t("settings.deploy.keepRemoteArchive")}
              </Checkbox>
              <Checkbox
                checked={draft.atomicRelease}
                onChange={(checked) => setDraft({ ...draft, atomicRelease: checked })}
              >
                {t("settings.deploy.atomicRelease")}
              </Checkbox>
              {draft.atomicRelease && (
                <div className="flex items-center gap-3 pl-6">
                  <span className="text-[11px] text-ink-faint">
                    {t("settings.deploy.releaseKeep")}
                  </span>
                  <div className="w-24">
                    <Input
                      type="number"
                      value={draft.releaseKeep}
                      onChange={(event) =>
                        setDraft({ ...draft, releaseKeep: Number(event.target.value) })
                      }
                    />
                  </div>
                </div>
              )}
              <p className="pl-6 text-[11px] leading-relaxed text-ink-faint">
                {t("settings.deploy.atomicReleaseHint")}
              </p>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.backupTargets.title")}
            description={t("settings.backupTargets.description")}
            actions={
              <Button
                icon={<Save className="size-4" />}
                onClick={() => void handleSaveTargets()}
              >
                {t("settings.backupTargets.save")}
              </Button>
            }
          />
          <Card className="flex flex-col gap-3 p-5">
            {targetDraft.length === 0 && (
              <p className="text-xs text-ink-faint">{t("settings.backupTargets.empty")}</p>
            )}
            {targetDraft.map((target, index) => (
              <div
                key={target.id || `new-${index}`}
                className="grid grid-cols-[150px_minmax(0,1fr)_auto] items-center gap-2"
              >
                <Input
                  value={target.name}
                  placeholder={t("settings.backupTargets.namePlaceholder")}
                  onChange={(event) => {
                    const next = [...targetDraft];
                    next[index] = { ...target, name: event.target.value };
                    setTargetDraft(next);
                  }}
                />
                <Input
                  value={target.url}
                  placeholder={t("settings.backupTargets.urlPlaceholder")}
                  onChange={(event) => {
                    const next = [...targetDraft];
                    next[index] = { ...target, url: event.target.value };
                    setTargetDraft(next);
                  }}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  title={t("settings.backupTargets.deleteTarget")}
                  onClick={() => setTargetDraft(targetDraft.filter((_, i) => i !== index))}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            ))}
            <div className="flex items-center gap-2 border-t border-line pt-3">
              <Button
                variant="secondary"
                size="sm"
                icon={<Plus className="size-3.5" />}
                onClick={() =>
                  setTargetDraft([
                    ...targetDraft,
                    { id: "", name: "", url: "" },
                  ])
                }
              >
                {t("settings.backupTargets.addTarget")}
              </Button>
              <Button
                variant="secondary"
                size="sm"
                icon={<Server className="size-3.5" />}
                disabled={servers.length === 0}
                onClick={() => setFromServerOpen(true)}
              >
                {t("settings.backupTargets.fromServer")}
              </Button>
            </div>

            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field
                label={t("settings.backupTargets.defaultTarget")}
                hint={t("settings.backupTargets.defaultTargetHint")}
              >
                <Select
                  value={draft.defaultBackupTargetId ?? ""}
                  onChange={(event) =>
                    setDraft({
                      ...draft,
                      defaultBackupTargetId: event.target.value || null,
                    })
                  }
                >
                  <option value="">{t("settings.backupTargets.none")}</option>
                  {backupTargets.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label={t("settings.backupTargets.historyLimit")}>
                <Input
                  type="number"
                  value={draft.backupHistoryLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, backupHistoryLimit: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label={t("settings.backupTargets.timeout")}>
                <Input
                  type="number"
                  value={draft.backupTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, backupTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
            </div>

            <div className="flex items-center justify-between gap-3 border-t border-line pt-3">
              {draft.supabaseUrl.trim() ? (
                <div className="flex min-w-0 items-center gap-2 text-[11px] text-warn">
                  <span className="min-w-0">{t("settings.backupTargets.legacy")}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void handleConvertLegacy(draft.supabaseUrl.trim())}
                  >
                    {t("settings.backupTargets.convertLegacy")}
                  </Button>
                </div>
              ) : (
                <span className="text-[11px] text-ink-faint">
                  {t("settings.backupTargets.fullOverwrite")}
                </span>
              )}
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.backupTargets.saveParams")}
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.containers.title")}
            description={t("settings.containers.description")}
          />
          <Card className="flex flex-col gap-3 p-5">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label={t("settings.containers.historyLimit")}>
                <Input
                  type="number"
                  value={draft.containerHistoryLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, containerHistoryLimit: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label={t("settings.containers.timeout")}>
                <Input
                  type="number"
                  value={draft.containerTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, containerTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
            </div>
            <Field
              label={t("settings.containers.bundleKeep")}
              hint={t("settings.containers.bundleKeepHint")}
            >
              <Input
                className="max-w-40"
                type="number"
                min={0}
                max={999}
                value={draft.containerBundleKeep}
                onChange={(event) =>
                  setDraft({ ...draft, containerBundleKeep: Number(event.target.value) })
                }
              />
            </Field>
            <div className="flex justify-end border-t border-line pt-3">
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.backupTargets.saveParams")}
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.cloudflare.title")}
            description={t("settings.cloudflare.description")}
          />
          <Card className="p-5">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field
                label={t("settings.cloudflare.apiToken")}
                hint={t("settings.cloudflare.apiTokenHint")}
              >
                <Input
                  type="password"
                  value={draft.cloudflareApiToken}
                  onChange={(event) =>
                    setDraft({ ...draft, cloudflareApiToken: event.target.value })
                  }
                  placeholder="Cloudflare API Token"
                />
              </Field>
              <Field label={t("settings.cloudflare.accountId")}>
                <Input
                  value={draft.cloudflareAccountId}
                  onChange={(event) =>
                    setDraft({ ...draft, cloudflareAccountId: event.target.value })
                  }
                  placeholder="Cloudflare Account ID"
                />
              </Field>
              <Field label={t("settings.cloudflare.historyLimit")}>
                <Input
                  type="number"
                  value={draft.pagesHistoryLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, pagesHistoryLimit: Number(event.target.value) })
                  }
                />
              </Field>
            </div>
            <div className="mt-4 flex justify-end border-t border-line pt-4">
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.cloudflare.save")}
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.github.title")}
            description={t("settings.github.description")}
          />
          <Card className="p-5">
            <Field
              label={t("settings.github.token")}
              hint={t("settings.github.tokenHint")}
            >
              <Input
                type="password"
                value={draft.githubToken}
                onChange={(event) => setDraft({ ...draft, githubToken: event.target.value })}
                placeholder="ghp_... / github_pat_..."
              />
            </Field>
            <div className="mt-4 flex justify-end border-t border-line pt-4">
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.github.save")}
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.cronjob.title")}
            description={t("settings.cronjob.description")}
          />
          <Card className="p-5">
            <Field
              label={t("settings.cronjob.apiKey")}
              hint={t("settings.cronjob.apiKeyHint")}
            >
              <Input
                type="password"
                value={draft.cronjobApiKey}
                onChange={(event) => setDraft({ ...draft, cronjobApiKey: event.target.value })}
                placeholder="cron-job.org API Key"
              />
            </Field>
            <div className="mt-4 flex justify-end border-t border-line pt-4">
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.cronjob.save")}
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.appearance.title")}
            description={t("settings.appearance.description")}
            actions={
              <Button
                icon={<Settings2 className="size-4" />}
                onClick={() => void handleSaveSettings()}
              >
                {t("settings.deploy.save")}
              </Button>
            }
          />
          <Card className="flex flex-col p-4">
            <div className="flex items-center gap-4">
              <span className="min-w-0 flex-1 text-[13px] text-ink-dim">
                {t("settings.appearance.theme")}
              </span>
              <ThemeToggle className="w-72 shrink-0" />
            </div>
            <div className="mt-4 flex items-center gap-4 border-t border-line pt-4">
              <span className="min-w-0 flex-1 text-[13px] text-ink-dim">
                {t("settings.appearance.language")}
              </span>
              <div className="w-72 shrink-0">
                <Select
                  value={normalizeLanguagePreference(draft.language)}
                  onChange={(event) => {
                    const next = { ...draft, language: event.target.value };
                    setDraft(next);
                    applyLanguage(next.language);
                  }}
                >
                  <option value="">{t("language.system")}</option>
                  <option value="zh-CN">{t("language.zhCN")}</option>
                  <option value="en-US">{t("language.enUS")}</option>
                </Select>
              </div>
            </div>
          </Card>
        </section>

        {isWindows && (
          <section>
            <SectionTitle
              title={t("settings.automation.title")}
              description={t("settings.automation.description")}
            />
            <Card className="flex flex-col gap-5 p-5">
              <div>
                <Checkbox
                  checked={autostart}
                  disabled={autostartBusy}
                  onChange={(checked) => void handleAutostart(checked)}
                >
                  {t("settings.automation.autostart")}
                </Checkbox>
                <p className="mt-2 pl-6 text-[11px] leading-relaxed text-ink-faint">
                  {t("settings.automation.autostartHint")}
                </p>
              </div>

              <div className="border-t border-line pt-5">
                <Checkbox
                  checked={settings.scheduledShutdownEnabled}
                  onChange={(checked) =>
                    void handleSaveSchedule({ scheduledShutdownEnabled: checked })
                  }
                >
                  {t("settings.shutdown.dailyEnable")}
                </Checkbox>
                <div className="mt-3 grid grid-cols-1 gap-4 md:grid-cols-2">
                  <Field label={t("settings.shutdown.time")}>
                    <Input
                      type="time"
                      value={shutdownTime}
                      onChange={(event) => {
                        setShutdownTime(event.target.value);
                        // 时间选完整即保存，避免改完直接关窗导致修改丢失。
                        if (/^\d{1,2}:\d{2}$/.test(event.target.value)) {
                          void handleSaveSchedule({ scheduledShutdownTime: event.target.value });
                        }
                      }}
                      onBlur={() => {
                        // 清空或非法值时回填已保存的时间，避免界面与摘要不一致。
                        if (!/^\d{1,2}:\d{2}$/.test(shutdownTime)) {
                          setShutdownTime(settings.scheduledShutdownTime);
                        }
                      }}
                    />
                  </Field>
                  {!autostart && settings.scheduledShutdownEnabled && (
                    <Field label={t("settings.shutdown.autostartLabel")}>
                      <Button
                        variant="secondary"
                        size="sm"
                        className="justify-start"
                        disabled={autostartBusy}
                        onClick={() => void handleAutostart(true)}
                      >
                        {t("settings.shutdown.enableAutostart")}
                      </Button>
                    </Field>
                  )}
                </div>
                <p className="mt-2 text-[11px] leading-relaxed text-ink-faint">
                  {settings.scheduledShutdownEnabled
                    ? t("settings.shutdown.summary", {
                        time: settings.scheduledShutdownTime,
                        secs: shutdownStatus?.cancelWindowSecs ?? FALLBACK_CANCEL_WINDOW_SECS,
                      })
                    : t("settings.shutdown.dailyDisabled")}
                </p>
              </div>

              <div className="border-t border-line pt-5">
                <div className="flex flex-wrap items-end gap-3">
                  <Field
                    label={t("settings.shutdown.countdownLabel")}
                    hint={t("settings.shutdown.countdownHint")}
                  >
                    <div className="w-28">
                      <Input
                        type="number"
                        min={1}
                        max={1440}
                        value={shutdownDelay}
                        onChange={(event) => setShutdownDelay(Number(event.target.value))}
                      />
                    </div>
                  </Field>
                  <Button
                    variant="secondary"
                    icon={<Power className="size-4" />}
                    loading={shutdownBusy}
                    disabled={!(shutdownDelay >= 1 && shutdownDelay <= 1440)}
                    onClick={() => void handleScheduleShutdown()}
                  >
                    {t("settings.shutdown.start")}
                  </Button>
                </div>
                {shutdownStatus?.pending && (
                  <div className="mt-3 flex flex-wrap items-center gap-3 rounded-md border border-warn/35 bg-warn-soft px-3 py-2">
                    <span className="min-w-0 flex-1 text-[12px] text-ink">
                      {t("settings.shutdown.armed", {
                        time: formatClockTime(shutdownStatus.pending.atMs),
                        source:
                          shutdownStatus.pending.source === "scheduled"
                            ? t("settings.shutdown.sourceScheduled")
                            : t("settings.shutdown.sourceManual"),
                      })}
                    </span>
                    <Button
                      variant="danger"
                      size="sm"
                      onClick={() => void cancelShutdown()}
                    >
                      {t("settings.shutdown.cancel")}
                    </Button>
                  </div>
                )}
                <p className="mt-2 text-[11px] leading-relaxed text-ink-faint">
                  {t("settings.shutdown.countdownDescription")}
                </p>
              </div>
            </Card>
          </section>
        )}

        <section>
          <SectionTitle
            title={t("settings.configBackup.title")}
            description={t("settings.configBackup.description")}
          />
          <Card className="flex items-center gap-3 p-5">
            <div className="flex items-center gap-2">
              <Button
                variant="secondary"
                icon={<FileJson className="size-4" />}
                onClick={handleExportConfig}
              >
                {t("settings.configBackup.export")}
              </Button>
              <label className="cursor-pointer">
                <input
                  type="file"
                  accept=".json"
                  className="hidden"
                  onChange={(e) => {
                    const file = e.target.files?.[0];
                    if (file) {
                      void handleImportConfig(file);
                      e.target.value = "";
                    }
                  }}
                />
                <Button variant="secondary" icon={<Upload className="size-4" />}>
                  {t("settings.configBackup.import")}
                </Button>
              </label>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("settings.dataDir.title")}
            description={t("settings.dataDir.description")}
          />
          <Card className="flex items-center gap-3 p-5">
            <p className="min-w-0 flex-1 truncate font-mono text-xs text-ink-dim" title={dataDir}>
              {dataDir || t("common.loading")}
            </p>
            <Button
              variant="secondary"
              icon={<FolderOpen className="size-4" />}
              disabled={!dataDir}
              onClick={() => void api.revealPath(dataDir).catch((e) => toast("error", String(e)))}
            >
              {t("settings.dataDir.openDir")}
            </Button>
          </Card>
        </section>
      </div>

      <BackupTargetFromServerModal
        open={fromServerOpen}
        servers={servers}
        onClose={() => setFromServerOpen(false)}
        onAdd={handleAddTargetFromServer}
      />

      {importDraft && (
        <Modal
          open
          onClose={() => setImportDraft(null)}
          title={t("settings.configBackup.confirmImport")}
          subtitle={t("settings.configBackup.preview.exportedAt", {
            time: importDraft.preview.exportedAt,
          })}
          footer={
            <>
              <Button
                variant="secondary"
                disabled={importBusy}
                onClick={() => setImportDraft(null)}
              >
                {t("common.cancel")}
              </Button>
              <Button
                icon={<Upload className="size-4" />}
                loading={importBusy}
                onClick={() => void handleApplyImport()}
              >
                {t("settings.configBackup.import")}
              </Button>
            </>
          }
        >
          <div className="flex flex-col gap-3">
            {importRows(importDraft.preview).length === 0 ? (
              <p className="text-xs text-ink-dim">{t("settings.configBackup.preview.nothing")}</p>
            ) : (
              <table className="w-full text-left text-xs">
                <thead>
                  <tr className="border-b border-line text-[11px] uppercase tracking-wide text-ink-faint">
                    <th className="py-2 font-medium">{t("settings.configBackup.preview.kind")}</th>
                    <th className="py-2 text-right font-medium">
                      {t("settings.configBackup.preview.added")}
                    </th>
                    <th className="py-2 text-right font-medium">
                      {t("settings.configBackup.preview.overwritten")}
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {importRows(importDraft.preview).map((row) => (
                    <tr key={row.key}>
                      <td className="py-2 text-ink">{t(`settings.configBackup.kinds.${row.key}`)}</td>
                      <td className="py-2 text-right tabular-nums text-ink-dim">{row.counts.added}</td>
                      <td className="py-2 text-right tabular-nums text-ink-dim">
                        {row.counts.overwritten}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {importDraft.preview.keptLocalSecrets > 0 && (
              <p className="rounded-md border border-warn/30 bg-warn-soft px-3 py-2 text-[11px] leading-relaxed text-warn">
                {t("settings.configBackup.preview.keptSecrets", {
                  count: importDraft.preview.keptLocalSecrets,
                })}
              </p>
            )}
            <p className="text-[11px] leading-relaxed text-ink-faint">
              {t("settings.configBackup.preview.note")}
            </p>
          </div>
        </Modal>
      )}
    </Page>
  );
}

/** 预览表格里只显示真正有变动的配置类别，全零的行不留占位。 */
function importRows(preview: ImportPreview) {
  return (
    [
      { key: "servers", counts: preview.servers },
      { key: "repos", counts: preview.repos },
      { key: "backupTargets", counts: preview.backupTargets },
      { key: "deployConfigs", counts: preview.deployConfigs },
      { key: "backupConfigs", counts: preview.backupConfigs },
      { key: "pagesConfigs", counts: preview.pagesConfigs },
      { key: "containerConfigs", counts: preview.containerConfigs },
    ] as const
  ).filter((row) => row.counts.added + row.counts.overwritten > 0);
}
