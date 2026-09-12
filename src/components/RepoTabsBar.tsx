import { GitBranch, Plus, X } from "lucide-react";
import { useNavigate } from "react-router-dom";

import { openRepoFolder } from "../lib/openRepo";
import { useApp } from "../lib/store";
import { cn } from "../lib/utils";

/** 顶部仓库标签栏：打开多个仓库时可快速切换（类 IDE 编辑器标签）。 */
export function RepoTabsBar({ activeRepoId }: { activeRepoId: string }) {
  const tabs = useApp((state) => state.tabs);
  const repos = useApp((state) => state.repos);
  const closeTab = useApp((state) => state.closeTab);
  const navigate = useNavigate();

  const openRepos = tabs
    .map((id) => repos.find((repo) => repo.id === id))
    .filter((repo): repo is NonNullable<typeof repo> => !!repo);

  if (openRepos.length === 0) return null;

  function handleClose(repoId: string) {
    const rest = tabs.filter((id) => id !== repoId);
    closeTab(repoId);
    if (repoId !== activeRepoId) return;
    if (rest.length > 0) navigate(`/repos/${rest[rest.length - 1]}`);
    else navigate("/repos");
  }

  return (
    <div className="ui-titlebar flex h-9 shrink-0 items-center gap-1 border-b border-line px-2">
      <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto py-1">
        {openRepos.map((repo) => {
          const active = repo.id === activeRepoId;
          return (
            <div
              key={repo.id}
              role="tab"
              tabIndex={0}
              aria-selected={active}
              onClick={() => active || navigate(`/repos/${repo.id}`)}
              onKeyDown={(event) => {
                if (event.key === "Enter") navigate(`/repos/${repo.id}`);
              }}
              className={cn(
                "group/tab flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2.5 py-1 text-xs select-none transition-colors",
                active ? "bg-chip text-ink" : "text-ink-dim hover:bg-hover hover:text-ink",
              )}
            >
              <GitBranch className={cn("size-3.5", active ? "text-ink-dim" : "text-ink-faint")} />
              <span className="max-w-[10rem] truncate">{repo.name}</span>
              {repo.isRepo && repo.currentBranch && (
                <span className="hidden text-[10px] text-ink-faint group-hover/tab:inline">
                  {repo.currentBranch}
                </span>
              )}
              <button
                type="button"
                onClick={(event) => {
                  event.stopPropagation();
                  handleClose(repo.id);
                }}
                className={cn(
                  "rounded p-0.5 text-ink-faint hover:bg-hover hover:text-ink",
                  active ? "opacity-100" : "opacity-0 group-hover/tab:opacity-100",
                )}
                title="关闭标签"
              >
                <X className="size-3" />
              </button>
            </div>
          );
        })}
        <button
          type="button"
          onClick={() => void openRepoFolder(navigate)}
          className="grid size-6 shrink-0 place-items-center rounded-md text-ink-faint transition-colors hover:bg-hover hover:text-ink"
          title="打开仓库"
        >
          <Plus className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
