import type { ReactNode } from "react";
import { ChevronDown, ChevronRight, Trash2 } from "lucide-react";

import { Button, SelectBox } from "./ui";

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
  deleteDisabled,
  log,
  selected,
  onSelect,
  selectionLabel,
  selectionDisabled,
}: {
  expanded: boolean;
  onToggle: () => void;
  title: ReactNode;
  subtitle: ReactNode;
  badge: ReactNode;
  actions?: ReactNode;
  onDelete: () => void;
  deleteTitle: string;
  /**
   * 正在跑的那条记录不许删：删掉它就等于把「这条任务还在跑」从盘上抹掉，
   * 对账找不到记录只能保持 running，发起按钮被挡住、停止按钮又解析不出 id。
   * 后端（`Store::guard_delete`）也会拒，这里只是别让用户点了才报错。
   */
  deleteDisabled?: boolean;
  log: ReactNode;
  /** 传入 onSelect 才渲染勾选框，未启用多选的列表保持原样。 */
  selected?: boolean;
  onSelect?: (on: boolean) => void;
  selectionLabel?: string;
  selectionDisabled?: boolean;
}) {
  return (
    <div>
      <div className="flex items-center gap-3 px-4 py-3">
        {onSelect && (
          <SelectBox
            checked={selected ?? false}
            disabled={selectionDisabled}
            label={selectionLabel ?? ""}
            onChange={onSelect}
          />
        )}
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
        <Button
          variant="ghost"
          size="sm"
          onClick={onDelete}
          disabled={deleteDisabled}
          title={deleteTitle}
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {expanded && log}
    </div>
  );
}
