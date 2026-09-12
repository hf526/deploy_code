import { useEffect, useRef, useState } from "react";
import { TerminalSquare } from "lucide-react";

import type { LogLine } from "../lib/types";
import { cn } from "../lib/utils";

const LEVEL_CLASS: Record<LogLine["level"], string> = {
  info: "text-ink-dim",
  command: "text-brand",
  success: "text-pos",
  warn: "text-warn",
  error: "text-neg",
};

export function LogConsole({
  lines,
  className,
  emptyText = "等待部署开始 ...",
}: {
  lines: LogLine[];
  className?: string;
  emptyText?: string;
}) {
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
        <span className="text-xs font-medium text-ink-dim">部署日志</span>
        {!autoScroll && (
          <button
            onClick={() => setAutoScroll(true)}
            className="ml-auto rounded border border-line px-1.5 py-0.5 text-[11px] text-ink-dim hover:bg-hover hover:text-ink"
          >
            恢复滚动
          </button>
        )}
      </div>

      <div
        ref={containerRef}
        onScroll={handleScroll}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2.5 font-mono text-[11.5px] leading-[1.7]"
      >
        {lines.length === 0 ? (
          <p className="text-ink-faint">{emptyText}</p>
        ) : (
          lines.map((line, index) => (
            <div key={index} className={cn("whitespace-pre-wrap break-all", LEVEL_CLASS[line.level])}>
              {line.message}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
