import { useTranslation } from "react-i18next";

import type { ServerConfig } from "../lib/types";
import { Checkbox } from "./ui";

/** 服务器展示名：与 ServerSelect 保持一致的「名称 (用户名@主机)」格式。 */
export function serverLabel(server: ServerConfig) {
  return `${server.name} (${server.username}@${server.host})`;
}

/**
 * 服务器勾选列表。部署配置选目标机器、以及部署时勾选本次范围都用它，
 * 两处共用一份渲染与顺序规则（按传入 servers 的顺序）。
 */
export function ServerCheckList({
  servers,
  checkedIds,
  onChange,
  disabled,
  emptyText,
}: {
  servers: ServerConfig[];
  checkedIds: string[];
  onChange: (ids: string[]) => void;
  disabled?: boolean;
  emptyText?: string;
}) {
  const { t } = useTranslation();
  if (servers.length === 0) {
    return (
      <p className="rounded-md border border-line bg-field px-3 py-2.5 text-[11px] text-ink-faint">
        {emptyText ?? t("deploy.noTargetServers")}
      </p>
    );
  }
  return (
    <div className="flex max-h-52 flex-col gap-1.5 overflow-y-auto rounded-md border border-line bg-field p-2.5">
      {servers.map((server) => {
        const checked = checkedIds.includes(server.id);
        return (
          <Checkbox
            key={server.id}
            checked={checked}
            disabled={disabled}
            onChange={(next) =>
              onChange(
                next
                  ? [...checkedIds, server.id]
                  : checkedIds.filter((id) => id !== server.id),
              )
            }
          >
            <span className="truncate">{serverLabel(server)}</span>
          </Checkbox>
        );
      })}
    </div>
  );
}
