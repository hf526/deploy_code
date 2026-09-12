import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowLeft,
  ArrowUpFromLine,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CloudDownload,
  Columns2,
  File,
  FileDiff,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  Inbox,
  Loader2,
  Plus,
  RefreshCw,
  Replace,
  Rocket,
  Save,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";

import { ConfirmModal, EmptyState } from "../components/ui";
import { Button, Input, Select } from "../components/ui";
import { DiffView } from "../components/CodeView";
import { FileExplorer } from "../components/FileExplorer";
import { api } from "../lib/api";
import { highlightCode } from "../lib/highlight";
import { useApp } from "../lib/store";
import type { Branch, RepoDetail, RepoStatus, SearchHit } from "../lib/types";
import { cn } from "../lib/utils";

/** 编辑器行高（px），与 leading-[20px] 保持一致。 */
const LINE_H = 20;
/** 超过该字符数不做高亮/查找，避免卡顿。 */
const HEAVY_LIMIT = 60_000;

type LeftView = "explorer" | "search" | "scm";
type Reveal = { path: string; line: number; ts: number };

interface MatchRange {
  start: number;
  end: number;
}

function findMatches(text: string, query: string, caseSensitive: boolean): MatchRange[] {
  if (!query) return [];
  const ranges: MatchRange[] = [];
  if (caseSensitive) {
    let i = text.indexOf(query);
    while (i !== -1) {
      ranges.push({ start: i, end: i + query.length });
      i = text.indexOf(query, i + query.length);
    }
    return ranges;
  }
  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  let i = lowerText.indexOf(lowerQuery);
  while (i !== -1) {
    const end = i + query.length;
    // 大小写折叠可能改变长度，校验切片确实匹配才接受该位置。
    if (text.slice(i, end).toLowerCase() === lowerQuery) {
      ranges.push({ start: i, end });
    }
    i = lowerText.indexOf(lowerQuery, i + Math.max(query.length, 1));
  }
  return ranges;
}

function offsetToLine(text: string, offset: number): number {
  let line = 0;
  for (let i = 0; i < offset && i < text.length; i++) {
    if (text.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function replaceAt(text: string, start: number, end: number, value: string): string {
  return text.slice(0, start) + value + text.slice(end);
}

type EditorView =
  | { kind: "none" }
  | {
      kind: "file";
      path: string;
      content: string | null;
      draft: string | null;
      truncated: boolean;
      error: string | null;
      loading: boolean;
      saving: boolean;
    }
  | { kind: "diff"; path: string; diff: string | null; error: string | null; loading: boolean };

export default function RepoDetailPage() {
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
  const [fileFilter, setFileFilter] = useState("");
  const [fileMatches, setFileMatches] = useState<string[] | null>(null);
  const [filterLoading, setFilterLoading] = useState(false);
  const [fsTick, setFsTick] = useState(0);
  const [commitMessage, setCommitMessage] = useState("");
  const [showNewBranch, setShowNewBranch] = useState(false);
  const [newBranchName, setNewBranchName] = useState("");
  const [newBranchFrom, setNewBranchFrom] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [branchMenuOpen, setBranchMenuOpen] = useState(false);
  const branchMenuRef = useRef<HTMLDivElement>(null);

  const [pendingBranch, setPendingBranch] = useState<string | null>(null);

  // 镜像当前仓库 id，异步响应回来时用于判断是否已切换仓库。
  const repoIdRef = useRef(repoId);
  repoIdRef.current = repoId;

  // 切换仓库时清空上一仓库的详情与编辑器，避免把旧仓库的文件内容保存到新仓库。
  useEffect(() => {
    setDetail(null);
    setBranches([]);
    setEditor({ kind: "none" });
    setReveal(null);
    setFileMatches(null);
  }, [repoId]);

  const reload = useCallback(async (silent = false) => {
    if (!repoId) return;
    const targetRepoId = repoId;
    if (!silent) setLoading(true);
    try {
      const [nextDetail, nextBranches] = await Promise.all([
        api.repoDetail(repoId),
        api.listBranches(repoId, true),
      ]);
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

  useEffect(() => {
    const query = fileFilter.trim();
    if (!query) {
      setFileMatches(null);
      setFilterLoading(false);
      return;
    }
    let cancelled = false;
    setFilterLoading(true);
    const timer = window.setTimeout(() => {
      void api
        .findFiles(repoId, query)
        .then((files) => {
          if (!cancelled) setFileMatches(files);
        })
        .catch(() => {
          if (!cancelled) setFileMatches([]);
        })
        .finally(() => {
          if (!cancelled) setFilterLoading(false);
        });
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [fileFilter, repoId, fsTick]);

  async function run(action: () => Promise<string>, after?: () => void) {
    setBusy(true);
    try {
      const message = await action();
      toast("success", message.trim() || "操作成功");
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
              ? { ...prev, content: file.content, draft: file.content, truncated: file.truncated }
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

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ repoId: string }>("repo://fs-changed", (event) => {
      if (disposed || event.payload.repoId !== repoId) return;
      onFsChangeRef.current();
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [repoId]);

  function openDiff(path: string) {
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
  }

  function openFile(path: string) {
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

  function updateDraft(path: string, draft: string) {
    setEditor((prev) => (prev.kind === "file" && prev.path === path ? { ...prev, draft } : prev));
  }

  function openFileAtLine(path: string, line: number) {
    setLeftView("explorer");
    const same = editor.kind === "file" && editor.path === path && editor.content !== null;
    if (!same) openFile(path);
    setReveal({ path, line, ts: Date.now() });
  }

  function revertFile(path: string) {
    setEditor((prev) =>
      prev.kind === "file" && prev.path === path && prev.content !== null
        ? { ...prev, draft: prev.content }
        : prev,
    );
  }

  function saveFile(path: string) {
    if (editor.kind !== "file" || editor.path !== path) return;
    if (editor.content === null || editor.draft === null || editor.saving) return;
    if (editor.truncated) {
      toast("error", "文件过大仅显示部分内容，禁止保存以免丢失数据");
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
            ? { ...prev, content: draft, saving: false }
            : prev,
        );
        toast("success", `已保存 ${path}`);
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

  async function handleCommit() {
    if (!commitMessage.trim()) {
      toast("error", "请输入提交信息");
      return;
    }
    await run(() => api.commitChanges(repoId, commitMessage.trim()), () => {
      setCommitMessage("");
      setEditor({ kind: "none" });
      void reload();
      void refreshRepos();
    });
  }

  async function handleSync() {
    await run(async () => {
      const pull = await api.pullRepo(repoId);
      const push = await api.pushRepo(repoId);
      return [pull, push].filter(Boolean).join("\n");
    }, reload);
  }

  async function handleCreateBranch() {
    const name = newBranchName.trim();
    if (!name) {
      toast("error", "请输入分支名称");
      return;
    }
    await run(() => api.createBranch(repoId, name, newBranchFrom || null, true), () => {
      setNewBranchName("");
      setNewBranchFrom("");
      setShowNewBranch(false);
      void reload();
      void refreshRepos();
    });
  }

  function handleCheckout(branch: string) {
    setBranchMenuOpen(false);
    if (branch === detail?.repo.currentBranch) return;
    void run(() => api.checkoutBranch(repoId, branch), () => {
      setEditor({ kind: "none" });
      void reload();
      void refreshRepos();
    });
  }

  function handleCheckoutRemote(remote: string) {
    const name = remote.replace(/^[^/]+\//, "");
    // 本地已有同名分支时直接切换，否则 checkout -b 会因分支已存在而必然失败。
    const existsLocally = branches.some((branch) => !branch.isRemote && branch.name === name);
    void run(
      () =>
        existsLocally
          ? api.checkoutBranch(repoId, name)
          : api.createBranch(repoId, name, remote, true),
      () => {
        void reload();
        void refreshRepos();
      },
    );
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
            <Loader2 className="size-4 animate-spin" /> 正在读取仓库信息 ...
          </span>
        ) : (
          "仓库不存在"
        )}
      </div>
    );
  }

  const status = detail.status;
  const repo = detail.repo;
  const currentBranch = repo.currentBranch;
  const localBranches = branches.filter((b) => !b.isRemote);
  const remoteBranches = branches.filter((b) => b.isRemote);

  return (
    <div className="flex h-full flex-col">
      {/* 顶部标题栏 */}
      <header className="ui-titlebar flex shrink-0 items-center gap-3 border-b border-line px-4 py-2.5">
        <Button
          size="sm"
          variant="ghost"
          icon={<ArrowLeft className="size-4" />}
          onClick={() => navigate("/repos?list=1")}
        />
        <div className="min-w-0">
          <h1 className="truncate text-sm font-semibold tracking-tight text-ink">{repo.name}</h1>
          <p className="truncate text-[11px] text-ink-faint">{repo.path}</p>
        </div>

        <div className="ml-auto flex items-center gap-2">
          {/* 分支选择器 */}
          <div className="relative" ref={branchMenuRef}>
            <button
              type="button"
              disabled={busy}
              onClick={() => setBranchMenuOpen((open) => !open)}
              className="ui-btn ui-btn-secondary flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium"
            >
              <GitBranch className="size-3.5 text-brand" />
              <span className="max-w-[10rem] truncate">
                {currentBranch === "HEAD" ? "分离头指针" : currentBranch}
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
              <div className="ui-pop absolute right-0 top-full z-30 mt-1 max-h-[60vh] w-64 overflow-y-auto p-1.5">
                <p className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-ink-faint">
                  本地分支
                </p>
                {localBranches.map((branch) => (
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
                {remoteBranches.length > 0 && (
                  <>
                    <p className="mt-1 px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-ink-faint">
                      远程分支
                    </p>
                    {remoteBranches.map((branch) => (
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
            )}
          </div>

          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            icon={<RefreshCw className="size-4" />}
            title="拉取远端 (fetch)"
            onClick={() => void run(() => api.fetchRepo(repoId), reload)}
          />
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            icon={<ArrowUpFromLine className="size-4" />}
            title="推送 (push)"
            onClick={() => void run(() => api.pushRepo(repoId), reload)}
          />
          <Button
            size="sm"
            variant="secondary"
            disabled={busy}
            icon={<CloudDownload className="size-4" />}
            onClick={() => void run(() => api.pullRepo(repoId), reload)}
          >
            拉取
          </Button>
          <Button
            size="sm"
            icon={<Rocket className="size-4" />}
            onClick={() => navigate(`/deploy?repo=${repo.id}`)}
          >
            部署
          </Button>
        </div>
      </header>

      {/* 主体：左活动栏 + 面板 + 右编辑器 */}
      <div className="flex min-h-0 flex-1">
        <nav className="ui-sidebar flex w-12 shrink-0 flex-col items-center gap-1 border-r border-line pt-2">
          <RailButton
            icon={<FolderTree className="size-5" />}
            label="资源管理器"
            active={leftView === "explorer"}
            onClick={() => setLeftView("explorer")}
          />
          <RailButton
            icon={<Search className="size-5" />}
            label="搜索"
            active={leftView === "search"}
            onClick={() => setLeftView("search")}
          />
          <RailButton
            icon={<GitCommitHorizontal className="size-5" />}
            label="源代码管理"
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
            title="更改"
            icon={<FileDiff className="size-3.5" />}
            count={status?.changes.length ?? 0}
            collapsed={!!collapsed.changes}
            onToggle={() => toggleSection("changes")}
          >
            <CommitBox
              branch={currentBranch}
              status={status}
              message={commitMessage}
              setMessage={setCommitMessage}
              busy={busy}
              onCommit={handleCommit}
              onSync={handleSync}
            />
            {!status || status.changes.length === 0 ? (
              <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-faint">
                <Inbox className="size-3.5" /> 没有未提交的改动
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
                        title={`${change.path}（点击查看差异）`}
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
            )}
          </Section>

          {/* 分支 */}
          <Section
            title="分支"
            icon={<GitBranch className="size-3.5" />}
            count={localBranches.length}
            collapsed={!!collapsed.branches}
            onToggle={() => toggleSection("branches")}
            action={
              <button
                type="button"
                onClick={() => setShowNewBranch((value) => !value)}
                className="rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink"
                title="新建分支"
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
                  placeholder="分支名称，如 feature/login"
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void handleCreateBranch();
                    if (event.key === "Escape") setShowNewBranch(false);
                  }}
                />
                <div className="flex items-center gap-2">
                  <Select
                    value={newBranchFrom}
                    onChange={(event) => setNewBranchFrom(event.target.value)}
                    style={{ height: 32, fontSize: 12 }}
                  >
                    <option value="">HEAD</option>
                    {localBranches.map((branch) => (
                      <option key={branch.name} value={branch.name}>
                        {branch.name}
                      </option>
                    ))}
                  </Select>
                  <Button size="sm" loading={busy} onClick={() => void handleCreateBranch()}>
                    创建
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
                      title={isCurrent ? "当前分支" : `切换到 ${branch.name}`}
                    >
                      {branch.name}
                    </button>
                    {!isCurrent && (
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setPendingBranch(branch.name)}
                        className="shrink-0 rounded p-0.5 text-ink-dim opacity-0 transition-opacity hover:bg-neg-soft hover:text-neg group-hover/branch:opacity-100"
                        title="删除分支"
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
              title="远程分支"
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
                      title={`检出 ${branch.name}`}
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
                <span className="text-xs font-semibold text-ink">资源管理器</span>
                <button
                  type="button"
                  onClick={() => setTreeKey((key) => key + 1)}
                  className="ml-auto rounded p-0.5 text-ink-dim hover:bg-hover hover:text-ink"
                  title="刷新文件树"
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
                    placeholder="按名称搜索文件…"
                    className="ui-input h-8 w-full rounded-lg pl-7 pr-6 text-xs text-ink placeholder:text-ink-faint"
                  />
                  {fileFilter && (
                    <button
                      type="button"
                      onClick={() => setFileFilter("")}
                      className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-ink-dim hover:text-ink"
                      title="清空"
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
                      <Loader2 className="size-3.5 animate-spin" /> 正在搜索 ...
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
                    <p className="px-3 py-3 text-xs text-ink-faint">无匹配文件</p>
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
            onClose={() => setEditor({ kind: "none" })}
            onDraftChange={updateDraft}
            onSave={saveFile}
            onRevert={revertFile}
            onRevealDone={() => setReveal(null)}
          />
        </section>
      </div>

      <ConfirmModal
        open={!!pendingBranch}
        danger
        loading={busy}
        title="删除分支"
        confirmText="删除"
        description={
          <span>
            确定删除分支 <b className="text-ink">{pendingBranch}</b>
            吗？未合并的提交将无法通过该分支找回。
          </span>
        }
        onCancel={() => setPendingBranch(null)}
        onConfirm={() => void handleConfirmDelete()}
      />
    </div>
  );
}

function EditorPane({
  editor,
  reveal,
  onClose,
  onDraftChange,
  onSave,
  onRevert,
  onRevealDone,
}: {
  editor: EditorView;
  reveal: Reveal | null;
  onClose: () => void;
  onDraftChange: (path: string, draft: string) => void;
  onSave: (path: string) => void;
  onRevert: (path: string) => void;
  onRevealDone: () => void;
}) {
  if (editor.kind === "none") {
    return (
      <div className="flex flex-1 items-center justify-center p-8">
        <EmptyState
          icon={<Columns2 className="size-5" />}
          title="编辑器"
          description="从左侧「资源管理器」打开文件进行查看与编辑，或在「源代码管理」中点击改动文件查看差异。"
        />
      </div>
    );
  }

  const isDiff = editor.kind === "diff";
  const isFile = editor.kind === "file";
  const dirty =
    isFile && editor.content !== null && editor.draft !== null && editor.draft !== editor.content;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* 标签页头 */}
      <div className="ui-titlebar flex h-9 shrink-0 items-center gap-2 border-b border-line pl-3">
        {isDiff ? (
          <FileDiff className="size-3.5 shrink-0 text-brand" />
        ) : (
          <File className="size-3.5 shrink-0 text-ink-dim" />
        )}
        <span className="truncate font-mono text-xs text-ink">{editor.path}</span>
        {dirty && (
          <span className="shrink-0 rounded-full bg-warn-soft px-1.5 text-[10px] text-warn">
            未保存
          </span>
        )}
        {editor.loading && <Loader2 className="size-3.5 shrink-0 animate-spin text-ink-dim" />}

        {isFile && !editor.loading && !editor.error && (
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            {dirty && (
              <Button size="sm" variant="ghost" onClick={() => onRevert(editor.path)}>
                还原
              </Button>
            )}
            <Button
              size="sm"
              disabled={!dirty || editor.saving}
              loading={editor.saving}
              icon={<Save className="size-3.5" />}
              onClick={() => onSave(editor.path)}
              title="保存 (Ctrl+S)"
            >
              保存
            </Button>
          </div>
        )}
        <button
          type="button"
          onClick={onClose}
          className={cn(
            "rounded p-1 text-ink-dim hover:bg-hover hover:text-ink",
            isFile && !editor.loading && !editor.error ? "" : "ml-auto",
            "mr-1",
          )}
          title="关闭"
        >
          <X className="size-3.5" />
        </button>
      </div>

      {/* 内容 */}
      {editor.error ? (
        <div className="min-h-0 flex-1 overflow-auto bg-sunken">
          <p className="m-4 rounded-md border border-neg/30 bg-neg-soft px-4 py-3 text-xs text-neg">
            {editor.error}
          </p>
        </div>
      ) : editor.loading ? (
        <div className="flex min-h-0 flex-1 items-center justify-center gap-2 bg-sunken text-sm text-ink-dim">
          <Loader2 className="size-4 animate-spin" /> 正在加载 ...
        </div>
      ) : isDiff ? (
        <div className="min-h-0 flex-1 overflow-auto bg-sunken">
          {editor.diff ? (
            <DiffView diff={editor.diff} />
          ) : (
            <p className="px-4 py-16 text-center text-xs text-ink-faint">没有可显示的差异</p>
          )}
        </div>
      ) : isFile && editor.draft !== null ? (
        <FileEditorView
          key={editor.path}
          path={editor.path}
          draft={editor.draft}
          truncated={editor.truncated}
          reveal={reveal}
          onDraftChange={onDraftChange}
          onSave={onSave}
          onRevealDone={onRevealDone}
        />
      ) : null}
    </div>
  );
}

function FileEditorView({
  path,
  draft,
  truncated,
  reveal,
  onDraftChange,
  onSave,
  onRevealDone,
}: {
  path: string;
  draft: string;
  truncated: boolean;
  reveal: Reveal | null;
  onDraftChange: (path: string, draft: string) => void;
  onSave: (path: string) => void;
  onRevealDone: () => void;
}) {
  const taRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [showReplace, setShowReplace] = useState(false);
  const [replaceValue, setReplaceValue] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [activeIdx, setActiveIdx] = useState(0);

  const heavy = draft.length > HEAVY_LIMIT;
  const highlighted = useMemo(
    () => (!heavy ? highlightCode(draft, path) : null),
    [draft, path, heavy],
  );
  const useOverlay = !!highlighted;

  const matches = useMemo(
    () => (findOpen && findQuery && !heavy ? findMatches(draft, findQuery, caseSensitive) : []),
    [draft, findQuery, caseSensitive, findOpen, heavy],
  );
  const matchesRef = useRef<MatchRange[]>([]);
  matchesRef.current = matches;
  const safeIdx = matches.length > 0 ? Math.min(activeIdx, matches.length - 1) : 0;
  const current = matches[safeIdx];

  useEffect(() => {
    setActiveIdx(0);
  }, [findQuery, caseSensitive]);

  function syncScroll(scrollTop: number, scrollLeft: number) {
    if (preRef.current) {
      preRef.current.scrollTop = scrollTop;
      preRef.current.scrollLeft = scrollLeft;
    }
    if (gutterRef.current) gutterRef.current.scrollTop = scrollTop;
  }

  function gotoLine(line0: number) {
    const ta = taRef.current;
    if (!ta) return;
    const top = line0 * LINE_H;
    if (top < ta.scrollTop || top > ta.scrollTop + ta.clientHeight - LINE_H * 2) {
      ta.scrollTop = Math.max(0, top - ta.clientHeight / 3);
      syncScroll(ta.scrollTop, ta.scrollLeft);
    }
  }

  function selectMatch(m: MatchRange | undefined) {
    const ta = taRef.current;
    if (!ta || !m) return;
    ta.focus();
    ta.setSelectionRange(m.start, m.end);
    gotoLine(offsetToLine(draft, m.start));
  }

  useEffect(() => {
    if (!findOpen) return;
    selectMatch(matchesRef.current[Math.min(activeIdx, Math.max(matchesRef.current.length - 1, 0))]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findQuery, caseSensitive, activeIdx, findOpen]);

  // 从搜索结果跳转：选中并滚动到目标行。
  const revealedTs = useRef(0);
  useEffect(() => {
    if (!reveal || reveal.path !== path || reveal.ts === revealedTs.current) return;
    revealedTs.current = reveal.ts;
    const lines = draft.split("\n");
    const target = Math.max(0, Math.min(reveal.line - 1, lines.length - 1));
    const start = lines.slice(0, target).reduce((acc, l) => acc + l.length + 1, 0);
    const end = start + (lines[target]?.length ?? 0);
    const ta = taRef.current;
    if (ta) {
      ta.focus();
      ta.setSelectionRange(start, end);
      ta.scrollTop = Math.max(0, target * LINE_H - ta.clientHeight / 3);
      syncScroll(ta.scrollTop, ta.scrollLeft);
    }
    onRevealDone();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reveal, draft, path]);

  function step(delta: number) {
    if (matches.length === 0) return;
    const next = (activeIdx + delta + matches.length) % matches.length;
    setActiveIdx(next);
  }

  function replaceCurrent() {
    if (!current) return;
    onDraftChange(path, replaceAt(draft, current.start, current.end, replaceValue));
  }

  function replaceAll() {
    if (matches.length === 0) return;
    let next = "";
    let last = 0;
    for (const m of matches) {
      next += draft.slice(last, m.start) + replaceValue;
      last = m.end;
    }
    next += draft.slice(last);
    onDraftChange(path, next);
  }

  const lineCount = Math.max(draft.split("\n").length, 1);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-sunken">
      {truncated && (
        <p className="shrink-0 border-b border-warn/30 bg-warn-soft px-4 py-1.5 text-[11px] text-warn">
          文件较大，仅显示前 2MB 内容（禁止保存以免丢失数据）
        </p>
      )}
      {findOpen && (
        <div className="ui-titlebar flex h-9 shrink-0 items-center gap-1.5 border-b border-line px-2.5">
          <input
            autoFocus
            value={findQuery}
            onChange={(event) => setFindQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                step(event.shiftKey ? -1 : 1);
              }
              if (event.key === "Escape") setFindOpen(false);
            }}
            placeholder="在文件中查找"
            className="ui-input h-6 w-44 rounded px-2 text-[11px] text-ink placeholder:text-ink-faint"
          />
          <span className="w-14 shrink-0 text-[10px] text-ink-dim">
            {findQuery ? (matches.length ? `${safeIdx + 1}/${matches.length}` : "无结果") : ""}
          </span>
          <button
            type="button"
            onClick={() => step(-1)}
            className="rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title="上一个 (Shift+Enter)"
          >
            <ChevronUp className="size-3" />
          </button>
          <button
            type="button"
            onClick={() => step(1)}
            className="rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title="下一个 (Enter)"
          >
            <ChevronDown className="size-3" />
          </button>
          <button
            type="button"
            onClick={() => setCaseSensitive((value) => !value)}
            className={cn(
              "h-6 w-6 shrink-0 rounded border text-[10px] font-semibold",
              caseSensitive
                ? "border-brand-line bg-brand-soft text-brand"
                : "border-transparent text-ink-dim hover:bg-hover",
            )}
            title="区分大小写"
          >
            Aa
          </button>
          <button
            type="button"
            onClick={() => setShowReplace((value) => !value)}
            className={cn(
              "rounded p-1",
              showReplace ? "text-brand" : "text-ink-dim hover:bg-hover hover:text-ink",
            )}
            title="替换"
          >
            <Replace className="size-3.5" />
          </button>
          {showReplace && (
            <>
              <input
                value={replaceValue}
                onChange={(event) => setReplaceValue(event.target.value)}
                placeholder="替换为"
                className="ui-input h-6 w-32 rounded px-2 text-[11px] text-ink placeholder:text-ink-faint"
              />
              <button
                type="button"
                onClick={replaceCurrent}
                disabled={!current}
                className="h-6 rounded border border-line px-1.5 text-[10px] text-ink hover:bg-hover disabled:opacity-40"
                title="替换当前"
              >
                替换
              </button>
              <button
                type="button"
                onClick={replaceAll}
                disabled={matches.length === 0}
                className="h-6 rounded border border-line px-1.5 text-[10px] text-ink hover:bg-hover disabled:opacity-40"
                title="替换全部"
              >
                全部
              </button>
            </>
          )}
          <button
            type="button"
            onClick={() => setFindOpen(false)}
            className="ml-auto rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title="关闭 (Esc)"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div
          ref={gutterRef}
          className="w-11 shrink-0 select-none overflow-hidden border-r border-line bg-sunken pt-3 text-right font-mono text-[10px] leading-[20px] text-ink-faint"
          aria-hidden
        >
          {Array.from({ length: lineCount }, (_, i) => (
            <div key={i} className="pr-2">
              {i + 1}
            </div>
          ))}
        </div>
        <div className="relative min-w-0 flex-1">
          {useOverlay && (
            <pre
              ref={preRef}
              aria-hidden
              className="hljs pointer-events-none absolute inset-0 overflow-hidden whitespace-pre px-3 pt-3 font-mono text-[11.5px] leading-[20px] [tab-size:4]"
            >
              <code dangerouslySetInnerHTML={{ __html: `${highlighted}\n` }} />
            </pre>
          )}
          <textarea
            ref={taRef}
            value={draft}
            readOnly={truncated}
            spellCheck={false}
            wrap="off"
            onChange={(event) => onDraftChange(path, event.target.value)}
            onScroll={(event) => syncScroll(event.currentTarget.scrollTop, event.currentTarget.scrollLeft)}
            onKeyDown={(event) => {
              if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
                event.preventDefault();
                onSave(path);
              }
              if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
                event.preventDefault();
                setFindOpen(true);
              }
              if (event.key === "F3") {
                event.preventDefault();
                step(event.shiftKey ? -1 : 1);
              }
            }}
            className={cn(
              "absolute inset-0 h-full w-full resize-none overflow-auto bg-transparent px-3 pt-3 font-mono text-[11.5px] leading-[20px] caret-ink outline-none [tab-size:4]",
              useOverlay
                ? "text-transparent selection:bg-brand-soft"
                : "text-ink selection:bg-brand-soft",
            )}
          />
        </div>
      </div>
    </div>
  );
}

function RailButton({
  icon,
  label,
  active,
  badge,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  active: boolean;
  badge?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      className={cn(
        "relative flex size-10 items-center justify-center rounded-md transition-colors",
        active ? "text-ink" : "text-ink-dim hover:bg-hover hover:text-ink",
      )}
    >
      {icon}
      {active && (
        <span className="absolute -right-2 bottom-2 top-2 w-0.5 rounded-full bg-brand" />
      )}
      {!!badge && badge > 0 && (
        <span className="absolute right-0.5 top-0.5 rounded-full bg-brand-soft px-1 text-[9px] font-medium leading-3.5 text-brand">
          {badge}
        </span>
      )}
    </button>
  );
}

function SearchPanel({
  repoId,
  onOpenHit,
  onReplaced,
}: {
  repoId: string;
  onOpenHit: (path: string, line: number) => void;
  onReplaced: () => void;
}) {
  const toast = useApp((state) => state.toast);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [showReplace, setShowReplace] = useState(false);
  const [replacement, setReplacement] = useState("");
  const [replacing, setReplacing] = useState(false);

  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      setHits(null);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const timer = window.setTimeout(() => {
      void api
        .searchContent(repoId, trimmed, caseSensitive)
        .then((result) => {
          if (!cancelled) setHits(result);
        })
        .catch((error) => {
          if (!cancelled) {
            setHits([]);
            toast("error", String(error));
          }
        })
        .finally(() => {
          if (!cancelled) setSearching(false);
        });
    }, 260);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, caseSensitive, repoId, toast]);

  const grouped = useMemo(() => {
    const map = new Map<string, SearchHit[]>();
    for (const hit of hits ?? []) {
      const list = map.get(hit.path);
      if (list) list.push(hit);
      else map.set(hit.path, [hit]);
    }
    return Array.from(map.entries());
  }, [hits]);

  const matchCount = hits?.length ?? 0;
  const truncated = matchCount >= 800;

  async function handleReplace() {
    if (!grouped.length || replacing) return;
    setReplacing(true);
    try {
      const summary = await api.replaceContent(
        repoId,
        query.trim(),
        replacement,
        grouped.map(([path]) => path),
        caseSensitive,
      );
      toast("success", `已替换 ${summary.matchesReplaced} 处，涉及 ${summary.filesReplaced} 个文件`);
      onReplaced();
      const next = await api.searchContent(repoId, query.trim(), caseSensitive);
      setHits(next);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setReplacing(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-1.5 px-2.5">
        <Search className="size-3.5 shrink-0 text-ink-dim" />
        <span className="text-xs font-semibold text-ink">搜索</span>
        <button
          type="button"
          onClick={() => setShowReplace((value) => !value)}
          className={cn(
            "ml-auto rounded p-0.5 transition-colors",
            showReplace ? "text-brand" : "text-ink-dim hover:text-ink",
          )}
          title="显示替换"
        >
          <Replace className="size-3.5" />
        </button>
      </div>

      <div className="flex flex-col gap-2 px-2 pb-2">
        <div className="flex items-center gap-1.5">
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索全部内容…"
            className="ui-input h-7 min-w-0 flex-1 rounded-md px-2 text-xs text-ink placeholder:text-ink-faint"
          />
          <button
            type="button"
            onClick={() => setCaseSensitive((value) => !value)}
            className={cn(
              "h-7 w-7 shrink-0 rounded-md border text-[11px] font-semibold transition-colors",
              caseSensitive
                ? "border-brand-line bg-brand-soft text-brand"
                : "border-line text-ink-dim hover:text-ink",
            )}
            title="区分大小写"
          >
            Aa
          </button>
        </div>
        {showReplace && (
          <div className="flex items-center gap-1.5">
            <input
              value={replacement}
              onChange={(event) => setReplacement(event.target.value)}
              placeholder="替换为…"
              className="ui-input h-7 min-w-0 flex-1 rounded-md px-2 text-xs text-ink placeholder:text-ink-faint"
            />
            <Button
              size="sm"
              variant="secondary"
              className="h-7 shrink-0"
              loading={replacing}
              disabled={matchCount === 0}
              onClick={() => void handleReplace()}
            >
              全部替换
            </Button>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto border-t border-line">
        {searching ? (
          <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-dim">
            <Loader2 className="size-3.5 animate-spin" /> 正在搜索 ...
          </p>
        ) : !query.trim() ? (
          <p className="px-3 py-3 text-xs text-ink-faint">输入关键字搜索仓库内容。</p>
        ) : matchCount === 0 ? (
          <p className="px-3 py-3 text-xs text-ink-faint">没有匹配结果。</p>
        ) : (
          <>
            <p className="px-3 py-1.5 text-[11px] text-ink-dim">
              {matchCount} 处匹配，{grouped.length} 个文件
              {truncated && "（结果过多已截断）"}
            </p>
            {grouped.map(([path, list]) => (
              <div key={path} className="mb-1">
                <div className="flex items-center gap-1.5 px-3 py-1 text-[11px] text-ink-dim">
                  <File className="size-3 shrink-0 text-ink-dim" />
                  <span className="truncate" title={path}>
                    {path}
                  </span>
                  <span className="ml-auto shrink-0 rounded bg-hover px-1.5 text-[10px]">
                    {list.length}
                  </span>
                </div>
                <ul>
                  {list.slice(0, 60).map((hit) => (
                    <li key={`${hit.path}:${hit.line}`}>
                      <button
                        type="button"
                        onClick={() => onOpenHit(hit.path, hit.line)}
                        className="flex w-full items-baseline gap-2 py-0.5 pl-6 pr-2 text-left hover:bg-hover"
                        title={hit.text}
                      >
                        <span className="w-8 shrink-0 text-right font-mono text-[10px] text-ink-faint">
                          {hit.line}
                        </span>
                        <span className="truncate font-mono text-[11px] text-ink">
                          {hit.text}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function changeFile(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] ?? path;
}

function changeDir(path: string): string {
  const parts = path.split("/");
  if (parts.length <= 1) return "";
  return parts.slice(0, -1).join("/");
}

function Section({
  title,
  icon,
  count,
  collapsed,
  onToggle,
  action,
  children,
}: {
  title: string;
  icon: ReactNode;
  count?: number;
  collapsed: boolean;
  onToggle: () => void;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="border-b border-line">
      <div className="flex items-center gap-1 px-2 py-1.5">
        <button
          type="button"
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded text-left"
        >
          {collapsed ? (
            <ChevronRight className="size-3.5 shrink-0 text-ink-dim" />
          ) : (
            <ChevronDown className="size-3.5 shrink-0 text-ink-dim" />
          )}
          <span className="shrink-0 text-ink-dim">{icon}</span>
          <span className="truncate text-xs font-semibold text-ink">{title}</span>
          {count !== undefined && count > 0 && (
            <span className="rounded bg-hover px-1.5 text-[10px] text-ink-dim">{count}</span>
          )}
        </button>
        {action}
      </div>
      {!collapsed && children}
    </div>
  );
}

function CommitBox({
  branch,
  status,
  message,
  setMessage,
  busy,
  onCommit,
  onSync,
}: {
  branch: string;
  status: RepoStatus | null;
  message: string;
  setMessage: (value: string) => void;
  busy: boolean;
  onCommit: () => void;
  onSync: () => void;
}) {
  const ahead = status?.ahead ?? 0;
  const behind = status?.behind ?? 0;
  return (
    <div className="flex flex-col gap-2 px-3 pb-2.5 pt-0.5">
      <textarea
        value={message}
        onChange={(event) => setMessage(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
            event.preventDefault();
            onCommit();
          }
        }}
        rows={2}
        placeholder="提交变更内容..."
        className="ui-input w-full resize-none rounded-md px-2.5 py-1.5 text-xs text-ink placeholder:text-ink-faint"
      />
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="secondary"
          className="flex-1"
          disabled={!message.trim() || busy}
          icon={<Check className="size-3.5" />}
          onClick={onCommit}
        >
          <span className="max-w-[7rem] truncate">提交 {branch === "HEAD" ? "HEAD" : branch}</span>
        </Button>
        {(ahead > 0 || behind > 0) && (
          <Button
            size="sm"
            className="flex-1"
            disabled={busy}
            icon={<RefreshCw className="size-3.5" />}
            onClick={onSync}
            title="拉取并推送"
          >
            <span className="truncate">
              同步更改 {ahead > 0 ? `↑${ahead}` : ""}
              {behind > 0 ? `↓${behind}` : ""}
            </span>
          </Button>
        )}
      </div>
    </div>
  );
}
