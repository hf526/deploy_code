import i18n from "./i18n";

/** 与 deploy-core 的 `CRON_METHODS` 逐位对应：数组下标就是 cron-job.org 的 requestMethod 数值。 */
export const CRON_METHODS = [
  "GET",
  "POST",
  "OPTIONS",
  "HEAD",
  "PUT",
  "DELETE",
  "TRACE",
  "CONNECT",
  "PATCH",
];

/** 常用频率预设。选中后只是把表达式写进输入框，仍可继续手改成自定义 cron。 */
export const CRON_PRESETS: { labelKey: string; cron: string }[] = [
  { labelKey: "cronJobs.presetEvery5", cron: "*/5 * * * *" },
  { labelKey: "cronJobs.presetEvery15", cron: "*/15 * * * *" },
  { labelKey: "cronJobs.presetEvery30", cron: "*/30 * * * *" },
  { labelKey: "cronJobs.presetHourly", cron: "0 * * * *" },
  { labelKey: "cronJobs.presetDailyMidnight", cron: "0 0 * * *" },
  { labelKey: "cronJobs.presetDailyNine", cron: "0 9 * * *" },
  { labelKey: "cronJobs.presetWeeklyMondayNine", cron: "0 9 * * 1" },
];

export function cronMethodLabel(code: number): string {
  return CRON_METHODS[code] ?? String(code);
}

/**
 * 命中预设时给中文读法，否则原样返回表达式。
 * 故意不在前端再实现一遍 cron 解析：校验和归一化都在 Rust 侧，两处规则会漂移。
 */
export function cronSummary(cron: string): string {
  const normalized = cron.trim().replace(/\s+/g, " ");
  const preset = CRON_PRESETS.find((item) => item.cron === normalized);
  return preset ? i18n.t(preset.labelKey) : normalized;
}

/** unix 秒 → `YYYY-MM-DD HH:mm`（本地时区）；0 或负数表示从未执行 / 未排期。 */
export function formatUnixSeconds(secs: number): string {
  if (!Number.isFinite(secs) || secs <= 0) return "-";
  const date = new Date(secs * 1000);
  if (Number.isNaN(date.getTime())) return "-";
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}` +
    ` ${pad(date.getHours())}:${pad(date.getMinutes())}`
  );
}

/** 浏览器所在时区的 IANA 名称，取不到时回落 UTC（cron-job.org 只认 IANA 名称）。 */
export function localTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}
