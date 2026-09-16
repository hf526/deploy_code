import type { ReactNode } from "react";
import { ChevronDown, ChevronRight, Trash2 } from "lucide-react";

import { Button } from "./ui";

/**
 * 任务记录日志：备份 / Pages / 部署历史共用的展示块。
 * 文案与错误前缀由调用方传入，避免在组件里硬编码 i18n key。
 */
export function RecordLog({
  error,
  errorPrefix,
  log,
  emptyText,
  className = "max-h-80 overflow-auto border-t border-line bg-sunken px-4 py-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-ink-dim",
}: {
  error?: string | null;
  errorPrefix?: (error: string) => string;
  log: string;
  emptyText: string;
  className?: string;
}) {
  return (
    <pre className={className}>
      {error && errorPrefix ? `${errorPrefix(error)}\n\n` : ""}
      {log || emptyText}
    </pre>
  );
}

/** 可展开的任务记录行：标题 + 副标题 + 状态徽章 + 操作按钮，展开后显示日志。 */
export function ExpandableRecordRow({
  expanded,
  onToggle,
  title,
  subtitle,
  badge,
  actions,
  onDelete,
  deleteTitle,
  log,
}: {
  expanded: boolean;
  onToggle: () => void;
  title: ReactNode;
  subtitle: ReactNode;
  badge: ReactNode;
  actions?: ReactNode;
  onDelete: () => void;
  deleteTitle: string;
  log: ReactNode;
}) {
  return (
    <div>
      <div className="flex items-center gap-3 px-4 py-3">
        <button className="flex min-w-0 flex-1 items-center gap-3 text-left" onClick={onToggle}>
          {expanded ? (
            <ChevronDown className="size-3.5 shrink-0 text-ink-faint" />
          ) : (
            <ChevronRight className="size-3.5 shrink-0 text-ink-faint" />
          )}
          <div className="min-w-0 flex-1">
            <p className="truncate text-[13px] font-medium text-ink">{title}</p>
            <p className="mt-0.5 truncate text-[11px] text-ink-faint">{subtitle}</p>
          </div>
          {badge}
        </button>
        {actions}
        <Button variant="ghost" size="sm" onClick={onDelete} title={deleteTitle}>
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {expanded && log}
    </div>
  );
}
