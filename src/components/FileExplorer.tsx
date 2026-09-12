import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  ChevronDown,
  ChevronRight,
  File,
  Folder,
  FolderOpen,
  Loader2,
} from "lucide-react";

import { api } from "../lib/api";
import type { FileEntry } from "../lib/types";
import { cn } from "../lib/utils";

export function FileExplorer({
  repoId,
  refreshKey,
  softKey,
  activePath,
  onSelectFile,
}: {
  repoId: string;
  refreshKey: number;
  softKey?: number;
  activePath: string | null;
  onSelectFile: (entry: FileEntry) => void;
}) {
  const [root, setRoot] = useState<FileEntry[] | null>(null);
  const [children, setChildren] = useState<Record<string, FileEntry[]>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [dirErrors, setDirErrors] = useState<Record<string, string>>({});
  const expandedRef = useRef<Record<string, boolean>>({});
  expandedRef.current = expanded;

  const load = useCallback(
    (path: string) => api.listDir(repoId, path),
    [repoId],
  );

  useEffect(() => {
    let cancelled = false;
    setRoot(null);
    setChildren({});
    setExpanded({});
    setError(null);
    setDirErrors({});
    void load("")
      .then((entries) => {
        if (!cancelled) setRoot(entries);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [load, refreshKey]);

  // 外部文件变化：静默重载根目录与已展开目录，保留展开状态。
  const firstSoft = useRef(true);
  useEffect(() => {
    if (firstSoft.current) {
      firstSoft.current = false;
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const entries = await load("");
        if (cancelled) return;
        setRoot(entries);
        const openPaths = Object.keys(expandedRef.current).filter((p) => expandedRef.current[p]);
        const pairs = await Promise.all(
          openPaths.map(async (p) => [p, await load(p)] as const),
        );
        if (cancelled) return;
        setChildren((prev) => {
          const next = { ...prev };
          for (const [p, list] of pairs) next[p] = list;
          return next;
        });
        setError(null);
        setDirErrors({});
      } catch {
        /* 忽略瞬时错误，等待下一次变化 */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [load, softKey]);

  async function toggleDir(entry: FileEntry) {
    const open = !expanded[entry.path];
    setExpanded((prev) => ({ ...prev, [entry.path]: open }));
    // 出错后允许重新展开重试。
    if (open && (!children[entry.path] || dirErrors[entry.path])) {
      try {
        const entries = await load(entry.path);
        setChildren((prev) => ({ ...prev, [entry.path]: entries }));
        setDirErrors((prev) => {
          if (!(entry.path in prev)) return prev;
          const next = { ...prev };
          delete next[entry.path];
          return next;
        });
      } catch (err) {
        setChildren((prev) => ({ ...prev, [entry.path]: [] }));
        setDirErrors((prev) => ({ ...prev, [entry.path]: String(err) }));
      }
    }
  }

  function renderLevel(entries: FileEntry[], depth: number): ReactNode {
    return entries.map((entry) => {
      const active = !entry.isDir && entry.path === activePath;
      return (
        <div key={entry.path}>
          <button
            type="button"
            onClick={() => (entry.isDir ? void toggleDir(entry) : onSelectFile(entry))}
            className={cn(
              "flex w-full items-center gap-1.5 py-1 pr-2 text-left font-mono text-xs transition-colors hover:bg-hover",
              active ? "bg-chip text-ink" : "text-ink",
            )}
            style={{ paddingLeft: 8 + depth * 14 }}
            title={entry.path}
          >
            {entry.isDir ? (
              <>
                {expanded[entry.path] ? (
                  <ChevronDown className="size-3 shrink-0 text-ink-faint" />
                ) : (
                  <ChevronRight className="size-3 shrink-0 text-ink-faint" />
                )}
                {expanded[entry.path] ? (
                  <FolderOpen className="size-3.5 shrink-0 text-ink-dim" />
                ) : (
                  <Folder className="size-3.5 shrink-0 text-ink-dim" />
                )}
              </>
            ) : (
              <>
                <span className="w-3 shrink-0" />
                <File className="size-3.5 shrink-0 text-ink-faint" />
              </>
            )}
            <span className="truncate">{entry.name}</span>
          </button>
          {entry.isDir && expanded[entry.path] && (
            <div>
              {children[entry.path] ? renderLevel(children[entry.path], depth + 1) : null}
              {dirErrors[entry.path] && (
                <p
                  className="py-1 pr-2 text-[10px] text-neg"
                  style={{ paddingLeft: 22 + (depth + 1) * 14 }}
                >
                  {dirErrors[entry.path]}
                </p>
              )}
            </div>
          )}
        </div>
      );
    });
  }

  if (error) {
    return <p className="px-3 py-2 text-[11px] text-neg">{error}</p>;
  }
  if (!root) {
    return (
      <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-dim">
        <Loader2 className="size-3.5 animate-spin" /> 正在读取文件树 ...
      </p>
    );
  }
  if (root.length === 0) {
    return <p className="px-3 py-3 text-xs text-ink-faint">空目录</p>;
  }
  return <div className="pb-2">{renderLevel(root, 0)}</div>;
}
