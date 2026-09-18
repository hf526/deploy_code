import i18n from "./i18n";
import type { DeployStatus, ServerConfig, SshAuth } from "./types";

/** 合并 className。 */
export function cn(...values: Array<string | false | null | undefined>): string {
  return values.filter(Boolean).join(" ");
}

export function formatDuration(ms: number): string {
  if (!ms) return "-";
  if (ms < 1000) return `${ms}ms`;
  // 先按秒四舍五入，避免出现 "1m60s" 这类显示。
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(totalSeconds / 60)}m${totalSeconds % 60}s`;
}

export function deployStatusLabel(status: DeployStatus): string {
  switch (status) {
    case "success":
      return i18n.t("status.success");
    case "failed":
      return i18n.t("status.failed");
    default:
      return i18n.t("status.running");
  }
}

export function deployStatusClass(status: DeployStatus): string {
  switch (status) {
    case "success":
      return "border-pos/30 bg-pos-soft text-pos";
    case "failed":
      return "border-neg/30 bg-neg-soft text-neg";
    default:
      return "border-warn/30 bg-warn-soft text-warn";
  }
}

/** 状态徽章颜色（Badge kind），列表与记录页共用。 */
export function statusBadgeKind(status: DeployStatus): "green" | "red" | "amber" {
  if (status === "success") return "green";
  if (status === "failed") return "red";
  return "amber";
}

export function authLabel(auth: SshAuth): string {
  return auth.type === "password" ? i18n.t("auth.password") : i18n.t("auth.privateKey");
}

export function authSummary(server: ServerConfig): string {
  if (server.auth.type === "password") return i18n.t("auth.passwordAuth");
  const file = server.auth.keyPath.replace(/\\/g, "/").split("/").pop();
  return i18n.t("auth.keyFile", { name: file ?? server.auth.keyPath });
}

export function shortPath(path: string, max = 48): string {
  if (path.length <= max) return path;
  return `…${path.slice(path.length - max + 1)}`;
}

/** 字节数转展示文案（配置/日志大小等）。 */
export function humanSize(bytes: number): string {
  if (!bytes) return "-";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

/**
 * 由本地环境文件路径推导部署目录内的相对路径。
 * 命中仓库根目录时保留子目录（`<repo>/backend/.env` → `backend/.env`），
 * 否则退回文件名：用户也可能选仓库外的密钥文件，此时默认放到部署目录根。
 */
export function inferEnvRemotePath(localPath: string, repoPath?: string | null): string {
  const normalize = (value: string) => value.replace(/\\/g, "/").replace(/\/+$/, "");
  const local = normalize(localPath);
  const segments = local.split("/").filter(Boolean);
  const fallback = segments[segments.length - 1] ?? "";
  const repo = repoPath ? normalize(repoPath) : "";
  if (!local || !repo) return fallback;
  // Windows 盘符 / 目录大小写不敏感：比较时统一小写，返回时保留原始大小写。
  const prefix = `${repo.toLowerCase()}/`;
  if (!local.toLowerCase().startsWith(prefix)) return fallback;
  const relative = local.slice(prefix.length).replace(/^\/+/, "");
  return relative || fallback;
}

/** 遮蔽查询串中的 password 参数（与后端 mask_database_url 行为一致）。 */
function maskQueryPassword(remainder: string): string {
  const question = remainder.indexOf("?");
  if (question < 0) return remainder;
  const hashIndex = remainder.indexOf("#");
  const hash = hashIndex < 0 ? remainder.length : hashIndex;
  if (question > hash) return remainder;
  const prefix = remainder.slice(0, question);
  const query = remainder.slice(question + 1, hash);
  const fragment = remainder.slice(hash);
  const masked = query
    .split("&")
    .map((param) => {
      const key = param.split("=")[0];
      return key.toLowerCase() === "password" && param.includes("=") ? `${key}=***` : param;
    })
    .join("&");
  return `${prefix}?${masked}${fragment}`;
}

function firstIndexOf(value: string, chars: string[]): number {
  let found = -1;
  for (const char of chars) {
    const index = value.indexOf(char);
    if (index >= 0 && (found < 0 || index < found)) found = index;
  }
  return found;
}

/** 遮蔽连接串中的密码（userinfo 与查询参数）。密码含未编码 @ / 也能正确遮蔽。 */
export function maskUrlPassword(url: string): string {
  const schemeEnd = url.indexOf("://");
  if (schemeEnd < 0) return url;
  const prefix = url.slice(0, schemeEnd + 3);
  const rest = url.slice(schemeEnd + 3);
  // 只在查询 / 片段之前找最后一个 '@'，查询参数里的 '@' 不能当作 host 分界。
  const queryLimit = firstIndexOf(rest, ["?", "#"]);
  const limit = queryLimit < 0 ? rest.length : queryLimit;
  const at = rest.slice(0, limit).lastIndexOf("@");
  let authorityEnd: number;
  let authority: string;
  if (at >= 0) {
    const hostEndOffset = firstIndexOf(rest.slice(at + 1), ["/", "?", "#"]);
    const hostEnd = hostEndOffset < 0 ? rest.length : at + 1 + hostEndOffset;
    const creds = rest.slice(0, at);
    const user = creds.split(":")[0];
    const host = rest.slice(at + 1, hostEnd);
    authority = creds.includes(":") ? `${user}:***@${host}` : `${user}@${host}`;
    authorityEnd = hostEnd;
  } else {
    authorityEnd = limit;
    authority = rest.slice(0, authorityEnd);
  }
  return `${prefix}${authority}${maskQueryPassword(rest.slice(authorityEnd))}`;
}

/** 从远端地址推导 GitHub 仓库与预计的 Pages 访问地址（仅用于界面预览）。 */
export function githubTarget(remote: string | null): { owner: string; repo: string; url: string } | null {
  if (!remote) return null;
  const value = remote.trim().replace(/\/+$/, "");
  let rest: string | null = null;
  // scp 形式：git@github.com:owner/repo.git
  const scp = /^(?:[^@/]+@)?github\.com:(.+)$/i.exec(value);
  if (scp) {
    rest = scp[1];
  } else {
    // URL 形式：https://github.com/owner/repo.git、ssh://git@github.com/owner/repo
    const url = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]+@)?(?:www\.)?github\.com(?::\d+)?\/(.+)$/i.exec(
      value,
    );
    if (url) rest = url[1];
  }
  if (!rest) return null;
  // 去掉查询串 / 片段后再解析 owner/repo，避免 "repo.git#main" 这类结果。
  const cleanPath = rest.split(/[?#]/)[0];
  const [owner, repoRaw] = cleanPath.split("/");
  if (!owner || !repoRaw) return null;
  const repo = repoRaw.replace(/\.git$/i, "");
  if (!repo) return null;
  const url =
    repo.toLowerCase() === `${owner.toLowerCase()}.github.io`
      ? `https://${owner}.github.io/`
      : `https://${owner}.github.io/${repo}/`;
  return { owner, repo, url };
}

export function newServerTemplate(): ServerConfig {
  return {
    id: "",
    name: "",
    host: "",
    port: 22,
    username: "root",
    auth: { type: "password", password: "" },
    defaultTargetDir: "",
    createdAt: "",
  };
}
