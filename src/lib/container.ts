import i18n from "./i18n";
import type { ContainerRecord, ContainerRecordKind } from "./types";

/** 一句话概括任务对象：备份为「项目 · 来源」，迁移额外带上目标。 */
export function containerSummary(record: ContainerRecord): string {
  const source = record.serverName || record.serverId;
  const target = record.targetServerName || record.targetServerId;
  return target ? `${record.project} · ${source} → ${target}` : `${record.project} · ${source}`;
}

export function containerKindLabel(kind: ContainerRecordKind): string {
  switch (kind) {
    case "migrate":
      return i18n.t("containers.kindMigrate");
    case "restore":
      return i18n.t("containers.kindRestore");
    default:
      return i18n.t("containers.kindBackup");
  }
}
