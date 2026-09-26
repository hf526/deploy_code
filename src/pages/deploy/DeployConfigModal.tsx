import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  ChevronDown,
  ChevronUp,
  CircleDot,
  FileUp,
  Plus,
  X,
} from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { ConfigModalFooter } from "../../components/ConfigModalFooter";
import { SearchSelect } from "../../components/SearchSelect";
import { ServerCheckList } from "../../components/ServerCheckList";
import { Button, Checkbox, Field, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { DeployConfig, EnvFileConfig, ResolvedRev } from "../../lib/types";
import { cn, inferEnvRemotePath } from "../../lib/utils";
import { useRepoBranches } from "./useRepoBranches";

export interface DeployPrefill {
  repoId?: string;
  rev?: string;
}

function emptyConfig(): DeployConfig {
  return {
    id: "",
    name: "",
    repoId: "",
    serverIds: [],
    targetDir: "",
    rev: "",
    runScripts: true,
    scriptDir: "docker",
    scripts: [],
    uploadEnv: true,
    createdAt: "",
  };
}

function basename(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] ?? path;
}

/**
 * 服务器部署配置的新增 / 编辑弹窗。
 * 环境文件属于仓库配置，在弹窗内先作为草稿编辑，保存配置时一并写回。
 */
export function DeployConfigModal({
  initial,
  prefill,
  duplicate = false,
  onClose,
  onSaved,
  onRefresh,
}: {
  initial: DeployConfig | null;
  prefill: DeployPrefill | null;
  /** 复制已有配置：保留全部参数，仅 id 为空，保存时新建一条（名称由调用方预填副本名）。 */
  duplicate?: boolean;
  onClose: () => void;
  onSaved: (config: DeployConfig) => void;
  /** 配置已写盘但环境文件保存失败时通知父组件刷新列表（弹窗保持打开）。 */
  onRefresh: () => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const settings = useApp((state) => state.settings);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const toast = useApp((state) => state.toast);

  // 新建时用于套用默认值的仓库：优先深链带入，其次列表第一个。
  // 仓库列表可能晚于弹窗到达，此时先记录 id，等列表就绪后由下方 effect 补水合。
  const initialRepoId = initial?.repoId ?? prefill?.repoId ?? repos[0]?.id ?? "";
  const initialRepo = repos.find((repo) => repo.id === initialRepoId);
  const [draft, setDraft] = useState<DeployConfig>(() => {
    if (initial) return { ...initial, scripts: [...initial.scripts] };
    return {
      ...emptyConfig(),
      repoId: initialRepoId,
      rev: prefill?.rev ?? initialRepo?.currentBranch ?? "",
      serverIds: initialRepo?.defaultServerId ? [initialRepo.defaultServerId] : [],
      targetDir: initialRepo?.defaultTargetDir ?? "",
      runScripts: settings.runScripts,
      scriptDir: settings.scriptDir,
      uploadEnv: (initialRepo?.envFiles ?? []).length > 0,
    };
  });
  const [envFiles, setEnvFiles] = useState<EnvFileConfig[]>(() => initialRepo?.envFiles ?? []);
  // 只有从仓库配置读到过环境文件才允许写回：否则空草稿会把仓库里已保存的文件清掉。
  const [envFilesHydrated, setEnvFilesHydrated] = useState(() => initialRepo !== undefined);
  const [customRev, setCustomRev] = useState(() => !!(initial?.rev || prefill?.rev));
  const [scriptOptions, setScriptOptions] = useState<string[]>([]);
  const [resolved, setResolved] = useState<ResolvedRev | null>(null);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [saving, setSaving] = useState(false);

  const { branches, branchChoices, loaded: branchesLoaded } = useRepoBranches(
    draft.repoId,
    (error) => toast("error", String(error)),
  );

  // 空仓库（还没有任何提交）没有可解析的版本：默认打包当前工作区，
  // 否则仓库默认分支名会解析失败并卡住保存（与旧版部署页的兜底一致）。
  useEffect(() => {
    if (!branchesLoaded || customRev) return;
    if (branches.length === 0 && draft.rev.trim()) {
      setDraft((current) => ({ ...current, rev: "" }));
    }
  }, [branchesLoaded, branches.length, customRev, draft.rev]);

  // 深链带入且仓库已解析时 rev 已在初始化中应用；否则暂存 {仓库, 版本}，
  // 等仓库水合时只在同一个仓库上套用，避免残留值被套到之后切换的仓库。
  const pendingRev = useRef<{ repoId: string; rev: string } | null>(
    initialRepo || !prefill?.rev ? null : { repoId: initialRepoId, rev: prefill.rev },
  );
  // 编辑已有配置时不套用仓库默认值，避免覆盖保存过的参数；仅切换仓库时才重新套用。
  // 初始仓库尚未解析时保持空值，等仓库列表就绪后再水合一次。
  const appliedRepo = useRef(initialRepo ? initialRepo.id : "");
  const editingRepoId = initial?.repoId ?? null;

  useEffect(() => {
    if (repos.length === 0) return;
    const repoId = draft.repoId || repos[0].id;
    if (appliedRepo.current === repoId) return;
    const repo = repos.find((item) => item.id === repoId);
    if (!repo) {
      // 深链指向的仓库已不存在：丢掉暂存的版本，避免套用到之后选择的仓库。
      pendingRev.current = null;
      return;
    }
    const files = repo.envFiles ?? [];
    appliedRepo.current = repoId;
    // 切回原编辑仓库（含仓库列表晚到的情况）：只恢复仓库相关字段，避免把
    // server/targetDir/rev/scripts 重置成仓库默认值而在保存时覆盖原配置；
    // 名称等用户已改的内容保持不变。
    if (editingRepoId !== null && editingRepoId === repoId && initial) {
      setDraft((current) => ({
        ...current,
        rev: initial.rev,
        serverIds: [...initial.serverIds],
        targetDir: initial.targetDir,
        scripts: [...initial.scripts],
        uploadEnv: initial.uploadEnv,
      }));
      setCustomRev(Boolean(initial.rev));
      setEnvFiles(files);
      setEnvFilesHydrated(true);
      return;
    }
    const pending = pendingRev.current;
    pendingRev.current = null;
    const preservedRev = pending && pending.repoId === repo.id ? pending.rev : "";
    setDraft((current) => ({
      ...current,
      repoId: repo.id,
      rev: preservedRev || repo.currentBranch || "",
      serverIds: repo.defaultServerId ? [repo.defaultServerId] : [],
      targetDir: repo.defaultTargetDir ?? "",
      // 脚本列表属于「单次部署选择」，切换仓库时清空，避免把上一个仓库的脚本带到新仓库。
      scripts: [],
      uploadEnv: files.length > 0,
    }));
    setCustomRev(Boolean(preservedRev));
    setEnvFiles(files);
    setEnvFilesHydrated(true);
  }, [draft.repoId, repos, editingRepoId, initial]);

  /** 环境文件的所有修改都标记为已水合，避免因仓库列表缺失而丢弃用户刚做的编辑。 */
  function updateEnvFiles(updater: (current: EnvFileConfig[]) => EnvFileConfig[]) {
    setEnvFiles(updater);
    setEnvFilesHydrated(true);
  }

  useEffect(() => {
    const dir = draft.scriptDir.trim();
    if (!draft.repoId || !dir) {
      setScriptOptions([]);
      return;
    }
    let cancelled = false;
    void api
      .listDir(draft.repoId, dir)
      .then((entries) => {
        if (!cancelled) {
          setScriptOptions(
            entries
              .filter((entry) => !entry.isDir && entry.name.endsWith(".sh"))
              .map((entry) => entry.name),
          );
        }
      })
      .catch(() => {
        if (!cancelled) setScriptOptions([]);
      });
    return () => {
      cancelled = true;
    };
  }, [draft.repoId, draft.scriptDir]);

  useEffect(() => {
    const rev = draft.rev.trim();
    if (!draft.repoId || !rev) {
      setResolved(null);
      setResolveError(null);
      setResolving(false);
      return;
    }
    let cancelled = false;
    setResolving(true);
    setResolveError(null);
    setResolved(null);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const result = await api.resolveRev(draft.repoId, rev);
          if (!cancelled) {
            setResolved(result);
            setResolveError(null);
          }
        } catch (error) {
          if (!cancelled) {
            setResolved(null);
            setResolveError(String(error));
          }
        } finally {
          if (!cancelled) setResolving(false);
        }
      })();
    }, 300);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [draft.repoId, draft.rev]);

  const versionChoices = useMemo(
    () => [{ value: "", label: t("deploy.worktreeOption") }, ...branchChoices],
    [branchChoices, t],
  );

  const selectedRepo = repos.find((item) => item.id === draft.repoId);
  const selectedServers = draft.serverIds
    .map((id) => servers.find((item) => item.id === id))
    .filter((server): server is NonNullable<typeof server> => server !== undefined);

  function update(patch: Partial<DeployConfig>) {
    setDraft((current) => ({ ...current, ...patch }));
  }

  function addScript(name: string) {
    const value = name.trim();
    if (!value) return;
    setDraft((current) =>
      current.scripts.includes(value)
        ? current
        : { ...current, scripts: [...current.scripts, value] },
    );
  }

  function moveScript(index: number, delta: number) {
    setDraft((current) => {
      const target = index + delta;
      if (target < 0 || target >= current.scripts.length) return current;
      const next = [...current.scripts];
      [next[index], next[target]] = [next[target], next[index]];
      return { ...current, scripts: next };
    });
  }

  function removeScript(name: string) {
    setDraft((current) => ({
      ...current,
      scripts: current.scripts.filter((item) => item !== name),
    }));
  }

  async function pickEnvFile(index?: number) {
    try {
      const selected = await openDialog({ multiple: false, title: t("deploy.pickEnvFile") });
      if (typeof selected !== "string") return;
      // 自动套用仓库内相对路径（如 <repo>/backend/.env → backend/.env），
      // 免去每次手填目标目录；已手填的远端路径保持不动。
      const repoPath = repos.find((repo) => repo.id === draft.repoId)?.path ?? null;
      const remotePath = inferEnvRemotePath(selected, repoPath);
      if (index === undefined) {
        updateEnvFiles((current) => [...current, { localPath: selected, remotePath }]);
        return;
      }
      updateEnvFiles((current) =>
        current.map((file, itemIndex) =>
          itemIndex === index
            ? {
                localPath: selected,
                remotePath: file.remotePath.trim() ? file.remotePath : remotePath,
              }
            : file,
        ),
      );
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleSave() {
    if (!draft.name.trim()) return toast("error", t("deploy.configNameRequired"));
    if (!draft.repoId) return toast("error", t("deploy.errorRepo"));
    if (draft.serverIds.length === 0)
      return toast("error", t("deploy.errorServer"));
    if (!draft.targetDir.trim()) return toast("error", t("deploy.errorTargetDir"));
    if (resolveError) return toast("error", t("deploy.errorResolve"));

    setSaving(true);
    try {
      const saved = await api.saveDeployConfig({
        ...draft,
        name: draft.name.trim(),
        targetDir: draft.targetDir.trim(),
        rev: draft.rev.trim(),
        scriptDir: draft.scriptDir.trim() || settings.scriptDir,
      });
      // 立即回填 id / 创建时间：若后续环境文件保存失败，重试仍更新同一条配置，
      // 不会因为 id 为空而变成「重名新建」或产生重复配置。
      setDraft((current) => ({ ...current, id: saved.id, createdAt: saved.createdAt }));
      if (envFilesHydrated) {
        try {
          await api.saveRepoEnvFiles(saved.repoId, envFiles);
          // 同步仓库快照后再关弹窗：否则立刻重新打开会读到旧的环境文件列表，
          // 再次保存时会把刚写入的改动整体覆盖回去。
          await refreshRepos();
        } catch (error) {
          // 配置已保存、仅环境文件失败：刷新列表让新配置可见，弹窗保持打开供修正后重试。
          onRefresh();
          throw error;
        }
      }
      onSaved(saved);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open
      onClose={saving ? () => undefined : onClose}
      title={
        duplicate
          ? t("deploy.copyConfig")
          : initial
            ? t("deploy.editConfig")
            : t("deploy.newConfig")
      }
      subtitle={t("deploy.configModalSubtitle")}
      width="max-w-2xl"
      footer={
        <ConfigModalFooter
          onClose={onClose}
          onSave={() => void handleSave()}
          saving={saving}
          testing={false}
          saveIcon={false}
        />
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("deploy.configName")} required>
          <Input
            value={draft.name}
            onChange={(event) => update({ name: event.target.value })}
            placeholder={t("deploy.configNamePlaceholder")}
            disabled={saving}
          />
        </Field>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("deploy.repo")} required>
            <Select
              value={draft.repoId}
              onChange={(event) => update({ repoId: event.target.value })}
              disabled={saving}
            >
              <option value="">{t("deploy.repoPlaceholder")}</option>
              {repos.map((repo) => (
                <option key={repo.id} value={repo.id}>
                  {repo.name}
                </option>
              ))}
            </Select>
          </Field>

          <Field label={t("deploy.server")} required hint={t("deploy.serversHint")}>
            {servers.length > 0 && (
              <ServerCheckList
                servers={servers}
                checkedIds={draft.serverIds}
                disabled={saving}
                onChange={(ids) =>
                  update({
                    serverIds: ids,
                    // 目录还空着时用这一台的默认目录兜底，省掉一次手填。
                    ...(ids.length > 0 && !draft.targetDir.trim()
                      ? {
                          targetDir:
                            servers.find((item) => item.id === ids[0])?.defaultTargetDir ?? "",
                        }
                      : {}),
                  })
                }
              />
            )}
            {servers.length === 0 && (
              <p className="mt-2 text-[11px] text-ink-faint">
                <Trans
                  i18nKey="deploy.noServers"
                  components={{
                    link: (
                      <button
                        type="button"
                        className="mx-0.5 text-brand hover:underline"
                        onClick={() => navigate("/servers")}
                      />
                    ),
                  }}
                />
              </p>
            )}
          </Field>
        </div>

        <Field
          label={t("deploy.revision")}
          hint={resolving ? t("deploy.resolving") : resolved?.short}
        >
          {customRev ? (
            <div className="flex gap-2">
              <div className="min-w-0 flex-1">
                <SearchSelect
                  value={draft.rev}
                  onChange={(value) => update({ rev: value })}
                  options={versionChoices}
                  allowCustom
                  placeholder={t("deploy.branchOrCommit")}
                  disabled={saving}
                />
              </div>
              <Button
                variant="secondary"
                onClick={() => {
                  setCustomRev(false);
                  update({ rev: selectedRepo?.currentBranch ?? "" });
                }}
              >
                {t("deploy.selectBranch")}
              </Button>
            </div>
          ) : (
            <div className="flex gap-2">
              <div className="min-w-0 flex-1">
                <SearchSelect
                  value={draft.rev}
                  onChange={(value) => update({ rev: value })}
                  options={versionChoices}
                  placeholder={t("deploy.branchPlaceholder")}
                  disabled={saving || !branchesLoaded}
                />
              </div>
              <Button variant="secondary" onClick={() => setCustomRev(true)}>
                {t("deploy.specifyCommit")}
              </Button>
            </div>
          )}
          {!draft.rev.trim() && !resolveError && (
            <p className="mt-2 flex items-start gap-1.5 text-[11px] text-ink-dim">
              <CircleDot className="mt-0.5 size-3 shrink-0 text-brand" />
              <span className="min-w-0">{t("deploy.worktreeNote")}</span>
            </p>
          )}
          {resolved && !resolveError && (
            <p className="mt-2 flex items-start gap-1.5 text-[11px] text-ink-dim">
              <CircleDot className="mt-0.5 size-3 shrink-0 text-pos" />
              <span className="min-w-0">
                <span className="text-ink-dim">{resolved.short}</span> {resolved.subject}
                <span className="text-ink-faint">
                  {" "}
                  · {resolved.author} · {resolved.date}
                </span>
              </span>
            </p>
          )}
          {resolveError && (
            <p className="mt-2 flex items-start gap-1.5 text-[11px] text-neg">
              <AlertTriangle className="mt-0.5 size-3 shrink-0" />
              {resolveError}
            </p>
          )}
        </Field>

        <Field label={t("deploy.targetDir")} required hint={t("deploy.targetDirHint")}>
          <Input
            value={draft.targetDir}
            onChange={(event) => update({ targetDir: event.target.value })}
            placeholder="/opt/apps/my-app"
            disabled={saving}
          />
        </Field>

        <div className="rounded-md border border-line bg-field p-3.5">
          <Checkbox
            checked={draft.runScripts}
            onChange={(checked) => update({ runScripts: checked })}
            disabled={saving}
          >
            {t("deploy.runScripts")}
          </Checkbox>
          {draft.runScripts && (
            <div className="mt-3 flex flex-col gap-3">
              <div className="grid grid-cols-2 gap-3">
                <Field label={t("deploy.scriptDir")}>
                  <Input
                    value={draft.scriptDir}
                    onChange={(event) => update({ scriptDir: event.target.value })}
                    placeholder="docker"
                    disabled={saving}
                  />
                </Field>
                <Field label={t("deploy.script")} hint={t("deploy.scriptHint")}>
                  <SearchSelect
                    value=""
                    onChange={addScript}
                    options={scriptOptions.map((name) => ({ value: name, label: name }))}
                    allowCustom
                    placeholder={t("deploy.scriptPlaceholder")}
                    disabled={saving}
                  />
                </Field>
              </div>
              {draft.scripts.length > 0 && (
                <div className="flex flex-col gap-1">
                  <span className="text-[11px] text-ink-faint">
                    {t("deploy.scriptOrderHint")}
                  </span>
                  {draft.scripts.map((name, index) => (
                    <div
                      key={name}
                      className="flex items-center gap-1.5 rounded-md border border-line bg-panel px-2 py-1"
                    >
                      <span className="w-5 shrink-0 text-right font-mono text-[10px] text-ink-faint">
                        {index + 1}
                      </span>
                      <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink">
                        {name}
                      </span>
                      <button
                        type="button"
                        disabled={saving || index === 0}
                        title={t("common.moveUp")}
                        onClick={() => moveScript(index, -1)}
                        className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink disabled:opacity-30"
                      >
                        <ChevronUp className="size-3.5" />
                      </button>
                      <button
                        type="button"
                        disabled={saving || index === draft.scripts.length - 1}
                        title={t("common.moveDown")}
                        onClick={() => moveScript(index, 1)}
                        className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink disabled:opacity-30"
                      >
                        <ChevronDown className="size-3.5" />
                      </button>
                      <button
                        type="button"
                        disabled={saving}
                        title={t("common.delete")}
                        onClick={() => removeScript(name)}
                        className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-neg disabled:opacity-30"
                      >
                        <X className="size-3.5" />
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>

        <div className="rounded-md border border-line bg-field p-3.5">
          <Checkbox
            checked={draft.uploadEnv}
            onChange={(checked) => update({ uploadEnv: checked })}
            disabled={saving}
          >
            {t("deploy.uploadEnv")}
          </Checkbox>
          {draft.uploadEnv && (
            <div className="mt-3 flex flex-col gap-2">
              <p className="text-[11px] leading-relaxed text-ink-faint">
                {t("deploy.envFilesHint")}
              </p>
              {!envFilesHydrated ? (
                // 还没读到仓库的环境文件列表：禁止编辑，避免保存时只写回部分文件。
                <p className="text-[11px] text-ink-faint">{t("deploy.envFilesLoading")}</p>
              ) : (
                <>
                  {envFiles.length === 0 && (
                    <p className="text-[11px] text-ink-faint">{t("deploy.envFilesEmpty")}</p>
                  )}
                  {envFiles.map((file, index) => (
                    <div
                      key={`${file.localPath}-${index}`}
                      className="rounded-md border border-line bg-panel px-2 py-1.5"
                    >
                      <div className="flex items-center gap-1.5">
                        <FileUp className="size-3.5 shrink-0 text-ink-dim" />
                        <button
                          type="button"
                          disabled={saving}
                          onClick={() => void pickEnvFile(index)}
                          title={file.localPath || t("deploy.pickEnvFile")}
                          className={cn(
                            "min-w-0 flex-1 truncate text-left font-mono text-[11px] hover:underline disabled:opacity-60",
                            file.localPath ? "text-ink" : "text-ink-faint",
                          )}
                        >
                          {file.localPath ? basename(file.localPath) : t("deploy.pickEnvFile")}
                        </button>
                        <button
                          type="button"
                          disabled={saving}
                          title={t("common.delete")}
                          onClick={() =>
                            updateEnvFiles((current) =>
                              current.filter((_, itemIndex) => itemIndex !== index),
                            )
                          }
                          className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-neg disabled:opacity-30"
                        >
                          <X className="size-3.5" />
                        </button>
                      </div>
                      <div className="mt-1.5 flex items-center gap-1.5">
                        <span className="shrink-0 text-[10px] text-ink-faint">
                          {t("deploy.envRemotePath")}
                        </span>
                        <Input
                          value={file.remotePath}
                          placeholder=".env 或 docker/.env"
                          disabled={saving}
                          onChange={(event) =>
                            updateEnvFiles((current) =>
                              current.map((item, itemIndex) =>
                                itemIndex === index
                                  ? { ...item, remotePath: event.target.value }
                                  : item,
                              ),
                            )
                          }
                          className="h-7! min-w-0 flex-1 text-[11px]!"
                        />
                      </div>
                    </div>
                  ))}
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={saving}
                    icon={<Plus className="size-3.5" />}
                    onClick={() => void pickEnvFile()}
                  >
                    {t("deploy.addEnvFile")}
                  </Button>
                </>
              )}
            </div>
          )}
        </div>

        {settings.atomicRelease && (
          <p className="text-[11px] leading-relaxed text-ink-faint">{t("deploy.atomicHint")}</p>
        )}

        {selectedRepo && (
          <p className="truncate text-[11px] text-ink-faint" title={selectedRepo.path}>
            {selectedServers.length > 0 ? `${selectedServers.map((item) => item.name).join("、")} → ` : ""}
            {draft.targetDir || t("deploy.targetDirEmpty")}
          </p>
        )}
      </div>
    </Modal>
  );
}
