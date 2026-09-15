import { useEffect, useMemo, useState } from "react";
import { Link2, Save, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import { SearchSelect } from "../../components/SearchSelect";
import { Button, Field, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { Branch, PagesConfig, PagesConfigEntry, RepoInfo } from "../../lib/types";
import { githubTarget } from "../../lib/utils";

const EMPTY_CONFIG: PagesConfig = {
  provider: "cloudflare",
  projectName: "",
  buildCommand: "",
  outputDir: "dist",
  branch: "main",
  publishBranch: "gh-pages",
};

/**
 * Pages 配置的新增 / 编辑弹窗。
 * Pages 配置按仓库保存（一仓库一份），编辑时不能更换仓库。
 */
export function PagesConfigModal({
  entry,
  prefillRepoId,
  onClose,
  onSaved,
  onRequestBindRemote,
}: {
  entry: PagesConfigEntry | null;
  prefillRepoId?: string;
  onClose: () => void;
  onSaved: (repoId: string) => void;
  onRequestBindRemote: (repo: RepoInfo) => void;
}) {
  const { t } = useTranslation();
  const repos = useApp((state) => state.repos);
  const settings = useApp((state) => state.settings);
  const toast = useApp((state) => state.toast);

  const [repoId, setRepoId] = useState(
    () => entry?.repoId ?? prefillRepoId ?? repos[0]?.id ?? "",
  );
  const [draft, setDraft] = useState<PagesConfig>(() => entry?.config ?? EMPTY_CONFIG);
  const [loaded, setLoaded] = useState(() => entry !== null);
  const [loadError, setLoadError] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);
  const [branches, setBranches] = useState<Branch[]>([]);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);

  const selected = repos.find((repo) => repo.id === repoId) ?? null;

  // 打开弹窗时仓库列表可能还没加载完成：优先补深链带入的仓库，其次第一个仓库，
  // 否则表单会一直处于禁用状态。
  useEffect(() => {
    if (entry || repos.length === 0) return;
    if (!repoId) {
      setRepoId(prefillRepoId ?? repos[0].id);
      return;
    }
    // 深链指向的仓库已被删除：回退到第一个仓库，避免表单永久停留在加载失败状态。
    if (!repos.some((repo) => repo.id === repoId)) setRepoId(repos[0].id);
  }, [entry, repoId, repos, prefillRepoId]);

  useEffect(() => {
    if (entry || !repoId) return;
    let cancelled = false;
    setLoaded(false);
    setLoadError(false);
    void api
      .getPagesConfig(repoId)
      .then((config) => {
        if (!cancelled) {
          setDraft(config);
          setLoaded(true);
        }
      })
      .catch((error) => {
        // 加载失败时保留当前草稿，避免保存把已保存配置覆盖成空白；同时提供重试。
        if (!cancelled) {
          setLoadError(true);
          toast("error", String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [entry, repoId, reloadKey, toast]);

  useEffect(() => {
    if (!repoId) {
      setBranches([]);
      return;
    }
    let cancelled = false;
    void api
      .listBranches(repoId, false)
      .then((list) => {
        if (!cancelled) setBranches(list);
      })
      .catch(() => {
        if (!cancelled) setBranches([]);
      });
    return () => {
      cancelled = true;
    };
  }, [repoId]);

  const branchChoices = useMemo(
    () =>
      branches.map((branch) => ({
        value: branch.name,
        label: branch.name,
        hint: branch.isCurrent ? t("deploy.current") : undefined,
      })),
    [branches, t],
  );

  const isGitHub = draft.provider === "github";
  const githubPreview = useMemo(
    () => githubTarget(selected?.remote ?? null),
    [selected?.remote],
  );
  const tokenReady = settings.cloudflareApiToken.trim().length > 0;
  const accountReady = settings.cloudflareAccountId.trim().length > 0;

  function update(patch: Partial<PagesConfig>) {
    setDraft((current) => ({ ...current, ...patch }));
  }

  /** 校验并保存；通过时返回是否成功。 */
  async function save(): Promise<boolean> {
    if (!repoId) {
      toast("error", t("pages.errorRepo"));
      return false;
    }
    if (isGitHub) {
      if (!draft.publishBranch.trim()) {
        toast("error", t("pages.errorPublishBranch"));
        return false;
      }
    } else if (!draft.projectName.trim()) {
      toast("error", t("pages.errorProjectName"));
      return false;
    }
    if (!draft.outputDir.trim()) {
      toast("error", t("pages.errorOutputDir"));
      return false;
    }
    setSaving(true);
    try {
      await api.savePagesConfig(repoId, draft);
      return true;
    } catch (error) {
      toast("error", String(error));
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function handleSave() {
    if (await save()) onSaved(repoId);
  }

  async function handleTest() {
    if (!repoId || !loaded) return;
    setTesting(true);
    try {
      const message = await api.testPages(repoId, draft);
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
      title={entry ? t("pages.editConfig") : t("pages.newConfig")}
      subtitle={t("pages.configModalSubtitle")}
      width="max-w-xl"
      footer={
        <>
          <Button variant="secondary" disabled={saving || testing} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="secondary"
            loading={testing}
            disabled={saving || !loaded || !repoId}
            icon={<ShieldCheck className="size-4" />}
            onClick={() => void handleTest()}
          >
            {t("common.testEnvironment")}
          </Button>
          <Button
            loading={saving}
            disabled={testing || !loaded}
            icon={<Save className="size-4" />}
            onClick={() => void handleSave()}
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("pages.repo")} required hint={entry ? t("pages.repoLocked") : undefined}>
          <Select
            value={repoId}
            onChange={(event) => setRepoId(event.target.value)}
            disabled={entry !== null || saving || repos.length === 0}
          >
            {repos.length === 0 && <option value="">{t("pages.noRepos")}</option>}
            {repos.map((repo) => (
              <option key={repo.id} value={repo.id}>
                {repo.name}
              </option>
            ))}
          </Select>
        </Field>

        {loadError && (
          <div className="flex items-center justify-between gap-3 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-[11px] text-warn">
            <span>{t("pages.configLoadFailed")}</span>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => setReloadKey((value) => value + 1)}
            >
              {t("common.retry")}
            </Button>
          </div>
        )}

        <Field label={t("pages.provider")} required>
          <Select
            value={draft.provider}
            onChange={(event) => update({ provider: event.target.value as PagesConfig["provider"] })}
            disabled={saving || !loaded}
          >
            <option value="cloudflare">Cloudflare Pages</option>
            <option value="github">GitHub Pages</option>
          </Select>
        </Field>

        {isGitHub ? (
          <>
            <Field label={t("pages.publishBranch")} hint={t("pages.publishBranchHint")}>
              <SearchSelect
                value={draft.publishBranch}
                onChange={(value) => update({ publishBranch: value })}
                options={branchChoices}
                allowCustom
                placeholder="gh-pages"
                disabled={saving || !loaded}
              />
            </Field>

            <div className="rounded-md border border-line bg-sunken px-3 py-2 text-[11px] leading-relaxed">
              {selected?.remote ? (
                githubPreview ? (
                  <>
                    <p className="text-ink-dim">
                      {t("pages.repoLine", {
                        owner: githubPreview.owner,
                        repo: githubPreview.repo,
                      })}
                    </p>
                    <p className="mt-0.5 truncate font-mono text-ink-faint" title={selected.remote}>
                      {selected.remote}
                    </p>
                    <p className="mt-0.5 text-ink-faint">
                      {t("pages.expectedUrlLabel")}
                      <span className="font-mono">{githubPreview.url}</span>
                    </p>
                  </>
                ) : (
                  <div className="flex flex-col gap-2">
                    <p className="text-warn">{t("pages.remoteNotGithub")}</p>
                    {selected && (
                      <Button
                        size="sm"
                        variant="secondary"
                        className="self-start"
                        icon={<Link2 className="size-3.5" />}
                        onClick={() => onRequestBindRemote(selected)}
                      >
                        {t("remoteModal.editTitle")}
                      </Button>
                    )}
                  </div>
                )
              ) : (
                <div className="flex flex-col gap-2">
                  <p className="text-warn">{t("pages.repoNoRemote")}</p>
                  {selected && (
                    <Button
                      size="sm"
                      variant="secondary"
                      className="self-start"
                      icon={<Link2 className="size-3.5" />}
                      onClick={() => onRequestBindRemote(selected)}
                    >
                      {t("remoteModal.bindTitle")}
                    </Button>
                  )}
                </div>
              )}
            </div>

            <Field label={t("pages.buildCommand")} hint={t("pages.buildCommandHintGithub")}>
              <Input
                value={draft.buildCommand}
                onChange={(event) => update({ buildCommand: event.target.value })}
                placeholder="npm run build"
                disabled={saving || !loaded}
              />
            </Field>

            <Field label={t("pages.outputDir")} required>
              <Input
                value={draft.outputDir}
                onChange={(event) => update({ outputDir: event.target.value })}
                placeholder="dist"
                disabled={saving || !loaded}
              />
            </Field>

            <p className="text-[11px] leading-relaxed text-ink-faint">{t("pages.githubNote")}</p>
          </>
        ) : (
          <>
            <Field label={t("pages.branch")} hint={t("pages.branchHint")}>
              <SearchSelect
                value={draft.branch}
                onChange={(value) => update({ branch: value })}
                options={branchChoices}
                placeholder={t("deploy.branchPlaceholder")}
                disabled={saving || !loaded}
              />
            </Field>

            <Field label={t("pages.projectName")} required hint={t("pages.projectNameHint")}>
              <Input
                value={draft.projectName}
                onChange={(event) => update({ projectName: event.target.value })}
                placeholder="my-site"
                disabled={saving || !loaded}
              />
            </Field>

            <Field label={t("pages.buildCommand")} hint={t("pages.buildCommandHintCloudflare")}>
              <Input
                value={draft.buildCommand}
                onChange={(event) => update({ buildCommand: event.target.value })}
                placeholder="npm run build"
                disabled={saving || !loaded}
              />
            </Field>

            <Field label={t("pages.outputDir")} required>
              <Input
                value={draft.outputDir}
                onChange={(event) => update({ outputDir: event.target.value })}
                placeholder="dist"
                disabled={saving || !loaded}
              />
            </Field>

            {(!tokenReady || !accountReady) && (
              <p className="text-[11px] leading-relaxed text-warn">{t("pages.missingToken")}</p>
            )}
            <p className="text-[11px] leading-relaxed text-ink-faint">
              {t("pages.cloudflareNote")}
            </p>
          </>
        )}
      </div>
    </Modal>
  );
}
