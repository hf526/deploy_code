import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Cloud,
  Link2,
  Play,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { BindRemoteModal } from "../components/BindRemoteModal";
import { LogConsole } from "../components/LogConsole";
import { SearchSelect } from "../components/SearchSelect";
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
} from "../components/ui";
import { api } from "../lib/api";
import i18n from "../lib/i18n";
import { useApp } from "../lib/store";
import type { Branch, PagesConfig, PagesDeployRecord } from "../lib/types";
import { githubTarget } from "../lib/utils";

const EMPTY_CONFIG: PagesConfig = {
  provider: "cloudflare",
  projectName: "",
  buildCommand: "",
  outputDir: "dist",
  branch: "main",
  publishBranch: "gh-pages",
};

function statusBadge(record: PagesDeployRecord) {
  if (record.status === "success") return <Badge kind="green">{i18n.t("status.success")}</Badge>;
  if (record.status === "failed") return <Badge kind="red">{i18n.t("status.failed")}</Badge>;
  return <Badge kind="amber">{i18n.t("status.running")}</Badge>;
}

export default function PagesPage() {
  const { t } = useTranslation();
  const repos = useApp((state) => state.repos);
  const settings = useApp((state) => state.settings);
  const records = useApp((state) => state.pagesRecords);
  const live = useApp((state) => state.live);
  const livePages = useApp((state) => state.livePages);
  const startPagesDeploy = useApp((state) => state.startPagesDeploy);
  const refreshPagesRecords = useApp((state) => state.refreshPagesRecords);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const clearLivePages = useApp((state) => state.clearLivePages);
  const toast = useApp((state) => state.toast);

  const [repoId, setRepoId] = useState("");
  const [draft, setDraft] = useState<PagesConfig>(EMPTY_CONFIG);
  const [skipBuild, setSkipBuild] = useState(false);
  const [saving, setSaving] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [branches, setBranches] = useState<Branch[]>([]);
  const [bindOpen, setBindOpen] = useState(false);

  const selected = useMemo(
    () => repos.find((repo) => repo.id === repoId) ?? null,
    [repos, repoId],
  );

  useEffect(() => {
    if (!repoId && repos.length > 0) {
      setRepoId(repos[0].id);
    }
  }, [repos, repoId]);

  useEffect(() => {
    if (!selected) {
      setDraft(EMPTY_CONFIG);
      setLoaded(false);
      setLoadError(false);
      return;
    }
    let cancelled = false;
    setLoaded(false);
    setLoadError(false);
    void api
      .getPagesConfig(selected.id)
      .then((config) => {
        if (!cancelled) {
          setDraft(config);
          setLoaded(true);
        }
      })
      .catch((error) => {
        // 加载失败时保留当前草稿，避免后续保存把已保存配置覆盖成空白；
        // 同时提供「重试」，否则整个表单会被永久禁用。
        if (!cancelled) {
          setLoadError(true);
          toast("error", String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id, reloadKey, toast]);

  useEffect(() => {
    if (!selected) {
      setBranches([]);
      return;
    }
    let cancelled = false;
    void api
      .listBranches(selected.id, false)
      .then((list) => {
        if (!cancelled) setBranches(list);
      })
      .catch(() => {
        if (!cancelled) setBranches([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id]);

  const branchOptions = useMemo(() => {
    const names = branches.map((branch) => branch.name);
    if (draft.branch && !names.includes(draft.branch)) {
      names.unshift(draft.branch);
    }
    return names;
  }, [branches, draft.branch]);

  const branchChoices = useMemo(
    () =>
      branchOptions.map((name) => {
        const branch = branches.find((item) => item.name === name);
        return {
          value: name,
          label: name,
          hint: branch?.isCurrent ? t("deploy.current") : undefined,
        };
      }),
    [branchOptions, branches, t],
  );

  const serverRunning = live?.status === "running";
  const pagesRunning = livePages?.status === "running";
  // 与部署页保持一致：任意一种部署进行中都锁定操作，避免两种任务并发争用同一工作区。
  const running = pagesRunning || serverRunning;
  const tokenReady = settings.cloudflareApiToken.trim().length > 0;
  const accountReady = settings.cloudflareAccountId.trim().length > 0;
  const isGitHub = draft.provider === "github";
  const githubPreview = useMemo(
    () => githubTarget(selected?.remote ?? null),
    [selected?.remote],
  );
  const platformReady = isGitHub
    ? !!githubPreview
    : tokenReady && accountReady;

  async function handleSave() {
    if (!selected || running) return;
    setSaving(true);
    try {
      const saved = await api.savePagesConfig(selected.id, draft);
      setDraft(saved);
      toast("success", t("pages.configSaved", { name: selected.name }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  async function handleTest() {
    if (!selected || submitting || running) return;
    setSubmitting(true);
    try {
      const message = await api.testPages(selected.id, draft);
      toast("success", message || t("backup.testPassed"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDeploy() {
    if (!selected || submitting || running) return;
    if (isGitHub) {
      if (!githubPreview) {
        toast("error", t("pages.errorBindGithub"));
        return;
      }
    } else if (!draft.projectName.trim()) {
      toast("error", t("pages.errorProjectName"));
      return;
    }
    setSubmitting(true);
    try {
      // 先保存当前表单，保证实际部署参数与界面一致。
      const saved = await api.savePagesConfig(selected.id, draft);
      setDraft(saved);
    } catch (error) {
      toast("error", String(error));
      setSubmitting(false);
      return;
    }
    clearLivePages();
    try {
      await startPagesDeploy({ repoId: selected.id, skipBuild });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete(recordId: string) {
    try {
      await api.deletePagesRecord(recordId);
      await refreshPagesRecords();
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleClear() {
    try {
      await api.clearPagesRecords();
      await refreshPagesRecords();
      toast("success", t("pages.cleared"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setConfirmClear(false);
    }
  }

  async function copyUrl(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      toast("success", t("pages.urlCopied"));
    } catch {
      toast("info", url);
    }
  }

  return (
    <Page
      title={t("pages.title")}
      subtitle={t("pages.subtitle")}
      actions={
        <Button
          icon={<RefreshCw className="size-4" />}
          variant="secondary"
          onClick={() => void refreshPagesRecords()}
        >
          {t("pages.refreshRecords")}
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-6">
          <section>
            <SectionTitle
              title={t("pages.projectConfig")}
              description={t("pages.projectConfigDescription")}
            />
            <Card className="flex flex-col gap-4 p-5">
              <Field label={t("pages.repo")} required>
                <Select
                  value={repoId}
                  onChange={(event) => setRepoId(event.target.value)}
                  disabled={repos.length === 0 || saving || submitting || running}
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
                  onChange={(event) =>
                    setDraft({ ...draft, provider: event.target.value as PagesConfig["provider"] })
                  }
                >
                  <option value="cloudflare">Cloudflare Pages</option>
                  <option value="github">GitHub Pages</option>
                </Select>
              </Field>

              {isGitHub ? (
                <>
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
                          <p
                            className="mt-0.5 truncate font-mono text-ink-faint"
                            title={selected.remote}
                          >
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
                          <Button
                            size="sm"
                            variant="secondary"
                            className="self-start"
                            icon={<Link2 className="size-3.5" />}
                            onClick={() => setBindOpen(true)}
                          >
                            {t("remoteModal.editTitle")}
                          </Button>
                        </div>
                      )
                    ) : (
                      <div className="flex flex-col gap-2">
                        <p className="text-warn">{t("pages.repoNoRemote")}</p>
                        <Button
                          size="sm"
                          variant="secondary"
                          className="self-start"
                          icon={<Link2 className="size-3.5" />}
                          onClick={() => setBindOpen(true)}
                        >
                          {t("remoteModal.bindTitle")}
                        </Button>
                      </div>
                    )}
                  </div>

                  <Field label={t("pages.buildCommand")} hint={t("pages.buildCommandHintGithub")}>
                    <Input
                      value={draft.buildCommand}
                      onChange={(event) => setDraft({ ...draft, buildCommand: event.target.value })}
                      placeholder="npm run build"
                    />
                  </Field>

                  <div className="grid grid-cols-2 gap-4">
                    <Field label={t("pages.outputDir")} required>
                      <Input
                        value={draft.outputDir}
                        onChange={(event) => setDraft({ ...draft, outputDir: event.target.value })}
                        placeholder="dist"
                      />
                    </Field>
                    <Field label={t("pages.publishBranch")} hint={t("pages.publishBranchHint")}>
                      <SearchSelect
                        value={draft.publishBranch}
                        onChange={(value) => setDraft({ ...draft, publishBranch: value })}
                        options={branchChoices}
                        allowCustom
                        placeholder="gh-pages"
                      />
                    </Field>
                  </div>
                </>
              ) : (
                <>
                  <Field label={t("pages.projectName")} required hint={t("pages.projectNameHint")}>
                    <Input
                      value={draft.projectName}
                      onChange={(event) => setDraft({ ...draft, projectName: event.target.value })}
                      placeholder="my-site"
                    />
                  </Field>

                  <Field
                    label={t("pages.buildCommand")}
                    hint={t("pages.buildCommandHintCloudflare")}
                  >
                    <Input
                      value={draft.buildCommand}
                      onChange={(event) => setDraft({ ...draft, buildCommand: event.target.value })}
                      placeholder="npm run build"
                    />
                  </Field>

                  <div className="grid grid-cols-2 gap-4">
                    <Field label={t("pages.outputDir")} required>
                      <Input
                        value={draft.outputDir}
                        onChange={(event) => setDraft({ ...draft, outputDir: event.target.value })}
                        placeholder="dist"
                      />
                    </Field>
                    <Field label={t("pages.branch")} hint={t("pages.branchHint")}>
                      <SearchSelect
                        value={draft.branch}
                        onChange={(value) => setDraft({ ...draft, branch: value })}
                        options={branchChoices}
                        placeholder={t("deploy.branchPlaceholder")}
                        disabled={!selected || !loaded}
                      />
                    </Field>
                  </div>
                </>
              )}

              <div className="flex justify-end gap-2 border-t border-line pt-4">
                <Button
                  variant="secondary"
                  loading={submitting}
                  disabled={!selected || !loaded || running}
                  onClick={() => void handleTest()}
                >
                  <ShieldCheck className="size-4" />
                  {t("common.testEnvironment")}
                </Button>
                <Button
                  variant="secondary"
                  loading={saving}
                  disabled={!selected || !loaded || submitting || running}
                  onClick={() => void handleSave()}
                >
                  <Save className="size-4" />
                  {t("common.save")}
                </Button>
              </div>
            </Card>
          </section>

          <section>
            <SectionTitle
              title={t("nav.deploy")}
              description={
                isGitHub
                  ? t("pages.deployDescriptionGithub")
                  : t("pages.deployDescriptionCloudflare")
              }
            />
            <Card className="flex flex-col gap-4 p-5">
              <Checkbox checked={skipBuild} onChange={setSkipBuild}>
                {t("pages.skipBuild")}
              </Checkbox>
              {isGitHub ? (
                <>
                  {!githubPreview && (
                    <p className="text-[11px] leading-relaxed text-warn">
                      {t("pages.needBindGithub")}
                    </p>
                  )}
                  <p className="text-[11px] leading-relaxed text-ink-faint">
                    {t("pages.githubNote")}
                  </p>
                </>
              ) : (
                <>
                  {(!tokenReady || !accountReady) && (
                    <p className="text-[11px] leading-relaxed text-warn">
                      {t("pages.missingToken")}
                    </p>
                  )}
                  <p className="text-[11px] leading-relaxed text-ink-faint">
                    {t("pages.cloudflareNote")}
                  </p>
                </>
              )}
              <Button
                size="lg"
                loading={pagesRunning || submitting}
                disabled={!selected || !loaded || !platformReady || running}
                onClick={() => void handleDeploy()}
              >
                <Play className="size-4" />
                {pagesRunning ? t("deploy.deploying") : t("pages.buildAndDeploy")}
              </Button>
            </Card>
          </section>
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          <section className="flex min-h-[320px] flex-col">
            <SectionTitle
              title={t("backup.liveLog")}
              description={pagesRunning ? t("pages.liveLogRunning") : t("pages.liveLogIdle")}
            />
            <LogConsole
              className="min-h-[280px] flex-1"
              title={t("pages.logTitle")}
              emptyText={t("pages.logEmpty")}
              lines={livePages?.lines ?? []}
            />
          </section>

          <section>
            <SectionTitle
              title={t("pages.records")}
              description={t("backup.recordsCount", { count: records.length })}
              actions={
                records.length > 0 ? (
                  <Button variant="ghost" size="sm" onClick={() => setConfirmClear(true)}>
                    {t("backup.clearRecords")}
                  </Button>
                ) : undefined
              }
            />
            {records.length === 0 ? (
              <EmptyState
                icon={<Cloud className="size-4.5" />}
                title={t("pages.emptyTitle")}
                description={t("pages.emptyDescription")}
              />
            ) : (
              <Card className="divide-y divide-line overflow-hidden">
                {records.map((record) => (
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
                            {record.repoName} → {record.projectName}
                            <span className="ml-2 rounded bg-hover px-1.5 py-0.5 text-[10px] font-normal text-ink-dim">
                              {record.provider === "github" ? "GitHub" : "Cloudflare"}
                            </span>
                            <span className="ml-2 text-[11px] font-normal text-ink-faint">
                              {record.branch}
                              {record.commitShort ? ` · ${record.commitShort}` : ""}
                            </span>
                          </p>
                          <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                            {record.startedAt}
                            {record.url ? ` · ${record.url}` : ""}
                          </p>
                        </div>
                        {statusBadge(record)}
                      </button>
                      {record.url && (
                        <Button variant="ghost" size="sm" onClick={() => void copyUrl(record.url!)}>
                          {t("pages.copyUrl")}
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="sm"
                        title={t("backup.deleteRecord")}
                        onClick={() => void handleDelete(record.id)}
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
        title={t("pages.clearTitle")}
        confirmText={t("common.clear")}
        description={t("pages.clearDescription")}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void handleClear()}
      />

      <BindRemoteModal
        repo={bindOpen ? selected : null}
        onClose={() => setBindOpen(false)}
        onSaved={() => void refreshRepos()}
      />
    </Page>
  );
}
