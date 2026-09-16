import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { ConfigModalFooter } from "../../components/ConfigModalFooter";
import { ServerSelect } from "../../components/ServerSelect";
import { Field, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { BackupConfig, BackupRequest, DbBackupSource } from "../../lib/types";
import { maskUrlPassword } from "../../lib/utils";

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

/** 备份配置的新增 / 编辑弹窗：来源、目标与连接串都在这里配置。 */
export function BackupConfigModal({
  config,
  duplicate = false,
  onClose,
  onSaved,
  onRefresh,
}: {
  config: BackupConfig | null;
  /** 复制已有配置：保留全部参数，仅 id 为空，保存时新建一条（名称由调用方预填副本名）。 */
  duplicate?: boolean;
  onClose: () => void;
  onSaved: (saved: BackupConfig) => void;
  /** 「测试环境」会先保存当前表单，保存后需要父组件刷新列表（弹窗保持打开）。 */
  onRefresh: () => void;
}) {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const backupTargets = useApp((state) => state.backupTargets);
  const settings = useApp((state) => state.settings);
  const toast = useApp((state) => state.toast);

  const [name, setName] = useState(config?.name ?? "");
  const [serverId, setServerId] = useState(config?.serverId ?? servers[0]?.id ?? "");
  const [draft, setDraft] = useState<DbBackupSource>(() => config?.source ?? defaultSource());
  const [targetId, setTargetId] = useState(config?.targetId ?? "");
  const [overrideUrl, setOverrideUrl] = useState(config?.supabaseUrl ?? "");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);

  const selectedServer = servers.find((server) => server.id === serverId) ?? null;
  // 服务器列表晚于弹窗加载完成时补选第一台，避免表单一直无法提交。
  useEffect(() => {
    if (!serverId && servers.length > 0) setServerId(servers[0].id);
  }, [serverId, servers]);
  const targetById = (id: string | null | undefined) =>
    backupTargets.find((target) => target.id === id) ?? null;
  // 服务器 / 配置可能绑定了已被删除的目标：悬空 id 一律按未绑定处理。
  const validTargetId = backupTargets.some((target) => target.id === targetId) ? targetId : "";

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

  /** 校验并保存；通过时返回保存后的配置。 */
  async function save(): Promise<BackupConfig | null> {
    if (!name.trim()) {
      toast("error", t("backup.configNameRequired"));
      return null;
    }
    if (!serverId) {
      toast("error", t("backup.serverRequired"));
      return null;
    }
    if (!draft.database.trim()) {
      toast("error", t("backup.databaseRequired"));
      return null;
    }
    setSaving(true);
    try {
      // 目标列表尚未加载（接口失败/晚到）时无法解析绑定关系：
      // 若用户没动过下拉，保留配置里原有的 targetId，避免静默丢绑定。
      const originalTargetId = config?.targetId ?? "";
      const keepOriginalTarget =
        originalTargetId !== "" && backupTargets.length === 0 && targetId === originalTargetId;
      return await api.saveBackupConfig({
        id: config?.id ?? "",
        name: name.trim(),
        serverId,
        source: normalized(draft),
        targetId: keepOriginalTarget ? originalTargetId : validTargetId || null,
        supabaseUrl: overrideUrl.trim() || null,
      });
    } catch (error) {
      toast("error", String(error));
      return null;
    } finally {
      setSaving(false);
    }
  }

  async function handleSave() {
    const saved = await save();
    if (saved) onSaved(saved);
  }

  async function handleTest() {
    if (testing) return;
    setTesting(true);
    try {
      if (config) {
        // 已保存的配置：先保存当前表单再测试，否则后端会回退到配置里的旧目标。
        const saved = await save();
        if (!saved) return;
        // 表单已写盘：同步列表，避免关闭弹窗后仍显示旧名称 / 旧目标。
        onRefresh();
        const message = await api.testBackup({ serverId: saved.serverId, backupConfigId: saved.id });
        toast("success", message || t("backup.testPassed"));
        return;
      }
      const request: BackupRequest = {
        serverId,
        source: normalized(draft),
        targetId: validTargetId || null,
        supabaseUrl: overrideUrl.trim() || null,
      };
      const message = await api.testBackup(request);
      toast("success", message || t("backup.testPassed"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTesting(false);
    }
  }

  return (
    <Modal
      open
      onClose={saving ? () => undefined : onClose}
      title={
        duplicate
          ? t("backup.copyConfig")
          : config
            ? t("backup.editConfig")
            : t("backup.newConfig")
      }
      subtitle={t("backup.configModalSubtitle")}
      width="max-w-xl"
      footer={
        <ConfigModalFooter
          onClose={onClose}
          onSave={() => void handleSave()}
          onTest={() => void handleTest()}
          saving={saving}
          testing={testing}
          testDisabled={!serverId}
        />
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("backup.configName")} required>
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t("backup.configNamePlaceholder")}
            disabled={saving}
          />
        </Field>

        <Field label={t("backup.server")} required>
          <ServerSelect
            value={serverId}
            onChange={setServerId}
            servers={servers}
            disabled={saving}
            emptyText={t("backup.noServers")}
          />
        </Field>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("backup.mode")}>
            <Select
              value={draft.mode}
              onChange={(event) =>
                setDraft({ ...draft, mode: event.target.value as DbBackupSource["mode"] })
              }
              disabled={saving}
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
              disabled={saving}
            />
          </Field>
        </div>

        {draft.mode === "docker" && (
          <Field label={t("backup.container")} hint={t("backup.containerHint")} required>
            <Input
              value={draft.container}
              onChange={(event) => setDraft({ ...draft, container: event.target.value })}
              placeholder="postgres"
              disabled={saving}
            />
          </Field>
        )}

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("backup.database")} required>
            <Input
              value={draft.database}
              onChange={(event) => setDraft({ ...draft, database: event.target.value })}
              placeholder="app"
              disabled={saving}
            />
          </Field>
          <Field label={t("backup.username")} required>
            <Input
              value={draft.username}
              onChange={(event) => setDraft({ ...draft, username: event.target.value })}
              placeholder="postgres"
              disabled={saving}
            />
          </Field>
        </div>

        <Field label={t("backup.password")} hint={t("backup.passwordHint")}>
          <Input
            type="password"
            value={draft.password}
            onChange={(event) => setDraft({ ...draft, password: event.target.value })}
            placeholder={t("backup.passwordPlaceholder")}
            disabled={saving}
          />
        </Field>

        <div className="flex flex-col gap-4 border-t border-line pt-4">
          <Field label={t("backup.useTarget")} hint={t("backup.useTargetHint")}>
            <Select
              value={validTargetId}
              onChange={(event) => setTargetId(event.target.value)}
              disabled={saving}
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
              disabled={saving}
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
        </div>
      </div>
    </Modal>
  );
}
