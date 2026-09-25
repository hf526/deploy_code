import { useEffect, useRef, useState } from "react";
import { FileJson, FolderOpen, Plus, Save, Server, Settings2, Trash2, Upload } from "lucide-react";
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
import { useApp } from "../lib/store";
import type { BackupTarget, ImportSummary, Settings } from "../lib/types";

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

  const [dataDir, setDataDir] = useState("");
  const [draft, setDraft] = useState<Settings>(settings);
  const [targetDraft, setTargetDraft] = useState<BackupTarget[]>(backupTargets);
  const [fromServerOpen, setFromServerOpen] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  // 配置备份弹窗
  const [importModalOpen, setImportModalOpen] = useState(false);
  const [importSummary, setImportSummary] = useState<ImportSummary | null>(null);
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

  async function handleImportConfig(file: File) {
    try {
      const text = await file.text();
      const summary = await api.importConfig(text);
      setImportSummary(summary);
      setImportModalOpen(true);
      toast("success", t("settings.configBackup.import"));
    } catch (error) {
      toast("error", String(error));
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
      language: normalizeLanguagePreference(source.language),
      releaseKeep: Math.min(50, Math.max(1, Number(source.releaseKeep) || 5)),
      scheduledBackupTime: /^\d{1,2}:\d{2}$/.test(source.scheduledBackupTime.trim())
        ? source.scheduledBackupTime.trim()
        : "03:00",
      scheduledBackupConfigId: source.scheduledBackupConfigId || null,
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
            <Card className="flex flex-col p-4">
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

      {importSummary && (
        <Modal
          title={t("settings.configBackup.confirmImport")}
          open={importModalOpen}
          onClose={() => {
            setImportModalOpen(false);
            setImportSummary(null);
          }}
        >
          <div className="space-y-3">
            <p className="text-sm text-ink-dim">
              {t("settings.configBackup.summary.title")}
            </p>
            <div className="grid grid-cols-2 gap-2 text-sm">
              <p>{t("settings.configBackup.summary.servers", { count: importSummary.serversImported })}</p>
              <p>{t("settings.configBackup.summary.repos", { count: importSummary.reposImported })}</p>
              <p>{t("settings.configBackup.summary.backupTargets", { count: importSummary.backupTargetsImported })}</p>
              <p>{t("settings.configBackup.summary.deployConfigs", { count: importSummary.deployConfigsImported })}</p>
              <p>{t("settings.configBackup.summary.backupConfigs", { count: importSummary.backupConfigsImported })}</p>
              <p>{t("settings.configBackup.summary.pagesConfigs", { count: importSummary.pagesConfigsImported })}</p>
            </div>
            <p className="text-xs text-ink-dim">
              ⚠️ 敏感信息（密码/密钥）已脱敏，请手动补充后再使用。
            </p>
            <div className="flex justify-end gap-2 pt-4">
              <Button
                variant="secondary"
                onClick={() => {
                  setImportModalOpen(false);
                  setImportSummary(null);
                }}
              >
                {t("common.close")}
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </Page>
  );
}
