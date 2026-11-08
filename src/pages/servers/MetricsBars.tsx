import { useTranslation } from "react-i18next";

import {
  CPU_WARN_PERCENT,
  MEMORY_WARN_PERCENT,
  memoryPercent,
  usageTone,
  usageToneClass,
  type MetricsView,
} from "../../lib/metrics";
import { humanSize } from "../../lib/utils";

/**
 * 一台机器的占用条：CPU 与内存各一条，越线换色。
 *
 * 三态各画各的——有数字画条；明确采不到画「未取到」并把原因挂在悬停上；还没采到画「采集中」。
 * 退回上一次的数字或干脆什么都不写都不行：常驻采集下，一个停在原地的百分比会被读成实时值。
 */
export function MetricsBars({ view }: { view: MetricsView }) {
  const { t } = useTranslation();

  if (view.kind === "failed") {
    return (
      <p className="truncate text-[11px] text-neg" title={view.reason}>
        {t("servers.metricsFailed")}
      </p>
    );
  }

  if (view.kind === "pending") {
    return (
      <p className="truncate text-[11px] text-ink-faint">{t("servers.metricsPending")}</p>
    );
  }

  const { metrics } = view;
  const memory = memoryPercent(metrics);
  // 条上只放百分比，绝对量（内存用了多少 / 一共多少、几个核）挂在悬停上：
  // 一眼扫的是「哪台不对劲」，要细看才点数字。
  const cpuTip =
    metrics.cpuCores > 0
      ? `${t("servers.metricsCpu", { percent: metrics.cpuPercent })} · ${t("servers.metricsCores", { cores: metrics.cpuCores })}`
      : t("servers.metricsCpu", { percent: metrics.cpuPercent });
  return (
    <div className="mt-1 flex flex-col gap-1">
      <Bar label="CPU" percent={metrics.cpuPercent} warnAt={CPU_WARN_PERCENT} title={cpuTip} />
      <Bar
        label="MEM"
        percent={memory}
        warnAt={MEMORY_WARN_PERCENT}
        title={t("servers.metricsMemory", {
          used: humanSize(metrics.memoryUsedBytes),
          total: humanSize(metrics.memoryTotalBytes),
          percent: memory,
        })}
      />
    </div>
  );
}

function Bar({
  label,
  percent,
  warnAt,
  title,
}: {
  label: string;
  percent: number;
  warnAt: number;
  title?: string;
}) {
  const tone = usageTone(percent, warnAt);
  const width = Math.max(0, Math.min(100, percent));
  return (
    <div className="flex items-center gap-1.5" title={title}>
      <span className="w-6 shrink-0 text-[10px] font-medium tracking-wide text-ink-faint select-none">
        {label}
      </span>
      <span className="h-1 min-w-0 flex-1 overflow-hidden rounded-full bg-line-strong">
        <span
          className={`block h-full rounded-full transition-[width] duration-300 ${usageToneClass[tone]}`}
          style={{ width: `${width}%` }}
        />
      </span>
      <span className="w-8 shrink-0 text-right font-mono text-[10px] text-ink-dim tabular-nums">
        {width}%
      </span>
    </div>
  );
}
