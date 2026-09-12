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

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function deployStatusLabel(status: DeployStatus): string {
  switch (status) {
    case "success":
      return "成功";
    case "failed":
      return "失败";
    default:
      return "进行中";
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

export function authLabel(auth: SshAuth): string {
  return auth.type === "password" ? "密码" : "私钥";
}

export function authSummary(server: ServerConfig): string {
  if (server.auth.type === "password") return "密码认证";
  const file = server.auth.keyPath.replace(/\\/g, "/").split("/").pop();
  return `密钥: ${file ?? server.auth.keyPath}`;
}

export function shortPath(path: string, max = 48): string {
  if (path.length <= max) return path;
  return `…${path.slice(path.length - max + 1)}`;
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
