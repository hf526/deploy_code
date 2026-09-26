import type { ReactNode } from "react";
import { Check, ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "../../components/ui";
import type { RepoStatus } from "../../lib/types";
import { cn } from "../../lib/utils";

export function RailButton({
  icon,
  label,
  active,
  badge,
  disabled,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  active: boolean;
  badge?: number;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      className={cn(
        "relative flex size-10 items-center justify-center rounded-md transition-colors",
        active ? "text-ink" : "text-ink-dim hover:bg-hover hover:text-ink",
        disabled && "cursor-default text-ink-faint hover:bg-transparent",
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

export function Section({
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

/** 从仓库相对路径中取文件名。 */
export function changeFile(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] ?? path;
}

/** 从仓库相对路径中取所在目录（根目录返回空串）。 */
export function changeDir(path: string): string {
  const parts = path.split("/");
  if (parts.length <= 1) return "";
  return parts.slice(0, -1).join("/");
}

export function CommitBox({
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
  const { t } = useTranslation();
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
        placeholder={t("repoDetail.commitPlaceholder")}
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
          <span className="max-w-[7rem] truncate">
            {t("repoDetail.commit", { branch: branch === "HEAD" ? "HEAD" : branch })}
          </span>
        </Button>
        {(ahead > 0 || behind > 0) && (
          <Button
            size="sm"
            className="flex-1"
            disabled={busy}
            icon={<RefreshCw className="size-3.5" />}
            onClick={onSync}
            title={t("repoDetail.syncTitle")}
          >
            <span className="truncate">
              {t("repoDetail.syncChanges", {
                ahead: ahead > 0 ? `↑${ahead}` : "",
                behind: behind > 0 ? `↓${behind}` : "",
              })}
            </span>
          </Button>
        )}
      </div>
    </div>
  );
}
