import { GitBranch, Tag, Cloud } from "lucide-react";
import { useTranslation } from "react-i18next";

import { ROW_H, colX, laneColor, layoutGraph, rowY } from "../lib/graph";
import type { GraphCommit, GraphRef } from "../lib/types";
import { cn } from "../lib/utils";

function refPillStyle(ref: GraphRef): { wrapper: string; Icon: typeof GitBranch } {
  if (ref.isHead) return { wrapper: "border-pos/35 bg-pos-soft text-pos", Icon: GitBranch };
  switch (ref.kind) {
    case "remote":
      return { wrapper: "border-line bg-hover text-ink-dim", Icon: Cloud };
    case "tag":
      return { wrapper: "border-warn/35 bg-warn-soft text-warn", Icon: Tag };
    default:
      return { wrapper: "border-brand-line bg-brand-soft text-brand", Icon: GitBranch };
  }
}

export interface CommitGraphProps {
  commits: GraphCommit[];
  busy?: boolean;
  currentBranch?: string;
  onCheckout?: (branch: string) => void;
  className?: string;
}

export function CommitGraph({
  commits,
  busy,
  currentBranch,
  onCheckout,
  className,
}: CommitGraphProps) {
  const { t } = useTranslation();
  const layout = layoutGraph(commits);
  const { width, height, nodes, segments } = layout;

  return (
    <div className={cn("relative", className)} style={{ minHeight: height }}>
      <svg
        className="pointer-events-none absolute left-0 top-0"
        width={width}
        height={height}
        aria-hidden
      >
        {segments.map((seg, index) => {
          const x1 = colX(seg.col);
          const y1 = rowY(seg.gap);
          const x2 = colX(seg.toCol);
          const y2 = rowY(seg.gap + 1);
          const d = seg.curve
            ? `M ${x1} ${y1} L ${x1} ${y2 - ROW_H * 0.45} Q ${x1} ${y2} ${x2} ${y2}`
            : `M ${x1} ${y1} L ${x2} ${y2}`;
          return (
            <path
              key={`seg-${index}`}
              d={d}
              fill="none"
              style={{ stroke: seg.color }}
              strokeWidth={1.6}
              opacity={0.75}
            />
          );
        })}
        {nodes.map((node) => {
          const x = colX(node.col);
          const y = rowY(node.row);
          const color = laneColor(node.col);
          const isMerge = node.commit.parents.length > 1;
          const isHead = node.commit.refs.some((r) => r.isHead);
          if (isMerge) {
            const s = 5;
            return (
              <rect
                key={`dot-${node.commit.hash}`}
                x={x - s}
                y={y - s}
                width={s * 2}
                height={s * 2}
                transform={`rotate(45 ${x} ${y})`}
                style={{ fill: color, stroke: "var(--sunken)" }}
                strokeWidth={1.2}
              />
            );
          }
          return (
            <circle
              key={`dot-${node.commit.hash}`}
              cx={x}
              cy={y}
              r={isHead ? 5 : 4}
              style={{
                fill: isHead ? color : "var(--sunken)",
                stroke: color,
              }}
              strokeWidth={isHead ? 1.4 : 2}
            />
          );
        })}
      </svg>

      <div style={{ marginLeft: width }} className="flex flex-col">
        {nodes.map((node) => {
          const { commit } = node;
          const visibleRefs: Array<GraphRef & { key: string }> = commit.refs.map((ref, i) => ({
            ...ref,
            key: `${ref.kind}-${ref.name}-${i}`,
          }));
          const isCurrent =
            !!currentBranch && commit.refs.some((r) => r.isHead && r.kind === "local");
          return (
            <div
              key={commit.hash}
              className="group/row flex items-center gap-3 pr-3"
              style={{ height: ROW_H }}
            >
              <div className="flex min-w-0 flex-1 items-center gap-2">
                {visibleRefs.map((ref) => {
                  const { wrapper, Icon } = refPillStyle(ref);
                  const canCheckout =
                    !!onCheckout && ((ref.kind === "local" && !ref.isHead) || ref.kind === "remote");
                  return (
                    <button
                      key={ref.key}
                      type="button"
                      disabled={!canCheckout || busy}
                      onClick={() => onCheckout?.(ref.name)}
                      title={ref.kind === "remote" ? t("commitGraph.checkout", { name: ref.name }) : ref.name}
                      className={cn(
                        "inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-px text-[11px] font-medium transition",
                        wrapper,
                        canCheckout ? "hover:brightness-110" : "cursor-default",
                      )}
                    >
                      <Icon className="size-3" />
                      <span className="max-w-[10rem] truncate">{ref.name}</span>
                    </button>
                  );
                })}
                <span
                  className={cn(
                    "truncate text-[13px]",
                    isCurrent ? "font-medium text-ink" : "text-ink-dim",
                  )}
                  title={commit.subject}
                >
                  {commit.subject}
                </span>
              </div>

              <span className="w-40 shrink-0 truncate text-right text-[11px] text-ink-dim">
                {commit.author}
              </span>
              <span className="w-24 shrink-0 text-right font-mono text-[11px] text-ink-faint">
                {commit.short}
              </span>
              <span className="w-28 shrink-0 text-right text-[11px] text-ink-faint">
                {commit.date}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
