import { useTranslation } from "react-i18next";

import type { ServerConfig } from "../lib/types";
import { Select } from "./ui";

/**
 * 服务器下拉选择：统一「名称 (用户名@主机)」展示。
 *
 * - 传入 `placeholder` 时始终保留一个空选项（部署配置可暂不选择服务器）；
 * - 未传 `placeholder` 且服务器为空时显示 `emptyText` 占位，服务器非空时不提供空选项。
 */
export function ServerSelect({
  value,
  onChange,
  servers,
  disabled,
  placeholder,
  emptyText,
}: {
  value: string;
  onChange: (serverId: string) => void;
  servers: ServerConfig[];
  disabled?: boolean;
  placeholder?: string;
  emptyText?: string;
}) {
  const { t } = useTranslation();
  return (
    <Select
      value={value}
      onChange={(event) => onChange(event.target.value)}
      disabled={disabled || servers.length === 0}
    >
      {placeholder !== undefined ? (
        <option value="">{placeholder}</option>
      ) : servers.length === 0 ? (
        <option value="">{emptyText ?? t("backup.noServers")}</option>
      ) : null}
      {servers.map((server) => (
        <option key={server.id} value={server.id}>
          {server.name} ({server.username}@{server.host})
        </option>
      ))}
    </Select>
  );
}
