import { useEffect, useState } from "react";
import { Plus, Save, Server, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { BackupTargetFromServerModal } from "../../components/BackupTargetFromServerModal";
import { Button, Card, Field, Input, SectionTitle, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { BackupTarget, Settings } from "../../lib/types";

const BACKUP_TARGET_KEYS: readonly (keyof Settings)[] = [
  "supabaseUrl",
  "defaultBackupTargetId",
  "backupHistoryLimit",
  "backupTimeoutSecs",
];

/**
 * 数据库备份目标库：它是备份模块自己的实体列表，不是全局设置，
 * 所以挂在备份页 —— 备份配置选的就是这里登记的目标。
 */
export function BackupTargetsCard() {
  const { t } = useTranslation();
  const { settings, draft, setDraft, save } = useSettingsForm(BACKUP_TARGET_KEYS);
  const backupTargets = useApp((state) => state.backupTargets);
  const servers = useApp((state) => state.servers);
  const refreshBackupTargets = useApp((state) => state.refreshBackupTargets);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const refreshServers = useApp((state) => state.refreshServers);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);

  const [targetDraft, setTargetDraft] = useState(backupTargets);
  const [fromServerOpen, setFromServerOpen] = useState(false);

  useEffect(() => {
    setTargetDraft(backupTargets);
  }, [backupTargets]);

  async function handleSaveTargets() {
    try {
      const saved = await api.saveBackupTargets(targetDraft);
      setTargetDraft(saved);
      await Promise.all([refreshBackupTargets(), refreshServers(), refreshBackupConfigs()]);
      // 后端可能清理指向已删除目标的默认设置，需要同步最新 settings。
      setSettings(await api.getSettings());
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
      await save({ supabaseUrl: "" }, { notify: false });
      toast("success", t("settings.legacyConverted"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  return (
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
          {settings.supabaseUrl.trim() ? (
            <div className="flex min-w-0 items-center gap-2 text-[11px] text-warn">
              <span className="min-w-0">{t("settings.backupTargets.legacy")}</span>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void handleConvertLegacy(settings.supabaseUrl.trim())}
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
            onClick={() => void save()}
          >
            {t("settings.saveParams")}
          </Button>
        </div>
      </Card>

      <BackupTargetFromServerModal
        open={fromServerOpen}
        servers={servers}
        onClose={() => setFromServerOpen(false)}
        onAdd={handleAddTargetFromServer}
      />
    </section>
  );
}
