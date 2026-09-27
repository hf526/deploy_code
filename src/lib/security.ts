/** 服务器安全的纯逻辑：免封白名单草稿校验、拉黑可行性判断。服务端 normalize 才是最终口径。 */

/** 白名单条数上限，与 deploy-core 的 WHITELIST_MAX 一致。 */
export const WHITELIST_MAX = 64;

export type WhitelistDraftReason = "invalid" | "duplicate" | "full";
export type WhitelistDraftResult =
  | { ok: true; entries: string[] }
  | { ok: false; reason: WhitelistDraftReason };

/** 一个 IP 里为什么不该被拉黑；null 表示没有保护理由。 */
export type BlockGuardReason = "self" | "whitelisted";

/**
 * 规范化条目：去掉 `[IPv6]` 方括号与 `%zone`、统一小写、每组去前导零，用于比对与去重。
 * 只做展示层的最小整理（不展开 `::`）；去重口径以服务端为准——服务端按地址解析后重写，
 * `2001:db8:0:0:0:0:0:1` 这种全写形式在那里也会并成一条。
 */
export function normalizeIpEntry(value: string): string {
  const trimmed = value.trim().toLowerCase();
  const bare = trimmed.startsWith("[") ? trimmed.slice(1, trimmed.lastIndexOf("]")) : trimmed;
  // 只有 IPv6 才带 zone id（fe80::1%eth0），IPv4 后面挂东西就是写错了。
  if (!bare.includes(":")) return bare;
  const zoneAt = bare.lastIndexOf("%");
  const withoutZone = zoneAt > 0 ? bare.slice(0, zoneAt) : bare;
  return withoutZone
    .split(":")
    .map((group) => (/^0+[0-9a-f]{1,4}$/.test(group) ? group.replace(/^0+/, "") : group))
    .join(":");
}

function isIpv4(value: string): boolean {
  const groups = value.split(".");
  if (groups.length !== 4) return false;
  return groups.every((group) => /^(0|[1-9]\d?|1\d\d|2[0-4]\d|25[0-5])$/.test(group));
}

function isHexGroup(value: string): boolean {
  return /^[0-9a-f]{1,4}$/.test(value);
}

function isIpv6(value: string): boolean {
  if (!value.includes(":")) return false;
  // :: 最多出现一次，否则地址不唯一。
  const compressed = value.split("::");
  if (compressed.length > 2) return false;
  let head: string[];
  let tail: string[] | null;
  if (compressed.length === 2) {
    head = compressed[0].split(":");
    tail = compressed[1].split(":");
  } else {
    head = value.split(":");
    tail = null;
  }
  // 末尾可能是内嵌 IPv4（::ffff:1.2.3.4），它占两组。
  const countGroups = (groups: string[]): number | null => {
    let count = 0;
    for (let index = 0; index < groups.length; index += 1) {
      const group = groups[index];
      if (group === "") {
        // 只有压缩后的首尾允许空段（"::1" 拆出 ["", "1"]，"fe80::" 拆出 ["fe80", ""]）。
        if (index !== 0 && index !== groups.length - 1) return null;
        continue;
      }
      if (group.includes(".")) {
        if (!isIpv4(group) || index !== groups.length - 1) return null;
        count += 2;
        continue;
      }
      if (!isHexGroup(group)) return null;
      count += 1;
    }
    return count;
  };
  if (tail === null) {
    return countGroups(head) === 8;
  }
  const headCount = countGroups(head);
  const tailCount = countGroups(tail);
  if (headCount === null || tailCount === null) return false;
  return headCount + tailCount <= 7;
}

/** 全零地址：防火墙规则里的"任意地址"，写进白名单等于对所有来源免封。 */
function isUnspecified(entry: string): boolean {
  return /^0*$/.test(entry.replace(/[:.]/g, ""));
}

/** 是否是一个可下发的单个 IP（不收 CIDR、不收主机名）。 */
export function isSingleIp(value: string): boolean {
  const entry = normalizeIpEntry(value);
  if (entry.length === 0 || entry.length > 45) return false;
  if (entry.includes("%")) return false;
  if (isUnspecified(entry)) return false;
  return entry.includes(":") ? isIpv6(entry) : isIpv4(entry);
}

/** 往白名单里加一条，返回新列表；不合法 / 重复 / 超限时给出原因。 */
export function addWhitelistEntry(entries: string[], value: string): WhitelistDraftResult {
  if (!isSingleIp(value)) return { ok: false, reason: "invalid" };
  const normalized = normalizeIpEntry(value);
  if (isWhitelisted(entries, normalized)) return { ok: false, reason: "duplicate" };
  if (entries.length >= WHITELIST_MAX) return { ok: false, reason: "full" };
  return { ok: true, entries: [...entries, normalized] };
}

export function isWhitelisted(entries: string[], ip: string): boolean {
  const target = normalizeIpEntry(ip);
  return entries.some((entry) => normalizeIpEntry(entry) === target);
}

/** 拉黑按钮的禁用理由：封自己当前的连接来源、或封白名单里的地址。 */
export function blockGuardReason(
  entries: string[],
  selfIp: string,
  ip: string,
): BlockGuardReason | null {
  if (selfIp && normalizeIpEntry(selfIp) === normalizeIpEntry(ip)) return "self";
  if (isWhitelisted(entries, ip)) return "whitelisted";
  return null;
}
