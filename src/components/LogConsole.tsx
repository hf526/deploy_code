import { memo, useEffect, useRef, useState } from "react";
import { TerminalSquare } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { LogLine } from "../lib/types";
import { cn } from "../lib/utils";

const LEVEL_CLASS: Record<LogLine["level"], string> = {
  info: "text-ink-dim",
  command: "text-brand",
  success: "text-pos",
  warn: "text-warn",
  error: "text-neg",
};

/**
 * 行对象的稳定 key：日志数组会从头部裁剪，用下标做 key 会让所有行错位重渲染。
 * 这里按对象身份生成自增 id（WeakMap 不阻止行对象被回收）。
 */
const lineIds = new WeakMap<LogLine, number>();
let nextLineId = 1;
function lineKey(line: LogLine): number {
  let id = lineIds.get(line);
  if (id === undefined) {
    id = nextLineId++;
    lineIds.set(line, id);
  }
  return id;
}

/** 单行日志：memo + content-visibility 跳过屏外行的重渲染与布局，长日志不再卡顿。 */
const LogRow = memo(function LogRow({ line }: { line: LogLine }) {
  return (
    <div
      className={cn(
        "whitespace-pre-wrap break-all [contain-intrinsic-size:auto_20px] [content-visibility:auto]",
        LEVEL_CLASS[line.level],
      )}
    >
      {line.message}
    </div>
  );
});

export function LogConsole({
  lines,
  className,
  emptyText,
  title,
}: {
  lines: LogLine[];
  className?: string;
  emptyText?: string;
  title?: string;
}) {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  useEffect(() => {
    const element = containerRef.current;
    if (autoScroll && element) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lines, autoScroll]);

  function handleScroll() {
    const element = containerRef.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    setAutoScroll(distance < 48);
  }

  return (
    <div
      className={cn(
        "ui-card flex min-h-0 flex-col overflow-hidden bg-sunken",
        className,
      )}
    >
      <div className="flex shrink-0 items-center gap-2 border-b border-line bg-panel px-3 py-1.5">
        <TerminalSquare className="size-3.5 text-ink-dim" />
        <span className="text-xs font-medium text-ink-dim">{title ?? t("log.title")}</span>
        {!autoScroll && (
          <button
            onClick={() => setAutoScroll(true)}
            className="ml-auto rounded border border-line px-1.5 py-0.5 text-[11px] text-ink-dim hover:bg-hover hover:text-ink"
          >
            {t("log.resumeScroll")}
          </button>
        )}
      </div>

      <div
        ref={containerRef}
        onScroll={handleScroll}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2.5 font-mono text-[11.5px] leading-[1.7]"
      >
        {lines.length === 0 ? (
          <p className="text-ink-faint">{emptyText ?? t("log.waiting")}</p>
        ) : (
          lines.map((line) => <LogRow key={lineKey(line)} line={line} />)
        )}
      </div>
    </div>
  );
}
