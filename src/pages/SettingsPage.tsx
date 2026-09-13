import { useEffect, useRef, useState } from "react";
import { FolderOpen, Plus, Save, Server, Settings2, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Field, Input, Page, SectionTitle, Select } from "../components/ui";
import { BackupTargetFromServerModal } from "../components/BackupTargetFromServerModal";
import { ThemeToggle } from "../components/ThemeToggle";
import { api } from "../lib/api";
import { applyLanguage, normalizeLanguagePreference } from "../lib/i18n";
import { useApp } from "../lib/store";
import type { BackupTarget, Settings } from "../lib/types";

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

  useEffect(() => {
    void api
      .getDataDir()
      .then(setDataDir)
      .catch(() => undefined);
  }, []);

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
      pagesHistoryLimit: Math.max(10, Number(source.pagesHistoryLimit) || 200),
      language: normalizeLanguagePreference(source.language),
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
              <label className="flex items-center gap-2.5 text-xs text-ink">
                <input
                  type="checkbox"
                  checked={draft.runScripts}
                  onChange={(event) => setDraft({ ...draft, runScripts: event.target.checked })}
                  className="size-3.5 accent-primary"
                />
                {t("settings.deploy.runScripts")}
              </label>
              <label className="flex items-center gap-2.5 text-xs text-ink">
                <input
                  type="checkbox"
                  checked={draft.keepRemoteArchive}
                  onChange={(event) =>
                    setDraft({ ...draft, keepRemoteArchive: event.target.checked })
                  }
                  className="size-3.5 accent-primary"
                />
                {t("settings.deploy.keepRemoteArchive")}
              </label>
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
    </Page>
  );
}
