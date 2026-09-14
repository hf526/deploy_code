import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  CircleDot,
  Cloud,
  FileUp,
  GitBranch,
  History,
  Plus,
  Rocket,
  Save,
  ShieldCheck,
  Square,
  Terminal,
  Timer,
  X,
  XCircle,
} from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";

import { LogConsole } from "../components/LogConsole";
import { SearchSelect } from "../components/SearchSelect";
import { Badge, Button, Card, Field, Input, Page, Select } from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type {
  Branch,
  DeployRecord,
  EnvFileConfig,
  PagesConfig,
  PagesDeployRecord,
  ResolvedRev,
} from "../lib/types";
import {
  cn,
  deployStatusClass,
  deployStatusLabel,
  formatDuration,
  githubTarget,
  shortPath,
} from "../lib/utils";

const EMPTY_PAGES_CONFIG: PagesConfig = {
  provider: "cloudflare",
  projectName: "",
  buildCommand: "",
  outputDir: "dist",
  branch: "main",
  publishBranch: "gh-pages",
};

export default function DeployPage() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const toast = useApp((state) => state.toast);
  const repos = useApp((state) => state.repos);
  const servers = useApp((state) => state.servers);
  const history = useApp((state) => state.history);
  const settings = useApp((state) => state.settings);
  const live = useApp((state) => state.live);
  const startDeploy = useApp((state) => state.startDeploy);
  const redeploy = useApp((state) => state.redeploy);
  const cancelDeploy = useApp((state) => state.cancelDeploy);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const pagesRecords = useApp((state) => state.pagesRecords);
  const livePages = useApp((state) => state.livePages);
  const startPagesDeploy = useApp((state) => state.startPagesDeploy);
  const clearLivePages = useApp((state) => state.clearLivePages);

  const [deployTarget, setDeployTarget] = useState<"server" | "pages">(() =>
    searchParams.get("target") === "pages" ? "pages" : "server",
  );
  const [repoId, setRepoId] = useState(() => searchParams.get("repo") ?? "");
  const [rev, setRev] = useState(() => searchParams.get("rev") ?? "");
  const [customRev, setCustomRev] = useState(() => !!searchParams.get("rev"));
  const [serverId, setServerId] = useState("");
  const [targetDir, setTargetDir] = useState("");
  const [runScripts, setRunScripts] = useState(settings.runScripts);
  const [scriptDir, setScriptDir] = useState(settings.scriptDir);
  const [scripts, setScripts] = useState<string[]>([]);
  const [scriptOptions, setScriptOptions] = useState<string[]>([]);
  const [envFiles, setEnvFiles] = useState<EnvFileConfig[]>([]);
  const [uploadEnv, setUploadEnv] = useState(true);
  const [pagesDraft, setPagesDraft] = useState<PagesConfig>(EMPTY_PAGES_CONFIG);
  const [pagesLoaded, setPagesLoaded] = useState(false);
  const [pagesLoadError, setPagesLoadError] = useState(false);
  const [pagesReloadKey, setPagesReloadKey] = useState(0);
  const [skipBuild, setSkipBuild] = useState(false);
  const [savingPages, setSavingPages] = useState(false);
  const [cancelling, setCancelling] = useState(false);

  const [branches, setBranches] = useState<Branch[]>([]);
  const [branchesLoaded, setBranchesLoaded] = useState(false);
  const [resolved, setResolved] = useState<ResolvedRev | null>(null);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const appliedQuery = useRef("");
  const appliedRepo = useRef("");
  const pendingRev = useRef<{ repoId: string | null; rev: string } | null>(null);
  const repoIdRef = useRef(repoId);
  repoIdRef.current = repoId;
  const envSaveRef = useRef<Promise<void> | null>(null);
  const envSaveSeq = useRef(0);

  useEffect(() => {
    const key = `${searchParams.get("repo") ?? ""}|${searchParams.get("rev") ?? ""}|${searchParams.get("target") ?? ""}`;
    if (key === appliedQuery.current) return;
    appliedQuery.current = key;
    const queryRepo = searchParams.get("repo");
    const queryRev = searchParams.get("rev");
    const queryTarget = searchParams.get("target");
    if (queryRepo) setRepoId(queryRepo);
    if (queryRev) {
      // 版本绑定到所属仓库，避免切换仓库时把旧仓库的版本套用过去；
      // 未带 repo 时留 null，表示接受默认选中的仓库。
      pendingRev.current = { repoId: queryRepo, rev: queryRev };
      setRev(queryRev);
      setCustomRev(true);
    }
    if (queryTarget === "pages" || queryTarget === "server") {
      setDeployTarget(queryTarget);
    }
  }, [searchParams]);

  useEffect(() => {
    if (!repoId && repos.length > 0) setRepoId(repos[0].id);
  }, [repos, repoId]);

  useEffect(() => {
    if (!repoId) {
      setBranches([]);
      setBranchesLoaded(false);
      return;
    }
    let cancelled = false;
    setBranches([]);
    setBranchesLoaded(false);
    void (async () => {
      try {
        const list = await api.listBranches(repoId, false);
        if (!cancelled) {
          setBranches(list);
          setBranchesLoaded(true);
        }
      } catch (error) {
        if (!cancelled) toast("error", String(error));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoId, toast]);

  // 仓库没有任何分支（如尚未提交的空仓库）时默认部署当前工作区，避免版本解析失败后无法部署。
  useEffect(() => {
    if (!repoId || !branchesLoaded || customRev) return;
    if (branches.length === 0 && rev) setRev("");
  }, [repoId, branchesLoaded, branches.length, customRev, rev]);

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

  useEffect(() => {
    const dir = scriptDir.trim();
    if (!repoId || !dir) {
      setScriptOptions([]);
      return;
    }
    let cancelled = false;
    void api
      .listDir(repoId, dir)
      .then((entries) => {
        if (!cancelled) {
          setScriptOptions(
            entries.filter((entry) => !entry.isDir && entry.name.endsWith(".sh")).map((entry) => entry.name),
          );
        }
      })
      .catch(() => {
        if (!cancelled) setScriptOptions([]);
      });
    return () => {
      cancelled = true;
    };
  }, [repoId, scriptDir]);

  useEffect(() => {
    if (!repoId || appliedRepo.current === repoId) return;
    const repo = repos.find((item) => item.id === repoId);
    // 仓库列表尚未加载完成时先不标记，等加载后再补默认值。
    if (!repo) return;
    appliedRepo.current = repoId;
    const pending = pendingRev.current;
    pendingRev.current = null;
    if (pending && (pending.repoId === null || pending.repoId === repoId)) {
      // 深链带入的版本优先于仓库默认分支，否则会被默认值重置覆盖。
      setRev(pending.rev);
      setCustomRev(true);
    } else {
      setRev(repo.currentBranch ?? "");
      setCustomRev(false);
    }
    setTargetDir(repo.defaultTargetDir ?? "");
    setServerId(repo.defaultServerId ?? "");
    const files = repo.envFiles ?? [];
    setEnvFiles(files);
    setUploadEnv(files.length > 0);
  }, [repoId, repos]);

  useEffect(() => {
    setRunScripts(settings.runScripts);
    setScriptDir(settings.scriptDir);
  }, [settings]);

  useEffect(() => {
    if (!repoId || !rev.trim()) {
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
          const result = await api.resolveRev(repoId, rev.trim());
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
  }, [repoId, rev]);

  const branchChoices = useMemo(
    () =>
      branches.map((branch) => ({
        value: branch.name,
        label: branch.name,
        hint: branch.isCurrent ? t("deploy.current") : undefined,
      })),
    [branches, t],
  );

  // 服务端部署的版本选项：额外提供「当前工作区」，留空表示不走分支直接打包本地目录。
  const versionChoices = useMemo(
    () => [{ value: "", label: t("deploy.worktreeOption") }, ...branchChoices],
    [branchChoices, t],
  );

  const selectedRepo = repos.find((item) => item.id === repoId);
  const selectedServer = servers.find((item) => item.id === serverId);

  const recentDeploys = useMemo(
    () => history.filter((record) => record.repoId === repoId).slice(0, 6),
    [history, repoId],
  );

  const recentPagesDeploys = useMemo(
    () => pagesRecords.filter((record) => record.repoId === repoId).slice(0, 6),
    [pagesRecords, repoId],
  );

  const isGitHubPages = pagesDraft.provider === "github";
  const githubPreview = useMemo(
    () => githubTarget(selectedRepo?.remote ?? null),
    [selectedRepo?.remote],
  );
  const tokenReady = settings.cloudflareApiToken.trim().length > 0;
  const accountReady = settings.cloudflareAccountId.trim().length > 0;
  const pagesPlatformReady = isGitHubPages ? !!githubPreview : tokenReady && accountReady;

  function addScript(name: string) {
    const value = name.trim();
    if (!value) return;
    setScripts((current) => (current.includes(value) ? current : [...current, value]));
  }

  function moveScript(index: number, delta: number) {
    setScripts((current) => {
      const target = index + delta;
      if (target < 0 || target >= current.length) return current;
      const next = [...current];
      [next[index], next[target]] = [next[target], next[index]];
      return next;
    });
  }

  function removeScript(name: string) {
    setScripts((current) => current.filter((item) => item !== name));
  }

  function basename(path: string): string {
    const parts = path.replace(/\\/g, "/").split("/");
    return parts[parts.length - 1] ?? path;
  }

  /** 保存环境文件列表到仓库配置（串行，失败时回退到已保存的配置）。 */
  function persistEnvFiles(next: EnvFileConfig[]): Promise<void> {
    setEnvFiles(next);
    if (!repoId) return Promise.resolve();
    const seq = ++envSaveSeq.current;
    const task = (envSaveRef.current ?? Promise.resolve()).then(async () => {
      try {
        await api.saveRepoEnvFiles(repoId, next);
        await refreshRepos();
      } catch (error) {
        toast("error", String(error));
        // 仅当没有更新的保存排队时才回退，避免覆盖后续编辑。
        if (seq === envSaveSeq.current) {
          const repo = repos.find((item) => item.id === repoIdRef.current);
          setEnvFiles(repo?.envFiles ?? []);
        }
      }
    });
    envSaveRef.current = task;
    return task;
  }

  async function pickEnvFile(index?: number) {
    try {
      const selected = await open({ multiple: false, title: t("deploy.pickEnvFile") });
      if (typeof selected !== "string") return;
      if (index === undefined) {
        await persistEnvFiles([...envFiles, { localPath: selected, remotePath: basename(selected) }]);
        return;
      }
      const next = envFiles.map((file, itemIndex) =>
        itemIndex === index
          ? {
              localPath: selected,
              remotePath: file.remotePath.trim() ? file.remotePath : basename(selected),
            }
          : file,
      );
      await persistEnvFiles(next);
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function removeEnvFile(index: number) {
    await persistEnvFiles(envFiles.filter((_, itemIndex) => itemIndex !== index));
  }

  function updateEnvRemote(index: number, value: string) {
    setEnvFiles((current) =>
      current.map((file, itemIndex) => (itemIndex === index ? { ...file, remotePath: value } : file)),
    );
  }

  async function handleDeploy() {
    if (!repoId) return toast("error", t("deploy.errorRepo"));
    if (!serverId) return toast("error", t("deploy.errorServer"));
    if (!targetDir.trim()) return toast("error", t("deploy.errorTargetDir"));
    if (resolveError) return toast("error", t("deploy.errorResolve"));

    setSubmitting(true);
    try {
      // 等待未完成的环境文件保存，避免部署读到旧的远端路径。
      await envSaveRef.current;
      await startDeploy({
        repoId,
        rev: rev.trim(),
        serverId,
        targetDir: targetDir.trim(),
        runScripts,
        scriptDir: scriptDir.trim() || settings.scriptDir,
        scripts,
        uploadEnv,
      });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  async function handleSavePages() {
    if (!repoId) return toast("error", t("deploy.errorRepo"));
    if (running) return;
    setSavingPages(true);
    try {
      const saved = await api.savePagesConfig(repoId, pagesDraft);
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
      const saved = await api.savePagesConfig(repoId, pagesDraft);
      setPagesDraft(saved);
      clearLivePages();
      await startPagesDeploy({ repoId, skipBuild });
    } catch {
      // store 已提示错误
    } finally {
      setSubmitting(false);
    }
  }

  const serverRunning = live?.status === "running";
  const pagesRunning = livePages?.status === "running";
  // 任意一种部署进行中时都锁定表单，避免两种任务并发。
  const running = serverRunning || pagesRunning;
  const activeLive = deployTarget === "pages" ? livePages : live;
  const activeRunning = deployTarget === "pages" ? pagesRunning : serverRunning;

  // 从其他页面（如 Pages 部署页）发起后回到本页时，自动切到对应的部署方式查看日志。
  useEffect(() => {
    if (pagesRunning && !serverRunning) setDeployTarget("pages");
  }, [pagesRunning, serverRunning]);

  return (
    <Page
      title={t("nav.deploy")}
      subtitle={deployTarget === "pages" ? t("pages.subtitle") : t("deploy.subtitle")}
      actions={
        <Button
          variant="secondary"
          icon={<History className="size-4" />}
          onClick={() => navigate("/history")}
        >
          {t("history.title")}
        </Button>
      }
    >
      <div className="grid grid-cols-1 gap-5 xl:grid-cols-[400px_minmax(0,1fr)]">
        <div className="flex flex-col gap-5">
          <Card className="p-5">
            <h2 className="mb-4 flex items-center gap-2.5 text-sm font-semibold tracking-tight text-ink">
              <span className="grid size-7 place-items-center rounded-md border border-brand-line bg-brand-soft">
                <Rocket className="size-3.5 text-brand" />
              </span>
              {t("deploy.config")}
            </h2>

            <div className="flex flex-col gap-4">
              <Field label={t("deploy.repo")} required>
                <Select
                  value={repoId}
                  onChange={(event) => setRepoId(event.target.value)}
                  disabled={running || savingPages || submitting}
                >
                  <option value="">{t("deploy.repoPlaceholder")}</option>
                  {repos.map((repo) => (
                    <option key={repo.id} value={repo.id}>
                      {repo.name}
                    </option>
                  ))}
                </Select>
              </Field>

              <Field label={t("deploy.deployTarget")} required>
                <Select
                  value={deployTarget}
                  onChange={(event) =>
                    setDeployTarget(event.target.value as "server" | "pages")
                  }
                  disabled={running}
                >
                  <option value="server">{t("deploy.deployTargetServer")}</option>
                  <option value="pages">{t("deploy.deployTargetPages")}</option>
                </Select>
              </Field>

              <div
                className={
                  deployTarget === "pages" ? "hidden" : "flex flex-col gap-4"
                }
              >
              <Field
                label={t("deploy.revision")}
                hint={resolving ? t("deploy.resolving") : resolved?.short}
              >
                {customRev ? (
                  <div className="flex gap-2">
                    <div className="min-w-0 flex-1">
                      <SearchSelect
                        value={rev}
                        onChange={setRev}
                        options={versionChoices}
                        allowCustom
                        placeholder={t("deploy.branchOrCommit")}
                        disabled={running}
                      />
                    </div>
                    <Button
                      variant="secondary"
                      onClick={() => {
                        setCustomRev(false);
                        setRev(selectedRepo?.currentBranch ?? "");
                      }}
                    >
                      {t("deploy.selectBranch")}
                    </Button>
                  </div>
                ) : (
                  <div className="flex gap-2">
                    <div className="min-w-0 flex-1">
                      <SearchSelect
                        value={rev}
                        onChange={setRev}
                        options={versionChoices}
                        placeholder={t("deploy.branchPlaceholder")}
                        disabled={running}
                      />
                    </div>
                    <Button variant="secondary" onClick={() => setCustomRev(true)}>
                      {t("deploy.specifyCommit")}
                    </Button>
                  </div>
                )}
                {!rev.trim() && !resolveError && (
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

              <Field label={t("deploy.server")} required>
                <Select
                  value={serverId}
                  onChange={(event) => {
                    const next = event.target.value;
                    setServerId(next);
                    const server = servers.find((item) => item.id === next);
                    if (server && !targetDir.trim()) setTargetDir(server.defaultTargetDir);
                  }}
                  disabled={running}
                >
                  <option value="">{t("deploy.serverPlaceholder")}</option>
                  {servers.map((server) => (
                    <option key={server.id} value={server.id}>
                      {server.name} ({server.username}@{server.host})
                    </option>
                  ))}
                </Select>
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

              <Field label={t("deploy.targetDir")} required hint={t("deploy.targetDirHint")}>
                <Input
                  value={targetDir}
                  onChange={(event) => setTargetDir(event.target.value)}
                  placeholder="/opt/apps/my-app"
                  disabled={running}
                />
              </Field>

              <div className="rounded-md border border-line bg-field p-3.5">
                <label className="flex cursor-pointer items-center gap-2.5 text-xs text-ink">
                  <input
                    type="checkbox"
                    checked={runScripts}
                    onChange={(event) => setRunScripts(event.target.checked)}
                    disabled={running}
                    className="size-3.5 accent-primary"
                  />
                  {t("deploy.runScripts")}
                </label>
                {runScripts && (
                  <div className="mt-3 flex flex-col gap-3">
                    <div className="grid grid-cols-2 gap-3">
                      <Field label={t("deploy.scriptDir")}>
                        <Input
                          value={scriptDir}
                          onChange={(event) => setScriptDir(event.target.value)}
                          placeholder="docker"
                          disabled={running}
                        />
                      </Field>
                      <Field label={t("deploy.script")} hint={t("deploy.scriptHint")}>
                        <SearchSelect
                          value=""
                          onChange={addScript}
                          options={scriptOptions.map((name) => ({ value: name, label: name }))}
                          allowCustom
                          placeholder={t("deploy.scriptPlaceholder")}
                          disabled={running}
                        />
                      </Field>
                    </div>
                    {scripts.length > 0 && (
                      <div className="flex flex-col gap-1">
                        <span className="text-[11px] text-ink-faint">
                          {t("deploy.scriptOrderHint")}
                        </span>
                        {scripts.map((name, index) => (
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
                              disabled={running || index === 0}
                              title={t("common.moveUp")}
                              onClick={() => moveScript(index, -1)}
                              className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink disabled:opacity-30"
                            >
                              <ChevronUp className="size-3.5" />
                            </button>
                            <button
                              type="button"
                              disabled={running || index === scripts.length - 1}
                              title={t("common.moveDown")}
                              onClick={() => moveScript(index, 1)}
                              className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink disabled:opacity-30"
                            >
                              <ChevronDown className="size-3.5" />
                            </button>
                            <button
                              type="button"
                              disabled={running}
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
                <label className="flex cursor-pointer items-center gap-2.5 text-xs text-ink">
                  <input
                    type="checkbox"
                    checked={uploadEnv}
                    onChange={(event) => setUploadEnv(event.target.checked)}
                    disabled={running}
                    className="size-3.5 accent-primary"
                  />
                  {t("deploy.uploadEnv")}
                </label>
                {uploadEnv && (
                  <div className="mt-3 flex flex-col gap-2">
                    <p className="text-[11px] leading-relaxed text-ink-faint">
                      {t("deploy.envFilesHint")}
                    </p>
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
                            disabled={running}
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
                            disabled={running}
                            title={t("common.delete")}
                            onClick={() => void removeEnvFile(index)}
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
                            disabled={running}
                            onChange={(event) => updateEnvRemote(index, event.target.value)}
                            onBlur={() => void persistEnvFiles(envFiles)}
                            onKeyDown={(event) => {
                              if (event.key === "Enter") event.currentTarget.blur();
                            }}
                            className="h-7! min-w-0 flex-1 text-[11px]!"
                          />
                        </div>
                      </div>
                    ))}
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={running}
                      icon={<Plus className="size-3.5" />}
                      onClick={() => void pickEnvFile()}
                    >
                      {t("deploy.addEnvFile")}
                    </Button>
                  </div>
                )}
              </div>

              <div className="flex items-stretch gap-2">
                <Button
                  className="flex-1"
                  size="lg"
                  loading={submitting || serverRunning}
                  disabled={!!resolveError}
                  icon={<Rocket className="size-4" />}
                  onClick={() => void handleDeploy()}
                >
                  {serverRunning ? t("deploy.deploying") : t("deploy.startDeploy")}
                </Button>
                {serverRunning && (
                  <Button
                    variant="secondary"
                    size="lg"
                    loading={cancelling}
                    disabled={!live?.recordId}
                    icon={<Square className="size-3.5" />}
                    onClick={() => {
                      setCancelling(true);
                      void cancelDeploy().finally(() => setCancelling(false));
                    }}
                  >
                    {t("deploy.cancel")}
                  </Button>
                )}
              </div>

              {settings.atomicRelease && (
                <p className="text-center text-[11px] leading-relaxed text-ink-faint">
                  {t("deploy.atomicHint")}
                </p>
              )}

              {selectedRepo && (
                <p className="truncate text-center text-[11px] text-ink-faint" title={selectedRepo.path}>
                  {selectedServer ? `${selectedServer.name} → ` : ""}
                  {targetDir || t("deploy.targetDirEmpty")}
                </p>
              )}
              </div>

              {deployTarget === "pages" && (
                <div className="flex flex-col gap-4">
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
                      <Field
                        label={t("pages.publishBranch")}
                        hint={t("pages.publishBranchHint")}
                      >
                        <SearchSelect
                          value={pagesDraft.publishBranch}
                          onChange={(value) =>
                            setPagesDraft({ ...pagesDraft, publishBranch: value })
                          }
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

                      <Field
                        label={t("pages.buildCommand")}
                        hint={t("pages.buildCommandHintGithub")}
                      >
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
                      <p className="text-[11px] leading-relaxed text-ink-faint">
                        {t("pages.githubNote")}
                      </p>
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

                      <Field
                        label={t("pages.projectName")}
                        required
                        hint={t("pages.projectNameHint")}
                      >
                        <Input
                          value={pagesDraft.projectName}
                          onChange={(event) =>
                            setPagesDraft({ ...pagesDraft, projectName: event.target.value })
                          }
                          placeholder="my-site"
                          disabled={running}
                        />
                      </Field>

                      <Field
                        label={t("pages.buildCommand")}
                        hint={t("pages.buildCommandHintCloudflare")}
                      >
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
                        <p className="text-[11px] leading-relaxed text-warn">
                          {t("pages.missingToken")}
                        </p>
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
              )}
            </div>
          </Card>

          {deployTarget === "server" && recentDeploys.length > 0 && (
            <Card className="overflow-hidden">
              <div className="border-b border-line px-5 py-3">
                <h2 className="text-xs font-semibold text-ink">{t("deploy.recent")}</h2>
              </div>
              <ul className="divide-y divide-line">
                {recentDeploys.map((record) => (
                  <RecentDeployRow
                    key={record.id}
                    record={record}
                    disabled={running}
                    onRedeploy={() => {
                      void redeploy(record.id).catch(() => {
                        // store 已提示错误
                      });
                    }}
                  />
                ))}
              </ul>
            </Card>
          )}

          {deployTarget === "pages" && recentPagesDeploys.length > 0 && (
            <Card className="overflow-hidden">
              <div className="border-b border-line px-5 py-3">
                <h2 className="text-xs font-semibold text-ink">{t("deploy.recent")}</h2>
              </div>
              <ul className="divide-y divide-line">
                {recentPagesDeploys.map((record) => (
                  <RecentPagesDeployRow
                    key={record.id}
                    record={record}
                    onView={() => navigate("/pages")}
                  />
                ))}
              </ul>
            </Card>
          )}
        </div>

        <div className="flex min-h-0 flex-col gap-5">
          <Card className="p-5">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <h2 className="flex items-center gap-2.5 text-sm font-semibold tracking-tight text-ink">
                  <span className="grid size-7 place-items-center rounded-md border border-brand-line bg-brand-soft">
                    <Terminal className="size-3.5 text-brand" />
                  </span>
                  {t("deploy.status")}
                </h2>
                <p className="mt-1 text-xs text-ink-dim">
                  {activeLive
                    ? activeRunning
                      ? t("deploy.statusRunning")
                      : activeLive.status === "success"
                        ? t("deploy.statusSuccess")
                        : t("deploy.statusFailed")
                    : t("deploy.statusIdle")}
                </p>
              </div>
              {activeLive && (
                <Badge className={cn("shrink-0 border", deployStatusClass(activeLive.status))}>
                  {activeRunning && (
                    <span className="size-1.5 animate-pulse rounded-full bg-current" />
                  )}
                  {deployStatusLabel(activeLive.status)}
                </Badge>
              )}
            </div>

            {activeRunning && deployTarget === "server" && (
              <div className="mt-4">
                <div className="h-1.5 overflow-hidden rounded-full bg-hover">
                  <div
                    className="h-full rounded-full bg-linear-to-r from-[var(--btn-primary-top)] to-[var(--btn-primary-bottom)] transition-all duration-300"
                    style={{ width: `${Math.max(live?.progress ?? 0, 3)}%` }}
                  />
                </div>
                <p className="mt-2 text-right text-[11px] text-ink-dim">{live?.progress ?? 0}%</p>
              </div>
            )}

            {deployTarget === "server" && live?.record && (
              <div
                className={cn(
                  "mt-4 flex flex-wrap items-center gap-3 rounded-md border px-4 py-3 text-xs",
                  live.record.status === "success"
                    ? "border-pos/30 bg-pos-soft text-pos"
                    : "border-neg/30 bg-neg-soft text-neg",
                )}
              >
                {live.record.status === "success" ? (
                  <CheckCircle2 className="size-4 shrink-0" />
                ) : (
                  <XCircle className="size-4 shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate">
                  {live.record.repoName} ·{" "}
                  {live.record.worktree ? t("deploy.worktree") : live.record.commitShort} ·{" "}
                  {live.record.serverName} · {formatDuration(live.record.durationMs)}
                </span>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => navigate(`/history?record=${live.record?.id ?? ""}`)}
                >
                  {t("deploy.viewRecord")}
                </Button>
              </div>
            )}

            {deployTarget === "pages" && livePages?.record && (
              <div
                className={cn(
                  "mt-4 flex flex-wrap items-center gap-3 rounded-md border px-4 py-3 text-xs",
                  livePages.record.status === "success"
                    ? "border-pos/30 bg-pos-soft text-pos"
                    : "border-neg/30 bg-neg-soft text-neg",
                )}
              >
                {livePages.record.status === "success" ? (
                  <CheckCircle2 className="size-4 shrink-0" />
                ) : (
                  <XCircle className="size-4 shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate">
                  {livePages.record.repoName} · {livePages.record.commitShort} ·{" "}
                  {livePages.record.projectName} ·{" "}
                  {formatDuration(livePages.record.durationMs)}
                </span>
                <Button size="sm" variant="secondary" onClick={() => navigate("/pages")}>
                  {t("deploy.viewRecord")}
                </Button>
              </div>
            )}

            {selectedRepo?.remote && (
              <p className="mt-4 flex items-center gap-1.5 text-[11px] text-ink-faint">
                <GitBranch className="size-3" />
                {shortPath(selectedRepo.remote, 72)}
              </p>
            )}
          </Card>

          <LogConsole
            lines={activeLive?.lines ?? []}
            className="h-[460px] xl:h-[520px]"
            emptyText={deployTarget === "pages" ? t("pages.logEmpty") : t("deploy.logEmpty")}
          />
        </div>
      </div>
    </Page>
  );
}

function RecentDeployRow({
  record,
  disabled,
  onRedeploy,
}: {
  record: DeployRecord;
  disabled: boolean;
  onRedeploy: () => void;
}) {
  const { t } = useTranslation();
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          record.status === "success"
            ? "bg-pos"
            : record.status === "failed"
              ? "bg-neg"
              : "bg-warn",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[11px] text-ink">
          {record.branch} · {record.worktree ? t("deploy.worktree") : record.commitShort}
        </p>
        <p className="mt-0.5 flex items-center gap-1 text-[10px] text-ink-faint">
          <Timer className="size-2.5" />
          {record.startedAt} · {record.serverName}
        </p>
      </div>
      <Button
        size="sm"
        variant="ghost"
        disabled={disabled || record.status === "running"}
        onClick={onRedeploy}
      >
        {t("history.redeploy")}
      </Button>
    </li>
  );
}

function RecentPagesDeployRow({
  record,
  onView,
}: {
  record: PagesDeployRecord;
  onView: () => void;
}) {
  const { t } = useTranslation();
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          record.status === "success"
            ? "bg-pos"
            : record.status === "failed"
              ? "bg-neg"
              : "bg-warn",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[11px] text-ink">
          {record.projectName} · {record.commitShort}
        </p>
        <p className="mt-0.5 flex items-center gap-1 text-[10px] text-ink-faint">
          <Timer className="size-2.5" />
          {record.startedAt} · {record.provider === "github" ? "GitHub" : "Cloudflare"}
        </p>
      </div>
      <Button size="sm" variant="ghost" onClick={onView}>
        {t("deploy.viewRecord")}
      </Button>
    </li>
  );
}
