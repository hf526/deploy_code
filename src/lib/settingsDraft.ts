import { normalizeLanguagePreference } from "./i18n";
import type { Settings } from "./types";

/** 保存到后端的规范化：数值字段限幅、空值用默认值兜底。 */
export function normalizeSettings(source: Settings): Settings {
  // 0 是「不自动清理」的有效值，不能用 `|| 3` 兜底；只有填不进数字的才回落默认。
  const bundleKeep = Number(source.containerBundleKeep);
  return {
    ...source,
    scriptDir: source.scriptDir.trim() || "docker",
    connectTimeoutSecs: Math.max(3, Number(source.connectTimeoutSecs) || 15),
    scriptTimeoutSecs: Math.max(10, Number(source.scriptTimeoutSecs) || 1800),
    historyLimit: Math.max(20, Number(source.historyLimit) || 500),
    supabaseUrl: source.supabaseUrl.trim(),
    defaultBackupTargetId: source.defaultBackupTargetId || null,
    backupHistoryLimit: Math.max(10, Number(source.backupHistoryLimit) || 200),
    backupTimeoutSecs: Math.max(60, Number(source.backupTimeoutSecs) || 3600),
    cloudflareApiToken: source.cloudflareApiToken.trim(),
    cloudflareAccountId: source.cloudflareAccountId.trim(),
    githubToken: source.githubToken.trim(),
    cronjobApiKey: source.cronjobApiKey.trim(),
    pagesHistoryLimit: Math.max(10, Number(source.pagesHistoryLimit) || 200),
    containerHistoryLimit: Math.max(10, Number(source.containerHistoryLimit) || 200),
    containerTimeoutSecs: Math.max(300, Number(source.containerTimeoutSecs) || 7200),
    containerBundleKeep:
      Number.isFinite(bundleKeep) && bundleKeep >= 0 ? Math.min(999, Math.trunc(bundleKeep)) : 3,
    language: normalizeLanguagePreference(source.language),
    releaseKeep: Math.min(50, Math.max(1, Number(source.releaseKeep) || 5)),
    scheduledBackupTime: /^\d{1,2}:\d{2}$/.test(source.scheduledBackupTime.trim())
      ? source.scheduledBackupTime.trim()
      : "03:00",
    scheduledBackupConfigId: source.scheduledBackupConfigId || null,
    scheduledShutdownTime: /^\d{1,2}:\d{2}$/.test(source.scheduledShutdownTime.trim())
      ? source.scheduledShutdownTime.trim()
      : "04:00",
  };
}

/**
 * 只把本组字段写回整份设置，其余字段沿用已落盘的那份。
 * 后端 `save_settings` 是整表回写，所以「这一组保存了什么」只能在这里界定。
 */
export function mergedGroup(
  current: Settings,
  patch: Partial<Settings>,
  groupKeys: readonly (keyof Settings)[],
): Settings {
  const normalized = normalizeSettings({ ...current, ...patch });
  const next: Settings = { ...current };
  const target = next as Record<keyof Settings, Settings[keyof Settings]>;
  for (const key of groupKeys) {
    target[key] = normalized[key];
  }
  return next;
}

/**
 * 全局只留一条 `save_settings` 的车道。
 *
 * 整表回写没有「只改这几个字段」的语义：两次提交同时在途时，后落盘的那次用的是自己开工前的快照，
 * 会把前一次刚写下去的字段原样顶回去——用户看到的是「A 板块保存成功，B 板块的设置悄悄变回去了」。
 * 所以所有写设置的地方都得排队，而且排到自己时再取最新快照（调用方负责用 getState 而不是渲染作用域）。
 */
let lane: Promise<unknown> = Promise.resolve();

export function enqueueSettingsSave<T>(task: () => Promise<T>): Promise<T> {
  const run = lane.then(() => task());
  // 失败不能把车道堵死：下一次提交照样能排队，错误由调用方弹给用户。
  lane = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}
