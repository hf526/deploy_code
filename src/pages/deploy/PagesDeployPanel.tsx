import { useEffect, useMemo, useRef, useState } from "react";
import { Cloud, Save, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import { SearchSelect, type SearchOption } from "../../components/SearchSelect";
import { Button, Field, Input, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { PagesConfig, RepoInfo } from "../../lib/types";
import { githubTarget } from "../../lib/utils";

const EMPTY_PAGES_CONFIG: PagesConfig = {
  provider: "cloudflare",
  projectName: "",
  buildCommand: "",
  outputDir: "dist",
  branch: "main",
  publishBranch: "gh-pages",
};

/** Pages 部署配置表单：状态与保存/测试/部署操作内聚在本组件，切换仓库时重新加载配置。 */
export function PagesDeployPanel({
  visible,
  repoId,
  selectedRepo,
  branchChoices,
  serverRunning,
  submitting,
  setSubmitting,
}: {
  /** 隐藏时保留组件状态（未保存的草稿不会因切换部署方式丢失）。 */
  visible: boolean;
  repoId: string;
  selectedRepo: RepoInfo | undefined;
  branchChoices: SearchOption[];
  serverRunning: boolean;
  submitting: boolean;
  setSubmitting: (value: boolean) => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const settings = useApp((state) => state.settings);
  const pagesRunning = useApp((state) => state.livePages?.status === "running");
  const startPagesDeploy = useApp((state) => state.startPagesDeploy);
  const clearLivePages = useApp((state) => state.clearLivePages);

  const [pagesDraft, setPagesDraft] = useState<PagesConfig>(EMPTY_PAGES_CONFIG);
  const [pagesLoaded, setPagesLoaded] = useState(false);
  const [pagesLoadError, setPagesLoadError] = useState(false);
  const [pagesReloadKey, setPagesReloadKey] = useState(0);
  const [skipBuild, setSkipBuild] = useState(false);
  const [savingPages, setSavingPages] = useState(false);

  // 镜像当前仓库 id：异步保存/部署返回时判断仓库是否已切换，避免把旧仓库配置写进新仓库表单。
  const repoIdRef = useRef(repoId);
  repoIdRef.current = repoId;

  const running = serverRunning || pagesRunning;
  const isGitHubPages = pagesDraft.provider === "github";
  const githubPreview = useMemo(
    () => githubTarget(selectedRepo?.remote ?? null),
    [selectedRepo?.remote],
  );
  const tokenReady = settings.cloudflareApiToken.trim().length > 0;
  const accountReady = settings.cloudflareAccountId.trim().length > 0;
  const pagesPlatformReady = isGitHubPages ? !!githubPreview : tokenReady && accountReady;

  // Pages 部署使用按仓库保存的配置，切换仓库时加载对应配置。
  useEffect(() => {
    if (!repoId) {
      setPagesDraft(EMPTY_PAGES_CONFIG);
      setPagesLoaded(false);
      setPagesLoadError(false);
      return;
    }
    let cancelled = false;
    setPagesLoaded(false);
    setPagesLoadError(false);
    void api
      .getPagesConfig(repoId)
      .then((config) => {
        if (!cancelled) {
          setPagesDraft(config);
          setPagesLoaded(true);
        }
      })
      .catch((error) => {
        // 加载失败时保留当前草稿，避免保存时把已保存配置覆盖成空白；
        // 同时提供「重试」，否则整个表单会被永久禁用。
        if (!cancelled) {
          setPagesLoadError(true);
          toast("error", String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [repoId, pagesReloadKey, toast]);

  // 「跳过构建」属于单次部署选择，切换仓库时清空。
  useEffect(() => {
    setSkipBuild(false);
  }, [repoId]);

  async function handleSavePages() {
    if (!repoId) return toast("error", t("deploy.errorRepo"));
    if (running) return;
    const targetRepo = repoId;
    setSavingPages(true);
    try {
      const saved = await api.savePagesConfig(targetRepo, pagesDraft);
      // 保存期间切换了仓库：丢弃结果，防止旧仓库配置覆盖当前表单。
      if (targetRepo !== repoIdRef.current) return;
      setPagesDraft(saved);
      toast("success", t("pages.configSaved", { name: selectedRepo?.name ?? "" }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSavingPages(false);
    }
  }

  async function handleTestPages() {
    if (!repoId || submitting || running) return;
    setSubmitting(true);
    try {
      const message = await api.testPages(repoId, pagesDraft);
      toast("success", message || t("backup.testPassed"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function handlePagesDeploy() {
    if (!repoId) return toast("error", t("deploy.errorRepo"));
    if (running) return;
    if (isGitHubPages) {
      if (!githubPreview) return toast("error", t("pages.errorBindGithub"));
    } else if (!pagesDraft.projectName.trim()) {
      return toast("error", t("pages.errorProjectName"));
    }
    setSubmitting(true);
    try {
      // 先保存当前表单，保证实际部署参数与界面一致。
      const targetRepo = repoId;
      const saved = await api.savePagesConfig(targetRepo, pagesDraft);
      // 保存期间切换了仓库：放弃本次部署，避免日志/草稿串到新仓库。
      if (targetRepo !== repoIdRef.current) return;
      setPagesDraft(saved);
      clearLivePages();
      await startPagesDeploy({ repoId: targetRepo, skipBuild });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className={visible ? "flex flex-col gap-4" : "hidden"}>
      <Field label={t("pages.provider")} required>
        <Select
          value={pagesDraft.provider}
          onChange={(event) =>
            setPagesDraft({
              ...pagesDraft,
              provider: event.target.value as PagesConfig["provider"],
            })
          }
          disabled={running}
        >
          <option value="cloudflare">Cloudflare Pages</option>
          <option value="github">GitHub Pages</option>
        </Select>
      </Field>

      {isGitHubPages ? (
        <>
          <Field label={t("pages.publishBranch")} hint={t("pages.publishBranchHint")}>
            <SearchSelect
              value={pagesDraft.publishBranch}
              onChange={(value) => setPagesDraft({ ...pagesDraft, publishBranch: value })}
              options={branchChoices}
              allowCustom
              placeholder="gh-pages"
              disabled={running || !repoId || !pagesLoaded}
            />
          </Field>

          <div className="rounded-md border border-line bg-sunken px-3 py-2 text-[11px] leading-relaxed">
            {selectedRepo?.remote ? (
              githubPreview ? (
                <>
                  <p className="text-ink-dim">
                    {t("pages.repoLine", {
                      owner: githubPreview.owner,
                      repo: githubPreview.repo,
                    })}
                  </p>
                  <p
                    className="mt-0.5 truncate font-mono text-ink-faint"
                    title={selectedRepo.remote}
                  >
                    {selectedRepo.remote}
                  </p>
                  <p className="mt-0.5 text-ink-faint">
                    {t("pages.expectedUrlLabel")}
                    <span className="font-mono">{githubPreview.url}</span>
                  </p>
                </>
              ) : (
                <p className="text-warn">{t("pages.remoteNotGithub")}</p>
              )
            ) : (
              <p className="text-warn">{t("pages.repoNoRemote")}</p>
            )}
          </div>

          <Field label={t("pages.buildCommand")} hint={t("pages.buildCommandHintGithub")}>
            <Input
              value={pagesDraft.buildCommand}
              onChange={(event) =>
                setPagesDraft({ ...pagesDraft, buildCommand: event.target.value })
              }
              placeholder="npm run build"
              disabled={running}
            />
          </Field>

          <Field label={t("pages.outputDir")} required>
            <Input
              value={pagesDraft.outputDir}
              onChange={(event) =>
                setPagesDraft({ ...pagesDraft, outputDir: event.target.value })
              }
              placeholder="dist"
              disabled={running}
            />
          </Field>

          {!githubPreview && (
            <p className="text-[11px] leading-relaxed text-warn">
              {t("pages.needBindGithub")}
            </p>
          )}
          <p className="text-[11px] leading-relaxed text-ink-faint">{t("pages.githubNote")}</p>
        </>
      ) : (
        <>
          <Field label={t("pages.branch")} hint={t("pages.branchHint")}>
            <SearchSelect
              value={pagesDraft.branch}
              onChange={(value) => setPagesDraft({ ...pagesDraft, branch: value })}
              options={branchChoices}
              placeholder={t("deploy.branchPlaceholder")}
              disabled={running || !repoId || !pagesLoaded}
            />
          </Field>

          <Field label={t("pages.projectName")} required hint={t("pages.projectNameHint")}>
            <Input
              value={pagesDraft.projectName}
              onChange={(event) =>
                setPagesDraft({ ...pagesDraft, projectName: event.target.value })
              }
              placeholder="my-site"
              disabled={running}
            />
          </Field>

          <Field label={t("pages.buildCommand")} hint={t("pages.buildCommandHintCloudflare")}>
            <Input
              value={pagesDraft.buildCommand}
              onChange={(event) =>
                setPagesDraft({ ...pagesDraft, buildCommand: event.target.value })
              }
              placeholder="npm run build"
              disabled={running}
            />
          </Field>

          <Field label={t("pages.outputDir")} required>
            <Input
              value={pagesDraft.outputDir}
              onChange={(event) =>
                setPagesDraft({ ...pagesDraft, outputDir: event.target.value })
              }
              placeholder="dist"
              disabled={running}
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

      <div className="rounded-md border border-line bg-field p-3.5">
        <label className="flex cursor-pointer items-center gap-2.5 text-xs text-ink">
          <input
            type="checkbox"
            checked={skipBuild}
            onChange={(event) => setSkipBuild(event.target.checked)}
            disabled={running}
            className="size-3.5 accent-primary"
          />
          {t("pages.skipBuild")}
        </label>
      </div>

      {pagesLoadError && (
        <div className="flex items-center justify-between gap-3 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-[11px] text-warn">
          <span>{t("pages.configLoadFailed")}</span>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => setPagesReloadKey((value) => value + 1)}
          >
            {t("common.retry")}
          </Button>
        </div>
      )}

      <div className="flex justify-end gap-2 border-t border-line pt-4">
        <Button
          variant="secondary"
          loading={submitting}
          disabled={!repoId || !pagesLoaded || running}
          onClick={() => void handleTestPages()}
        >
          <ShieldCheck className="size-4" />
          {t("common.testEnvironment")}
        </Button>
        <Button
          variant="secondary"
          loading={savingPages}
          disabled={!repoId || !pagesLoaded || submitting || running}
          onClick={() => void handleSavePages()}
        >
          <Save className="size-4" />
          {t("common.save")}
        </Button>
      </div>

      <Button
        size="lg"
        loading={submitting || pagesRunning}
        disabled={!repoId || !pagesLoaded || !pagesPlatformReady || serverRunning}
        icon={<Cloud className="size-4" />}
        onClick={() => void handlePagesDeploy()}
      >
        {pagesRunning ? t("deploy.deploying") : t("pages.buildAndDeploy")}
      </Button>

      {selectedRepo && (
        <p className="truncate text-center text-[11px] text-ink-faint">
          {isGitHubPages
            ? `GitHub Pages → ${githubPreview?.repo ?? t("deploy.targetDirEmpty")}`
            : `Cloudflare Pages → ${pagesDraft.projectName || t("deploy.targetDirEmpty")}`}
        </p>
      )}
    </div>
  );
}
