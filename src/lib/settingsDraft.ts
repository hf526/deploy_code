import { api } from "./api";
import { normalizeLanguagePreference } from "./i18n";
import { useApp } from "./store";
import type { Settings } from "./types";

/**
 * 每一个 Settings 字段属于哪一块界面 —— **由编译器强制维护**。
 *
 * 这张表本身没有运行时用途，它存在的唯一理由是那个 `Record<keyof Settings, GroupName>`
 * 标注：`Settings` 里新增一个字段而这里没登记时，`npm run build` 的 `tsc` 步骤直接报错。
 * 在此之前，「新字段有没有落进某个板块的 groupKeys」只能靠人记，已经漏过一次 ——
 * `dbBundleKeep`（数据库导出包保留数）至今没有任何卡片能写它，GUI 用户够不着这个功能。
 *
 * 新增字段时的三步：① 这里登记归属；② 该板块的 `*_KEYS` 里加上它（要能提交）；
 * ③ 需要限幅 / 兜底的写进 `normalizeSettings`。
 */
export type SettingsGroupName =
  | "appearance"
  | "deployDefaults"
  | "backupTargets"
  | "containerParams"
  | "scheduledBackup"
  | "scheduledContainer"
  | "shutdownMachine"
  | "credentials"
  | "storage"
  | "internal";

export const SETTINGS_FIELD_GROUPS: Record<keyof Settings, SettingsGroupName> = {
  language: "appearance",
  scriptDir: "deployDefaults",
  runScripts: "deployDefaults",
  connectTimeoutSecs: "deployDefaults",
  scriptTimeoutSecs: "deployDefaults",
  keepRemoteArchive: "deployDefaults",
  historyLimit: "deployDefaults",
  atomicRelease: "deployDefaults",
  releaseKeep: "deployDefaults",
  pagesHistoryLimit: "deployDefaults",
  backupHistoryLimit: "backupTargets",
  backupTimeoutSecs: "backupTargets",
  defaultBackupTargetId: "backupTargets",
  supabaseUrl: "backupTargets",
  containerHistoryLimit: "containerParams",
  containerTimeoutSecs: "containerParams",
  containerBundleKeep: "containerParams",
  // 还没有任何界面写它：后端默认 0（不落本机导出包）。登记成 storage 是待办标记，
  // 要么补进 StorageSection，要么明确「只给 CLI 用」。
  dbBundleKeep: "storage",
  scheduledBackupEnabled: "scheduledBackup",
  scheduledBackupTime: "scheduledBackup",
  scheduledBackupConfigId: "scheduledBackup",
  scheduledContainerEnabled: "scheduledContainer",
  scheduledContainerTime: "scheduledContainer",
  scheduledContainerConfigIds: "scheduledContainer",
  scheduledShutdownEnabled: "shutdownMachine",
  scheduledShutdownTime: "shutdownMachine",
  cloudflareApiToken: "credentials",
  cloudflareAccountId: "credentials",
  githubToken: "credentials",
  cronjobApiKey: "credentials",
  // 三条调度循环各自「已触发」的调度日期：由后端写入，界面只读不写。
  scheduledBackupLastRun: "internal",
  scheduledShutdownLastRun: "internal",
  scheduledContainerLastRun: "internal",
  // 主密码哈希：没有界面入口，但参与导出抹除与导入保留两条不变量。
  masterPasswordHash: "internal",
};

/**
 * 三条定时时间的共同兜底：`<Input type="time">` 正常送不出非法值，但旧数据 / 手改持久化
 * 文件可以，`99:99` 这种「格式对、数值越界」的串必须拦在落盘前，回落各自板块的默认点。
 */
function normalizeScheduledTime(value: string, fallback: string): string {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  const hours = match ? Number(match[1]) : -1;
  const minutes = match ? Number(match[2]) : -1;
  return hours >= 0 && hours <= 23 && minutes >= 0 && minutes <= 59 ? value.trim() : fallback;
}

/** 保存到后端的规范化：数值字段限幅、空值用默认值兜底。 */
export function normalizeSettings(source: Settings): Settings {
  // 0 是「不自动清理」的有效值，不能用 `|| 2` 兜底；只有填不进数字的才回落默认。
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
      Number.isFinite(bundleKeep) && bundleKeep >= 0 ? Math.min(999, Math.trunc(bundleKeep)) : 2,
    language: normalizeLanguagePreference(source.language),
    releaseKeep: Math.min(50, Math.max(1, Number(source.releaseKeep) || 5)),
    scheduledBackupTime: normalizeScheduledTime(source.scheduledBackupTime, "03:00"),
    scheduledBackupConfigId: source.scheduledBackupConfigId || null,
    scheduledShutdownTime: normalizeScheduledTime(source.scheduledShutdownTime, "04:00"),
    scheduledContainerTime: normalizeScheduledTime(source.scheduledContainerTime, "03:30"),
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

/**
 * 模块页上「即时保存」的控件（定时备份开关 / 时间 / 配置选择、容器定时）保存自己那一组。
 *
 * ⚠️ 不要在调用点写 `api.saveSettings({ ...useApp.getState().settings, ...patch })`：
 * 那个写法虽然也过了车道（不会被并发顶掉），但它提交的是**整张表**——
 * ① 别处卡片里已经存进 store、但没走过 `normalizeSettings` 的值会原样落盘（限幅被绕掉）；
 * ② 少 `mergedGroup` 这道闸，将来谁往这条路上多带一个字段都不会有人拦。
 *
 * `patch` 传函数时在**排到队之后**才求值（与 `useSettingsForm.save` 同口径），
 * 取值时机不能是「被点击」：连着点两个开关时，第二次点下的界面状态还没被第一次的结果
 * 刷新，直接读渲染作用域会把前一个改动吃掉。
 */
export function saveSettingsGroup<T>(
  groupKeys: readonly (keyof Settings)[],
  patch: Partial<Settings> | ((current: Settings) => Partial<Settings>),
): Promise<T> {
  return enqueueSettingsSave(() => {
    const current = useApp.getState().settings;
    const resolved = typeof patch === "function" ? patch(current) : patch;
    return api.saveSettings(mergedGroup(current, resolved, groupKeys)) as Promise<T>;
  });
}
