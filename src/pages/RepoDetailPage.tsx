import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowUpFromLine,
  Check,
  ChevronDown,
  CloudDownload,
  File,
  FileDiff,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  Inbox,
  Loader2,
  Plus,
  RefreshCw,
  Rocket,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { Trans, useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router-dom";

import { BindRemoteModal } from "../components/BindRemoteModal";
import { SearchSelect } from "../components/SearchSelect";
import { ConfirmModal } from "../components/ui";
import { Button, Input } from "../components/ui";
import { FileExplorer } from "../components/FileExplorer";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { Branch, RepoDetail } from "../lib/types";
import { registerUnsavedGuard, runGuarded } from "../lib/unsavedGuard";
import { useTauriEvent } from "../lib/useTauriEvent";
import { cn } from "../lib/utils";
import { EditorPane } from "./repoDetail/EditorPane";
import { changeDir, changeFile, CommitBox, RailButton, Section } from "./repoDetail/Panels";
import { SearchPanel } from "./repoDetail/SearchPanel";
import type { EditorView, Reveal } from "./repoDetail/editor";
import { useFileFilter } from "./repoDetail/useFileFilter";

type LeftView = "explorer" | "search" | "scm";

export default function RepoDetailPage() {
  const { t } = useTranslation();
  const { repoId = "" } = useParams();
  const navigate = useNavigate();
  const toast = useApp((state) => state.toast);
  const refreshRepos = useApp((state) => state.refreshRepos);
  const openTab = useApp((state) => state.openTab);

  const [detail, setDetail] = useState<RepoDetail | null>(null);
  const [branches, setBranches] = useState<Branch[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [treeKey, setTreeKey] = useState(0);

  const [editor, setEditor] = useState<EditorView>({ kind: "none" });
  const [leftView, setLeftView] = useState<LeftView>("explorer");
  const [reveal, setReveal] = useState<Reveal | null>(null);
  const [fsTick, setFsTick] = useState(0);
  const { fileFilter, setFileFilter, fileMatches, filterLoading } = useFileFilter(repoId, fsTick);
  const [commitMessage, setCommitMessage] = useState("");
  const [showNewBranch, setShowNewBranch] = useState(false);
  const [newBranchName, setNewBranchName] = useState("");
  const [newBranchFrom, setNewBranchFrom] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [branchMenuOpen, setBranchMenuOpen] = useState(false);
  const [branchSearch, setBranchSearch] = useState("");
  const [remoteOpen, setRemoteOpen] = useState(false);
  const [sensitiveFiles, setSensitiveFiles] = useState<string[] | null>(null);
  const branchMenuRef = useRef<HTMLDivElement>(null);

  const [pendingBranch, setPendingBranch] = useState<string | null>(null);
  // 切换文件 / 分支等会覆盖编辑器时，先确认是否放弃未保存的修改。
  const [pendingDiscard, setPendingDiscard] = useState<{ run: () => void } | null>(null);

  // 镜像当前仓库 id，异步响应回来时用于判断是否已切换仓库。
  const repoIdRef = useRef(repoId);
  repoIdRef.current = repoId;

  // 切换仓库时清空上一仓库的详情与编辑器，避免把旧仓库的文件内容保存到新仓库。
  useEffect(() => {
    setDetail(null);
    setBranches([]);
    setEditor({ kind: "none" });
    setReveal(null);
    setRemoteOpen(false);
    setSensitiveFiles(null);
    setPendingDiscard(null);
  }, [repoId]);

  const reload = useCallback(async (silent = false) => {
    if (!repoId) return;
    const targetRepoId = repoId;
    if (!silent) setLoading(true);
    try {
      const nextDetail = await api.repoDetail(repoId);
      if (targetRepoId !== repoIdRef.current) return;
      // 非 Git 文件夹没有分支（分支命令会报错），跳过；Git 仓库的分支错误仍照常抛出。
      const nextBranches = nextDetail.repo.isRepo
        ? await api.listBranches(repoId, true)
        : [];
      if (targetRepoId !== repoIdRef.current) return;
      setDetail(nextDetail);
      setBranches(nextBranches);
      if (!silent) setTreeKey((key) => key + 1);
    } catch (error) {
      if (targetRepoId === repoIdRef.current && !silent) toast("error", String(error));
    } finally {
      if (targetRepoId === repoIdRef.current && !silent) setLoading(false);
    }
  }, [repoId, toast]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 进入仓库工作区时登记到顶部标签栏，支持多仓库快速切换。
  useEffect(() => {
    if (repoId) openTab(repoId);
  }, [repoId, openTab]);

  useEffect(() => {
    if (!branchMenuOpen) return;
    const onClick = (event: MouseEvent) => {
      if (branchMenuRef.current && !branchMenuRef.current.contains(event.target as Node)) {
        setBranchMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [branchMenuOpen]);

  async function run(action: () => Promise<string>, after?: () => void) {
    setBusy(true);
    try {
      const message = await action();
      toast("success", message.trim() || t("repoDetail.operationOk"));
      after?.();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  // —— 外部文件变化实时刷新 ——
  const editorRef = useRef<EditorView>(editor);
  editorRef.current = editor;
  const lastSaveAt = useRef(0);
  const onFsChangeRef = useRef<() => void>(() => {});
  onFsChangeRef.current = () => {
    void reload(true);
    setFsTick((tick) => tick + 1);
    const ed = editorRef.current;
    if (
      ed.kind === "file" &&
      !ed.saving &&
      ed.content !== null &&
      ed.draft === ed.content &&
      Date.now() - lastSaveAt.current > 1500
    ) {
      // 编辑器中无未保存改动：静默重读该文件（不闪加载状态）。
      const targetRepoId = repoId;
      void api
        .readFile(targetRepoId, ed.path)
        .then((file) => {
          if (targetRepoId !== repoIdRef.current) return;
          setEditor((prev) =>
            prev.kind === "file" && prev.path === ed.path && prev.draft === prev.content
              ? {
                  ...prev,
                  content: file.content,
                  draft: file.content,
                  truncated: file.truncated,
                  conflict: false,
                }
              : prev,
          );
        })
        .catch(() => {});
    } else if (
      ed.kind === "file" &&
      !ed.saving &&
      ed.content !== null &&
      ed.draft !== null &&
      ed.draft !== ed.content
    ) {
      // 有未保存草稿时磁盘被外部改动：标记冲突，避免 Ctrl+S 静默覆盖新内容。
      const targetRepoId = repoId;
      void api
        .readFile(targetRepoId, ed.path)
        .then((file) => {
          if (targetRepoId !== repoIdRef.current) return;
          setEditor((prev) =>
            prev.kind === "file" && prev.path === ed.path && prev.content !== file.content
              ? { ...prev, conflict: true }
              : prev,
          );
        })
        .catch(() => {});
    }
  };

  const repoPath = detail?.repo.path ?? null;
  useEffect(() => {
    if (!repoPath) return;
    void api.watchRepo(repoId, repoPath).catch(() => {});
    return () => {
      void api.unwatchRepo(repoId).catch(() => {});
    };
  }, [repoId, repoPath]);

  useTauriEvent<{ repoId: string }>("repo://fs-changed", (payload) => {
    if (payload.repoId !== repoId) return;
    onFsChangeRef.current();
  });

  const editorDirty =
    editor.kind === "file" &&
    editor.content !== null &&
    editor.draft !== null &&
    editor.draft !== editor.content;
  // 确认流程里需要在同一事件循环内先“放弃修改”再执行动作，用 ref 避免闭包读到旧的 dirty 值。
  const editorDirtyRef = useRef(false);
  editorDirtyRef.current = editorDirty;

  /** 有未保存修改时先弹确认；确认后放弃修改再执行动作，避免静默丢失编辑内容。 */
  function guardUnsaved(action: () => void) {
    if (editorDirtyRef.current) {
      setPendingDiscard({ run: action });
      return;
    }
    action();
  }

  function confirmDiscard() {
    const action = pendingDiscard?.run;
    setPendingDiscard(null);
    editorDirtyRef.current = false;
    setEditor((prev) => (prev.kind === "file" ? { ...prev, draft: prev.content } : prev));
    action?.();
  }

  // 标签栏 / 侧边栏 / 打开仓库等全局跳转入口经 runGuarded 回调到本页的确认流程。
  const guardRef = useRef(guardUnsaved);
  guardRef.current = guardUnsaved;
  useEffect(() => {
    registerUnsavedGuard((action) => guardRef.current(action));
    return () => registerUnsavedGuard(null);
  }, []);

  function openDiff(path: string) {
    guardUnsaved(() => {
      const targetRepoId = repoId;
      setEditor({ kind: "diff", path, diff: null, error: null, loading: true });
      void api
        .fileDiff(targetRepoId, path)
        .then((text) => {
          if (targetRepoId !== repoIdRef.current) return;
          setEditor((prev) => (prev.kind === "diff" && prev.path === path ? { ...prev, diff: text, loading: false } : prev));
        })
        .catch((error) => {
          if (targetRepoId !== repoIdRef.current) return;
          setEditor((prev) => (prev.kind === "diff" && prev.path === path ? { ...prev, error: String(error), loading: false } : prev));
        });
    });
  }

  function openFileNow(path: string) {
    const targetRepoId = repoId;
    setEditor({
      kind: "file",
      path,
      content: null,
      draft: null,
      truncated: false,
      error: null,
      loading: true,
      saving: false,
      conflict: false,
    });
    void api
      .readFile(targetRepoId, path)
      .then((file) => {
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path
            ? {
                ...prev,
                content: file.content,
                draft: file.content,
                truncated: file.truncated,
                loading: false,
              }
            : prev,
        );
      })
      .catch((error) => {
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path
            ? { ...prev, error: String(error), loading: false }
            : prev,
        );
      });
  }

  function openFile(path: string) {
    // 已经打开同一文件时无需重读，避免覆盖正在编辑的草稿。
    if (editor.kind === "file" && editor.path === path && editor.content !== null) return;
    guardUnsaved(() => openFileNow(path));
  }

  function updateDraft(path: string, draft: string) {
    setEditor((prev) => (prev.kind === "file" && prev.path === path ? { ...prev, draft } : prev));
  }

  function openFileAtLine(path: string, line: number) {
    setLeftView("explorer");
    const same = editor.kind === "file" && editor.path === path && editor.content !== null;
    if (same) {
      setReveal({ path, line, ts: Date.now() });
      return;
    }
    // reveal 定位必须放进被确认后的动作里：取消弃改时不能残留跳转。
    guardUnsaved(() => {
      openFileNow(path);
      setReveal({ path, line, ts: Date.now() });
    });
  }

  function revertFile(path: string) {
    setEditor((prev) =>
      prev.kind === "file" && prev.path === path && prev.content !== null
        ? { ...prev, draft: prev.content }
        : prev,
    );
  }

  /** 冲突处理：用磁盘上的最新内容覆盖编辑器（放弃本地草稿）。 */
  function reloadFileFromDisk(path: string) {
    const targetRepoId = repoId;
    setEditor((prev) =>
      prev.kind === "file" && prev.path === path
        ? { ...prev, loading: true, error: null }
        : prev,
    );
    void api
      .readFile(targetRepoId, path)
      .then((file) => {
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path
            ? {
                ...prev,
                content: file.content,
                draft: file.content,
                truncated: file.truncated,
                loading: false,
                conflict: false,
              }
            : prev,
        );
      })
      .catch((error) => {
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path
            ? { ...prev, error: String(error), loading: false }
            : prev,
        );
      });
  }

  /** 冲突处理：保留本地草稿，清除冲突标记（保存时按用户意愿覆盖磁盘）。 */
  function keepDraft(path: string) {
    setEditor((prev) =>
      prev.kind === "file" && prev.path === path ? { ...prev, conflict: false } : prev,
    );
  }

  function saveFile(path: string) {
    if (editor.kind !== "file" || editor.path !== path) return;
    if (editor.content === null || editor.draft === null || editor.saving) return;
    if (editor.truncated) {
      toast("error", t("repoDetail.fileTooLargeSave"));
      return;
    }
    if (editor.draft === editor.content) return;
    const draft = editor.draft;
    const targetRepoId = repoId;
    lastSaveAt.current = Date.now();
    setEditor((prev) => (prev.kind === "file" && prev.path === path ? { ...prev, saving: true } : prev));
    void api
      .writeFile(targetRepoId, path, draft)
      .then(() => {
        lastSaveAt.current = Date.now();
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path
            ? { ...prev, content: draft, saving: false, conflict: false }
            : prev,
        );
        toast("success", t("repoDetail.saved", { path }));
        void refreshRepos();
        void reload(true);
      })
      .catch((error) => {
        if (targetRepoId !== repoIdRef.current) return;
        setEditor((prev) =>
          prev.kind === "file" && prev.path === path ? { ...prev, saving: false } : prev,
        );
        toast("error", String(error));
      });
  }

  async function scanSensitiveAndCommit() {
    const targetRepoId = repoId;
    setBusy(true);
    try {
      // 提交前扫描敏感文件，避免把 .env / 密钥带入远端历史。
      const files = await api.sensitiveChanges(targetRepoId);
      if (targetRepoId !== repoIdRef.current) return;
      if (files.length > 0) {
        setSensitiveFiles(files);
        return;
      }
    } catch (error) {
      if (targetRepoId !== repoIdRef.current) return;
      toast("error", String(error));
      return;
    } finally {
      setBusy(false);
    }
    commitNow(false);
  }

  function handleCommit() {
    if (!commitMessage.trim()) {
      toast("error", t("repoDetail.commitMessageRequired"));
      return;
    }
    // 先处理未保存修改，确认后再扫描敏感文件：两个弹窗不叠加，提交链路不中断。
    guardUnsaved(() => {
      void scanSensitiveAndCommit();
    });
  }

  /** 直接提交（调用前已完成未保存修改与敏感文件确认）。 */
  function commitNow(allowSensitive: boolean) {
    const targetRepoId = repoId;
    void run(
      () => api.commitChanges(targetRepoId, commitMessage.trim(), allowSensitive),
      () => {
        setCommitMessage("");
        setEditor({ kind: "none" });
        setSensitiveFiles(null);
        void reload();
        void refreshRepos();
      },
    );
  }

  async function handleSync() {
    await run(async () => {
      const pull = await api.pullRepo(repoId);
      const push = await api.pushRepo(repoId);
      return [pull, push].filter(Boolean).join("\n");
    }, reload);
  }

  function handleCreateBranch() {
    const name = newBranchName.trim();
    if (!name) {
      toast("error", t("repoDetail.branchNameRequired"));
      return;
    }
    guardUnsaved(() => {
      void run(() => api.createBranch(repoId, name, newBranchFrom || null, true), () => {
        setNewBranchName("");
        setNewBranchFrom("");
        setShowNewBranch(false);
        setEditor({ kind: "none" });
        void reload();
        void refreshRepos();
      });
    });
  }

  function handleCheckout(branch: string) {
    setBranchMenuOpen(false);
    if (branch === detail?.repo.currentBranch) return;
    guardUnsaved(() => {
      void run(() => api.checkoutBranch(repoId, branch), () => {
        setEditor({ kind: "none" });
        void reload();
        void refreshRepos();
      });
    });
  }

  function handleCheckoutRemote(remote: string) {
    const name = remote.replace(/^[^/]+\//, "");
    // 本地已有同名分支时直接切换，否则 checkout -b 会因分支已存在而必然失败。
    const existsLocally = branches.some((branch) => !branch.isRemote && branch.name === name);
    guardUnsaved(() => {
      void run(
        () =>
          existsLocally
            ? api.checkoutBranch(repoId, name)
            : api.createBranch(repoId, name, remote, true),
        () => {
          setEditor({ kind: "none" });
          void reload();
          void refreshRepos();
        },
      );
    });
  }

  async function handleConfirmDelete() {
    if (!pendingBranch) return;
    const branch = pendingBranch;
    await run(() => api.deleteBranch(repoId, branch, true), () => {
      setPendingBranch(null);
      void reload();
      void refreshRepos();
    });
  }

  function toggleSection(key: string) {
    setCollapsed((prev) => ({ ...prev, [key]: !prev[key] }));
  }

  if (!detail) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-ink-dim">
        {loading ? (
          <span className="flex items-center gap-2">
            <Loader2 className="size-4 animate-spin" /> {t("repoDetail.loadingRepo")}
          </span>
        ) : (
          t("repoDetail.notFound")
        )}
      </div>
    );
  }

  const status = detail.status;
  const repo = detail.repo;
  const currentBranch = repo.currentBranch;
  const localBranches = branches.filter((b) => !b.isRemote);
  const remoteBranches = branches.filter((b) => b.isRemote);
  const branchKeyword = branchSearch.trim().toLowerCase();
  const filteredLocal = branchKeyword
    ? localBranches.filter((branch) => branch.name.toLowerCase().includes(branchKeyword))
    : localBranches;
  const filteredRemote = branchKeyword
    ? remoteBranches.filter((branch) => branch.name.toLowerCase().includes(branchKeyword))
    : remoteBranches;

  return (
    <div className="flex h-full flex-col">
      {/* 顶部标题栏 */}
      <header className="ui-titlebar flex shrink-0 items-center gap-3 border-b border-line px-4 py-2.5">
        <Button
          size="sm"
          variant="ghost"
          icon={<ArrowLeft className="size-4" />}
          onClick={() => guardUnsaved(() => navigate("/repos?list=1"))}
        />
        <div className="min-w-0">
          <h1 className="truncate text-sm font-semibold tracking-tight text-ink">{repo.name}</h1>
          <p className="truncate text-[11px] text-ink-faint">{repo.path}</p>
          <p className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px]">
            {repo.remote ? (
              <span className="truncate font-mono text-ink-dim" title={repo.remote}>
                {repo.remote}
              </span>
            ) : (
              <span className="text-ink-faint">
                {repo.pathExists ? t("repos.noRemote") : t("repos.pathUnavailable")}
              </span>
            )}
            <button
              type="button"
              className="shrink-0 text-brand hover:underline"
              onClick={() => setRemoteOpen(true)}
            >
              {repo.remote ? t("repoDetail.edit") : t("repoDetail.bind")}
            </button>
          </p>
        </div>

        <div className="ml-auto flex items-center gap-2">
          {/* 分支选择器 */}
          <div className="relative" ref={branchMenuRef}>
            <button
              type="button"
              disabled={busy || !repo.isRepo}
              onClick={() => {
                setBranchSearch("");
                setBranchMenuOpen((open) => !open);
              }}
              className="ui-btn ui-btn-secondary flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium"
            >
              <GitBranch className="size-3.5 text-brand" />
              <span className="max-w-[10rem] truncate">
                {currentBranch === "HEAD" ? t("repoDetail.detachedHead") : currentBranch}
              </span>
              {status && (status.ahead > 0 || status.behind > 0) && (
                <span className="flex items-center gap-0.5 text-[10px] text-ink-dim">
                  {status.ahead > 0 && <span className="text-pos">↑{status.ahead}</span>}
                  {status.behind > 0 && <span className="text-warn">↓{status.behind}</span>}
                </span>
              )}
              <ChevronDown
                className={cn(
                  "size-3.5 text-ink-dim transition-transform",
                  branchMenuOpen && "rotate-180",
                )}
              />
            </button>

            {branchMenuOpen && (
              <div className="ui-pop absolute right-0 top-full z-30 mt-1 flex max-h-[60vh] w-64 flex-col overflow-hidden p-1.5">
                <Input
                  autoFocus
                  value={branchSearch}
                  onChange={(event) => setBranchSearch(event.target.value)}
                  placeholder={t("repoDetail.branchSearchPlaceholder")}
                  className="mb-1 h-7! text-xs!"
                />
                <div className="min-h-0 flex-1 overflow-y-auto">
                  {filteredLocal.length === 0 && filteredRemote.length === 0 && (
                    <p className="px-2 py-1.5 text-xs text-ink-faint">
                      {t("repoDetail.noMatchingBranches")}
                    </p>
                  )}
                  {filteredLocal.length > 0 && (
                    <p className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-ink-faint">
                      {t("repoDetail.localBranches")}
                    </p>
                  )}
                  {filteredLocal.map((branch) => (
                  <button
                    key={branch.name}
                    type="button"
                    onClick={() => handleCheckout(branch.name)}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-hover"
                  >
                    <Check
                      className={cn(
                        "size-3.5 shrink-0",
                        branch.name === currentBranch ? "text-pos" : "opacity-0",
                      )}
                    />
                    <span
                      className={cn(
                        "truncate",
                        branch.name === currentBranch
                          ? "font-medium text-ink"
                          : "text-ink",
                      )}
                    >
                      {branch.name}
                    </span>
                  </button>
                ))}
                {filteredRemote.length > 0 && (
                  <>
                    <p className="mt-1 px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-ink-faint">
                      {t("repoDetail.remoteBranches")}
                    </p>
                    {filteredRemote.map((branch) => (
                      <button
                        key={branch.name}
                        type="button"
                        onClick={() => handleCheckoutRemote(branch.name)}
                        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs text-ink-dim transition-colors hover:bg-hover hover:text-ink"
                      >
                        <CloudDownload className="size-3.5 shrink-0 text-ink-dim" />
                        <span className="truncate">{branch.name}</span>
                      </button>
                    ))}
                  </>
                )}
                </div>
              </div>
            )}
          </div>

          <Button
            size="sm"
            variant="ghost"
            disabled={busy || !repo.isRepo}
            icon={<RefreshCw className="size-4" />}
            title={t("repoDetail.fetchTitle")}
            onClick={() => void run(() => api.fetchRepo(repoId), reload)}
          />
          <Button
            size="sm"
            variant="ghost"
            disabled={busy || !repo.isRepo}
            icon={<ArrowUpFromLine className="size-4" />}
            title={t("repoDetail.pushTitle")}
            onClick={() => void run(() => api.pushRepo(repoId), reload)}
          />
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || !repo.isRepo}
            icon={<CloudDownload className="size-4" />}
            onClick={() => void run(() => api.pullRepo(repoId), reload)}
          >
            {t("repoDetail.pull")}
          </Button>
          <Button
            size="sm"
            icon={<Rocket className="size-4" />}
            onClick={() => runGuarded(() => navigate(`/deploy?repo=${repo.id}`))}
          >
            {t("nav.deploy")}
          </Button>
        </div>
      </header>

      {/* 主体：左活动栏 + 面板 + 右编辑器 */}
      <div className="flex min-h-0 flex-1">
        <nav className="ui-sidebar flex w-12 shrink-0 flex-col items-center gap-1 border-r border-line pt-2">
          <RailButton
            icon={<FolderTree className="size-5" />}
            label={t("repoDetail.railExplorer")}
            active={leftView === "explorer"}
            onClick={() => setLeftView("explorer")}
          />
          <RailButton
            icon={<Search className="size-5" />}
            label={t("repoDetail.search")}
            active={leftView === "search"}
            onClick={() => setLeftView("search")}
          />
          <RailButton
            icon={<GitCommitHorizontal className="size-5" />}
            label={t("repoDetail.railScm")}
            active={leftView === "scm"}
            badge={status?.changes.length ?? 0}
            onClick={() => setLeftView("scm")}
          />
        </nav>
        <aside className="ui-sidebar flex w-[300px] shrink-0 flex-col overflow-hidden border-r border-line">
          {leftView === "scm" ? (
            <div className="min-h-0 flex-1 overflow-y-auto">
          {/* 更改 */}
          <Section
            title={t("repoDetail.changes")}
            icon={<FileDiff className="size-3.5" />}
            count={status?.changes.length ?? 0}
            collapsed={!!collapsed.changes}
            onToggle={() => toggleSection("changes")}
          >
            {repo.isRepo ? (
              <CommitBox
                branch={currentBranch}
                status={status}
                message={commitMessage}
                setMessage={setCommitMessage}
                busy={busy}
                onCommit={handleCommit}
                onSync={handleSync}
              />
            ) : (
              <div className="flex flex-col gap-2 px-3 pb-2.5">
                <p className="text-[11px] leading-relaxed text-ink-faint">
                  {t("repoDetail.bindGitHint")}
                </p>
                <Button
                  size="sm"
                  variant="secondary"
                  className="self-start"
                  icon={<GitBranch className="size-3.5" />}
                  onClick={() => setRemoteOpen(true)}
                >
                  {t("remoteModal.bindTitle")}
                </Button>
              </div>
            )}
            {repo.isRepo && (!status || status.changes.length === 0 ? (
              <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-faint">
                <Inbox className="size-3.5" /> {t("repoDetail.noChanges")}
              </p>
            ) : (
              <ul className="pb-2">
                {status.changes.map((change, index) => {
                  const active =
                    editor.kind === "diff" && editor.path === change.path && !editor.loading;
                  return (
                    <li key={`${change.path}-${index}`}>
                      <button
                        type="button"
                        onClick={() => openDiff(change.path)}
                        title={t("repoDetail.diffHint", { path: change.path })}
                        className={cn(
                          "flex w-full items-center gap-2 px-3 py-1 text-left text-xs hover:bg-hover",
                          active && "bg-brand-soft text-ink",
                        )}
                      >
                        <span
                          className={cn(
                            "w-3 shrink-0 text-center font-mono text-[11px]",
                            change.code.trim() === "??"
                              ? "text-ink-faint"
                              : change.code.includes("D")
                                ? "text-neg"
                                : change.code.includes("A")
                                  ? "text-pos"
                                  : "text-brand",
                          )}
                        >
                          {change.code.trim().charAt(0) === "?" ? "U" : change.code.trim().charAt(0)}
                        </span>
                        <span className="min-w-0 flex-1 truncate font-mono">
                          {changeFile(change.path)}
                          <span className="ml-1.5 font-sans text-[10px] text-ink-faint">
                            {changeDir(change.path)}
                          </span>
                        </span>
                        {editor.kind === "diff" && editor.path === change.path && editor.loading && (
                          <Loader2 className="size-3 shrink-0 animate-spin text-ink-dim" />
                        )}
                      </button>
                    </li>
                  );
                })}
              </ul>
            ))}
          </Section>

          {/* 分支 */}
          <Section
            title={t("repoDetail.branches")}
            icon={<GitBranch className="size-3.5" />}
            count={localBranches.length}
            collapsed={!!collapsed.branches}
            onToggle={() => toggleSection("branches")}
            action={
              <button
                type="button"
                onClick={() => setShowNewBranch((value) => !value)}
                className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink"
                title={t("repoDetail.newBranch")}
              >
                <Plus className="size-3.5" />
              </button>
            }
          >
            {showNewBranch && (
              <div className="flex flex-col gap-2 border-b border-line px-3 py-2.5">
                <Input
                  autoFocus
                  value={newBranchName}
                  onChange={(event) => setNewBranchName(event.target.value)}
                  placeholder={t("repoDetail.newBranchPlaceholder")}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void handleCreateBranch();
                    if (event.key === "Escape") setShowNewBranch(false);
                  }}
                />
                <div className="flex items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <SearchSelect
                      value={newBranchFrom}
                      onChange={setNewBranchFrom}
                      options={[
                        { value: "", label: t("repoDetail.headCurrent") },
                        ...localBranches.map((branch) => ({
                          value: branch.name,
                          label: branch.name,
                          hint: branch.name === currentBranch ? t("deploy.current") : undefined,
                        })),
                      ]}
                      placeholder={t("repoDetail.headCurrent")}
                      className="text-xs!"
                    />
                  </div>
                  <Button size="sm" loading={busy} onClick={() => void handleCreateBranch()}>
                    {t("repoDetail.create")}
                  </Button>
                </div>
              </div>
            )}
            <ul className="pb-2">
              {localBranches.map((branch) => {
                const isCurrent = branch.name === currentBranch;
                return (
                  <li
                    key={branch.name}
                    className={cn(
                      "group/branch flex items-center gap-2 px-3 py-1 text-xs hover:bg-hover",
                      isCurrent && "bg-brand-soft",
                    )}
                  >
                    <GitBranch
                      className={cn(
                        "size-3.5 shrink-0",
                        isCurrent ? "text-brand" : "text-ink-faint",
                      )}
                    />
                    <button
                      type="button"
                      disabled={busy || isCurrent}
                      onClick={() => handleCheckout(branch.name)}
                      className={cn(
                        "min-w-0 flex-1 truncate text-left",
                        isCurrent ? "font-medium text-ink" : "text-ink",
                      )}
                      title={
                        isCurrent
                          ? t("repoDetail.currentBranchTitle")
                          : t("repoDetail.switchToBranch", { name: branch.name })
                      }
                    >
                      {branch.name}
                    </button>
                    {!isCurrent && (
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setPendingBranch(branch.name)}
                        className="shrink-0 rounded p-0.5 text-ink-dim opacity-0 transition-opacity hover:bg-neg-soft hover:text-neg group-hover/branch:opacity-100"
                        title={t("repoDetail.deleteBranch")}
                      >
                        <Trash2 className="size-3.5" />
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
          </Section>

          {/* 远程分支 */}
          {remoteBranches.length > 0 && (
            <Section
              title={t("repoDetail.remoteBranches")}
              icon={<CloudDownload className="size-3.5" />}
              count={remoteBranches.length}
              collapsed={!!collapsed.remote}
              onToggle={() => toggleSection("remote")}
            >
              <ul className="pb-2">
                {remoteBranches.map((branch) => (
                  <li
                    key={branch.name}
                    className="flex items-center gap-2 px-3 py-1 text-xs text-ink-dim hover:bg-hover"
                  >
                    <CloudDownload className="size-3.5 shrink-0 text-ink-dim" />
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => handleCheckoutRemote(branch.name)}
                      className="min-w-0 flex-1 truncate text-left"
                      title={t("commitGraph.checkout", { name: branch.name })}
                    >
                      {branch.name}
                    </button>
                  </li>
                ))}
              </ul>
            </Section>
          )}
            </div>
          ) : leftView === "search" ? (
            <SearchPanel
              repoId={repoId}
              onOpenHit={openFileAtLine}
              onReplaced={() => {
                void reload();
                void refreshRepos();
              }}
            />
          ) : (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex h-8 shrink-0 items-center gap-1.5 px-2.5">
                <FolderTree className="size-3.5 shrink-0 text-ink-dim" />
                <span className="text-xs font-semibold text-ink">{t("repoDetail.railExplorer")}</span>
                <button
                  type="button"
                  onClick={() => setTreeKey((key) => key + 1)}
                  className="ml-auto rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink"
                  title={t("repoDetail.refreshTree")}
                >
                  <RefreshCw className={cn("size-3.5", loading && "animate-spin")} />
                </button>
              </div>
              <div className="px-2 pb-1.5">
                <div className="relative">
                  <Search className="pointer-events-none absolute left-2 top-1/2 size-3 -translate-y-1/2 text-ink-faint" />
                  <input
                    value={fileFilter}
                    onChange={(event) => setFileFilter(event.target.value)}
                    placeholder={t("repoDetail.filterFilesPlaceholder")}
                    className="ui-input h-8 w-full rounded-lg pl-7 pr-6 text-xs text-ink placeholder:text-ink-faint"
                  />
                  {fileFilter && (
                    <button
                      type="button"
                      onClick={() => setFileFilter("")}
                      className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-ink-dim hover:text-ink"
                      title={t("common.clear")}
                    >
                      <X className="size-3" />
                    </button>
                  )}
                </div>
              </div>
              <div className="min-h-0 flex-1 overflow-y-auto">
                {fileFilter.trim() ? (
                  filterLoading && !fileMatches ? (
                    <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-dim">
                      <Loader2 className="size-3.5 animate-spin" /> {t("repoDetail.searching")}
                    </p>
                  ) : fileMatches && fileMatches.length > 0 ? (
                    <ul className="pb-2">
                      {fileMatches.map((item) => (
                        <li key={item}>
                          <button
                            type="button"
                            onClick={() => openFile(item)}
                            className="flex w-full items-center gap-1.5 px-3 py-1 text-left text-xs text-ink hover:bg-hover"
                            title={item}
                          >
                            <File className="size-3.5 shrink-0 text-ink-dim" />
                            <span className="shrink-0">{changeFile(item)}</span>
                            <span className="min-w-0 flex-1 truncate text-[10px] text-ink-faint">
                              {changeDir(item)}
                            </span>
                          </button>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="px-3 py-3 text-xs text-ink-faint">{t("repoDetail.noMatchingFiles")}</p>
                  )
                ) : (
                  <FileExplorer
                    repoId={repoId}
                    refreshKey={treeKey}
                    softKey={fsTick}
                    activePath={editor.kind === "file" && !editor.loading ? editor.path : null}
                    onSelectFile={(entry) => openFile(entry.path)}
                  />
                )}
              </div>
            </div>
          )}
        </aside>

        {/* 右侧编辑器区域 */}
        <section className="flex min-w-0 flex-1 flex-col">
          <EditorPane
            editor={editor}
            reveal={reveal}
            onClose={() => guardUnsaved(() => setEditor({ kind: "none" }))}
            onDraftChange={updateDraft}
            onSave={saveFile}
            onRevert={revertFile}
            onReload={reloadFileFromDisk}
            onKeepDraft={keepDraft}
            onRevealDone={() => setReveal(null)}
          />
        </section>
      </div>

      <ConfirmModal
        open={!!pendingBranch}
        danger
        loading={busy}
        title={t("repoDetail.deleteBranchTitle")}
        confirmText={t("common.delete")}
        description={
          <Trans
            i18nKey="repoDetail.deleteBranchDescription"
            values={{ name: pendingBranch }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setPendingBranch(null)}
        onConfirm={() => void handleConfirmDelete()}
      />

      <ConfirmModal
        open={!!pendingDiscard}
        danger
        title={t("repoDetail.discardTitle")}
        confirmText={t("repoDetail.discardConfirm")}
        description={t("repoDetail.discardDescription")}
        onCancel={() => setPendingDiscard(null)}
        onConfirm={confirmDiscard}
      />

      <ConfirmModal
        open={!!sensitiveFiles}
        danger
        loading={busy}
        title={t("repoDetail.sensitiveTitle")}
        confirmText={t("repoDetail.sensitiveConfirmText")}
        description={
          <div className="flex flex-col gap-2">
            <p>{t("repoDetail.sensitiveDescription")}</p>
            <ul className="max-h-44 overflow-y-auto rounded-md border border-line bg-panel px-3 py-2 font-mono text-[11px]">
              {(sensitiveFiles ?? []).map((file) => (
                <li key={file} className="truncate" title={file}>
                  {file}
                </li>
              ))}
            </ul>
          </div>
        }
        onCancel={() => setSensitiveFiles(null)}
        onConfirm={() => void commitNow(true)}
      />

      <BindRemoteModal
        repo={remoteOpen ? repo : null}
        onClose={() => setRemoteOpen(false)}
        onSaved={() => void reload()}
      />
    </div>
  );
}
