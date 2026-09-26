import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarClock } from "lucide-react";

import { Card, Checkbox, Field, Input, SectionTitle } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { Settings } from "../../lib/types";

/**
 * 容器定时备份：一个时间点 + 勾选若干配置，到点按勾选顺序排队，一次只跑一个。
 *
 * 与数据库备份各自独立：两类任务在后端有各自的名额与跨进程锁，同一晚并行不冲突。
 */
export function ContainerScheduleCard() {
  const { t } = useTranslation();
  const settings = useApp((state) => state.settings);
  const configs = useApp((state) => state.containerConfigs);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);

  const [time, setTime] = useState(settings.scheduledContainerTime);
  useEffect(() => setTime(settings.scheduledContainerTime), [settings.scheduledContainerTime]);
  // 设置保存串行化：连续修改时按顺序提交，避免在途请求用旧快照互相覆盖。
  const saveQueue = useRef<Promise<void>>(Promise.resolve());

  /**
   * patch 用函数给出，取值时机是「轮到自己被提交」而不是「被点击」：
   * 连着勾两个配置时，第二次点下的界面状态还没被第一次的结果刷新，
   * 直接读渲染作用域会把前一个勾选吃掉。
   */
  async function save(build: (current: Settings) => Partial<Settings>) {
    const run = saveQueue.current.then(async () => {
      const current = useApp.getState().settings;
      const saved = await api.saveSettings({ ...current, ...build(current) });
      setSettings(saved);
      toast("success", t("containers.schedule.saved"));
    });
    saveQueue.current = run.catch(() => undefined);
    try {
      await run;
    } catch (error) {
      toast("error", String(error));
    }
  }

  const selectedIds = settings.scheduledContainerConfigIds;

  function toggleConfig(configId: string, on: boolean) {
    void save((current) => ({
      scheduledContainerConfigIds: on
        ? [...current.scheduledContainerConfigIds.filter((id) => id !== configId), configId]
        : current.scheduledContainerConfigIds.filter((id) => id !== configId),
    }));
  }

  return (
    <section>
      <SectionTitle
        title={t("containers.schedule.title")}
        description={t("containers.schedule.description")}
      />
      <Card className="flex flex-col gap-4 p-5">
        <Checkbox
          checked={settings.scheduledContainerEnabled}
          onChange={(checked) => void save(() => ({ scheduledContainerEnabled: checked }))}
        >
          {t("containers.schedule.enable")}
        </Checkbox>
        <Field label={t("containers.schedule.time")}>
          <Input
            className="max-w-40"
            type="time"
            value={time}
            onChange={(event) => {
              setTime(event.target.value);
              // 时间选择完成即保存，避免改完直接关闭窗口导致修改丢失。
              if (/^\d{1,2}:\d{2}$/.test(event.target.value)) {
                void save(() => ({ scheduledContainerTime: event.target.value }));
              }
            }}
            onBlur={() => {
              // 清空或非法值时回填已保存的时间，避免界面与摘要不一致。
              if (!/^\d{1,2}:\d{2}$/.test(time)) setTime(settings.scheduledContainerTime);
            }}
          />
        </Field>

        {configs.length === 0 ? (
          <p className="rounded-md border border-line bg-field px-3 py-2.5 text-[11px] text-ink-faint">
            {t("containers.schedule.noConfigs")}
          </p>
        ) : (
          <div className="flex max-h-52 flex-col gap-1.5 overflow-y-auto rounded-md border border-line bg-field p-2.5">
            {configs.map((config) => (
              <Checkbox
                key={config.id}
                checked={selectedIds.includes(config.id)}
                onChange={(checked) => toggleConfig(config.id, checked)}
              >
                <span className="truncate">{config.name}</span>
                <span className="ml-1 truncate text-[11px] text-ink-faint">
                  {config.project}
                  {config.target ? ` → ${config.target.targetDir}` : ""}
                </span>
              </Checkbox>
            ))}
          </div>
        )}

        <p className="text-[11px] leading-relaxed text-ink-faint">
          {settings.containerBundleKeep > 0
            ? t("containers.schedule.retention", { keep: settings.containerBundleKeep })
            : t("containers.schedule.retentionOff")}
        </p>

        <p className="flex items-center gap-1.5 text-[11px] leading-relaxed text-ink-faint">
          <CalendarClock className="size-3.5 shrink-0" />
          {settings.scheduledContainerEnabled
            ? t("containers.schedule.summary", {
                time: settings.scheduledContainerTime,
                count: selectedIds.length,
              })
            : t("containers.schedule.disabled")}
        </p>
      </Card>
    </section>
  );
}
