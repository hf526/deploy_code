import type { TransferEvent, TransferInfo } from "./types";

/**
 * 文件传输的前端状态与展示用纯逻辑：事件归约、百分比、远端路径校验、上次用过的目录。
 *
 * 后端那份口径（`deploy-core::transfer`）不给百分比，只给 `done`/`total` 原值，所以这里算一份
 * 就是唯一一份。上限判定也不在这里：名额是后端按「同一条常驻会话的 SFTP 通道数」算的，
 * 前端再算一份就是两个真相，只会各说各话。
 */

/** 登记信息 + 当前进度：进度只活在内存里，刷新界面就靠 `listTransfers()` 重新对账。 */
export interface TransferEntry extends TransferInfo {
  done: number;
  total: number;
}

/** `list_transfers()` 回来的是登记信息，不带进度；先按「还没收到回调」画。 */
export function entriesFromInfos(infos: TransferInfo[]): TransferEntry[] {
  return infos.map((info) => ({ ...info, done: 0, total: 0 }));
}

/**
 * 挂载对账：只做加法，而且**已经收过尾的 id 不许被快照复活**。
 *
 * 快照是在后端某一瞬间拍的，而它回到前端要过一次 invoke：这中间那条传输正好结束的话，
 * `finished` 先到（那时列表里还没有它，归约只能忽略），快照后到——只按 id 去重的加法会把
 * 一条已经结束的传输重新画出来，而且再也不会有事件来摘它，取消还会报「已经结束或不存在」。
 * 没有新增时把原数组交回去，界面不为这一次对账白走一遍 React。
 */
export function mergeSeededEntries(
  current: TransferEntry[],
  infos: TransferInfo[],
  settled: ReadonlySet<string>,
): TransferEntry[] {
  let next = current;
  for (const seeded of entriesFromInfos(infos)) {
    if (settled.has(seeded.id)) continue;
    if (next.some((entry) => entry.id === seeded.id)) continue;
    next = [...next, seeded];
  }
  return next;
}

/**
 * 事件归约：四类任务共用的 `applyTaskEvent` 不接管这里——传输不占任务名额、不写记录，
 * 也就没有「界面重开之后拿持久化记录对账」那一步（盘上根本没有那份记录）。
 */
export function applyTransferEvent(
  entries: TransferEntry[],
  event: TransferEvent,
): TransferEntry[] {
  switch (event.type) {
    // started 会来两次（命令返回值 + 事件），按 id 去重：条带重复画一次就是噪音。
    case "started": {
      if (entries.some((entry) => entry.id === event.id)) return entries;
      return [
        ...entries,
        {
          id: event.id,
          serverId: event.serverId,
          serverName: event.serverName,
          direction: event.direction,
          fileName: event.fileName,
          remotePath: event.remotePath,
          done: 0,
          total: 0,
        },
      ];
    }
    case "progress": {
      const index = entries.findIndex((entry) => entry.id === event.id);
      if (index < 0) return entries;
      const current = entries[index];
      // 一秒好几条进度，而每次 setState 都要过一遍 React：没有变化就把原数组交回去。
      if (current.done === event.done && current.total === event.total) return entries;
      const next = entries.slice();
      next[index] = { ...current, done: event.done, total: event.total };
      return next;
    }
    // 收尾三条只剩「从列表里摘掉」：结果已经由 toast 定性，条带留着反而说不清现在在跑什么。
    case "finished":
    case "cancelled":
    case "failed":
      if (!entries.some((entry) => entry.id === event.id)) return entries;
      return entries.filter((entry) => entry.id !== event.id);
  }
}

/** 给展示层按 id 取一条（取消入口与进度条都只要这一句查询）。 */
export function findEntry(entries: TransferEntry[], id: string): TransferEntry | undefined {
  return entries.find((entry) => entry.id === id);
}

/**
 * 百分比。`total <= 0` 时返回 null 而不是 0%：头一条回调还不知道总量，
 * 画成 0% 是「刚开始」，画成「总量未知」才是实话，两者在界面上不该长得一样。
 */
export function transferPercent(entry: TransferEntry): number | null {
  if (entry.total <= 0) return null;
  const raw = (entry.done / entry.total) * 100;
  if (!Number.isFinite(raw)) return null;
  return Math.min(100, Math.max(0, Math.round(raw)));
}

/**
 * 远端路径校验，与 `transfer_target` 同一口径（那边是拒绝并说明下一步，这边只是让按钮先别亮）。
 *
 * 只收绝对路径：SFTP 那一路的当前目录由会话决定而不是由界面决定，用户在终端里 `cd` 过一次，
 * 同一句 `a.tar.gz` 两次点下去能落到两个地方。`~` 是字面量，不展开。
 */
export function isValidRemotePath(value: string): boolean {
  const trimmed = value.trim();
  if (!trimmed || trimmed === "~" || trimmed.startsWith("~/")) return false;
  if (!trimmed.startsWith("/")) return false;
  return !trimmed.split("/").some((part) => part === "..");
}

/**
 * 目录末尾恰好一个斜杠（后端按「以 / 结尾 = 目录」把文件名落进去）。
 *
 * 空串仍然返回空串：这里兜出一个 `/` 等于把用户没填的目录变成「传到根目录」，
 * 那是静默兜底，不是补全。
 */
export function joinRemoteDir(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return "";
  return `${trimmed.replace(/\/+$/, "")}/`;
}

/**
 * 每台服务器上次用完的远端目录：本机视图偏好，与字号 / 树宽同口径存 localStorage，
 * 不进 settings——它不该参与导出抹凭据 / 导入保留那两条不变量，也不该被配置导入盖掉。
 */
export const REMOTE_DIR_STORAGE_KEY = "deploycode:transfer.remoteDir";

/**
 * 盘上这份 JSON 可能是别的版本写的、可能根本不是对象：认不出就当没存过，
 * 拿它去喂路径输入框比空着更糟。非字符串的值一并丢掉（`{"s1":123}` 那种）。
 */
export function readStoredRemoteDirs(raw: string | null): Record<string, string> {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) return {};
  const dirs: Record<string, string> = {};
  for (const [serverId, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof value === "string" && value) dirs[serverId] = value;
  }
  return dirs;
}

/** 一台一份，其余原样留着：整表覆盖会把别的机器上次用的目录一起抹掉。 */
export function writeStoredRemoteDir(serverId: string, dir: string): void {
  try {
    const dirs = readStoredRemoteDirs(localStorage.getItem(REMOTE_DIR_STORAGE_KEY));
    localStorage.setItem(REMOTE_DIR_STORAGE_KEY, JSON.stringify({ ...dirs, [serverId]: dir }));
  } catch {
    // 隐私模式 / WebView 禁写：这次照样能传，只是下次进来要重新读一次远端目录。
  }
}

/** 没存过就是空串——调用方据此决定去不去问 `remoteHome()`，不能在这里编一个起点出来。 */
export function readStoredRemoteDirSafe(serverId: string): string {
  try {
    return readStoredRemoteDirs(localStorage.getItem(REMOTE_DIR_STORAGE_KEY))[serverId] ?? "";
  } catch {
    return "";
  }
}
