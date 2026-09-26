import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Boxes, Pencil, Play, Plus, Trash2 } from "lucide-react";

import { Badge, Button, Card, EmptyState, SectionTitle } from "../../components/ui";
import { useApp } from "../../lib/store";
import type { ContainerConfig } from "../../lib/types";

/**
 * 已保存的容器备份配置列表。
 *
 * 编辑与删除交给页面处理（弹窗与确认框是页面级状态），执行留在这里：
 * 「哪一行正在跑」只有列表自己关心，不必外溢到页面。
 */
export function ContainerConfigList({
  onEdit,
  onDelete,
}: {
  /** 传 null 表示新建。 */
  onEdit: (config: ContainerConfig | null) => void;
  onDelete: (config: ContainerConfig) => void;
}) {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const configs = useApp((state) => state.containerConfigs);
  const scheduledIds = useApp((state) => state.settings.scheduledContainerConfigIds);
  const live = useApp((state) => state.liveContainer);
  const startFromConfig = useApp((state) => state.startContainerConfigBackup);

  const [runningConfigId, setRunningConfigId] = useState("");
  const running = live?.status === "running";

  // 任务结束后清空列表项的进行中标记，使「立即备份」按钮恢复可用。
  useEffect(() => {
    if (live?.status !== "running") setRunningConfigId("");
  }, [live?.status]);

  /** 一键执行：参数只从盘上的配置取，与定时调度器走同一条命令。 */
  async function handleRun(config: ContainerConfig) {
    if (running) return;
    setRunningConfigId(config.id);
    try {
      await startFromConfig(config.id);
    } catch {
      // store 已弹出错误提示。
    }
  }

  return (
    <section>
      <SectionTitle
        title={t("containers.config.title")}
        description={t("containers.config.description")}
        actions={
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => onEdit(null)}>
            {t("containers.config.new")}
          </Button>
        }
      />
      {configs.length === 0 ? (
        <EmptyState
          icon={<Boxes className="size-4.5" />}
          title={t("containers.config.empty")}
          description={t("containers.config.emptyDescription")}
          action={
            <Button icon={<Plus className="size-4" />} onClick={() => onEdit(null)}>
              {t("containers.config.new")}
            </Button>
          }
        />
      ) : (
        <Card className="divide-y divide-line overflow-hidden">
          {configs.map((config) => {
            const server = servers.find((item) => item.id === config.serverId);
            const target = config.target;
            const targetServer = target
              ? servers.find((item) => item.id === target.serverId)
              : null;
            return (
              <div key={config.id} className="flex items-center gap-3 px-4 py-3">
                <span className="grid size-8 shrink-0 place-items-center rounded-md border border-brand-line bg-brand-soft text-brand">
                  <Boxes className="size-4" />
                </span>
                <div className="min-w-0 flex-1">
                  <p className="flex items-center gap-2 truncate text-[13px] font-medium text-ink">
                    {config.name || config.project}
                    {scheduledIds.includes(config.id) && (
                      <Badge kind="brand">{t("containers.config.scheduled")}</Badge>
                    )}
                  </p>
                  <p className="mt-0.5 truncate text-[11px] text-ink-faint">
                    {server?.name ?? t("containers.config.unknownServer")} · {config.project} →{" "}
                    {target
                      ? `${targetServer?.name ?? target.serverId} · ${target.targetDir}`
                      : t("containers.config.localBundle")}
                  </p>
                </div>
                <Button
                  size="sm"
                  icon={<Play className="size-3.5" />}
                  loading={runningConfigId === config.id && running}
                  disabled={running}
                  onClick={() => void handleRun(config)}
                >
                  {target ? t("containers.migrate") : t("containers.backupOnly")}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  title={t("containers.config.edit")}
                  disabled={running}
                  onClick={() => onEdit(config)}
                >
                  <Pencil className="size-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  title={t("containers.config.delete")}
                  disabled={running}
                  onClick={() => onDelete(config)}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            );
          })}
        </Card>
      )}
    </section>
  );
}
