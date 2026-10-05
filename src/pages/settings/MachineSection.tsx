import { useEffect, useState } from "react";
import { Power } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Checkbox, Field, Input, SectionTitle } from "../../components/ui";
import { api } from "../../lib/api";
import { FALLBACK_CANCEL_WINDOW_SECS, formatClockTime } from "../../lib/shutdown";
import { useApp } from "../../lib/store";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { Settings } from "../../lib/types";

const SHUTDOWN_KEYS: readonly (keyof Settings)[] = [
  "scheduledShutdownEnabled",
  "scheduledShutdownTime",
];

/** 本机开机自启与定时关机：都是这台 Windows 的事，与业务模块无关。 */
export function MachineSection() {
  const { t } = useTranslation();
  const { settings, save } = useSettingsForm(SHUTDOWN_KEYS);
  const toast = useApp((state) => state.toast);
  const shutdownStatus = useApp((state) => state.shutdownStatus);
  const setShutdownStatus = useApp((state) => state.setShutdownStatus);
  const cancelShutdown = useApp((state) => state.cancelShutdown);

  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [shutdownTime, setShutdownTime] = useState(settings.scheduledShutdownTime);
  const [shutdownDelay, setShutdownDelay] = useState(30);
  const [shutdownBusy, setShutdownBusy] = useState(false);

  // 开机自启动当前仅 Windows 支持：其它平台不展示该开关。
  const isWindows =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

  useEffect(() => {
    void api
      .getAutostart()
      .then(setAutostart)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    setShutdownTime(settings.scheduledShutdownTime);
  }, [settings.scheduledShutdownTime]);

  async function handleScheduleShutdown() {
    setShutdownBusy(true);
    try {
      setShutdownStatus(await api.scheduleShutdown(shutdownDelay));
      toast("success", t("settings.shutdown.started", { minutes: shutdownDelay }));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setShutdownBusy(false);
    }
  }

  async function handleAutostart(enabled: boolean) {
    setAutostartBusy(true);
    try {
      await api.setAutostart(enabled);
      setAutostart(enabled);
      toast(
        "success",
        enabled ? t("settings.automation.autostartOn") : t("settings.automation.autostartOff"),
      );
    } catch (error) {
      toast("error", String(error));
    } finally {
      setAutostartBusy(false);
    }
  }

  if (!isWindows) return null;

  return (
    <section>
      <SectionTitle
        title={t("settings.automation.title")}
        description={t("settings.automation.description")}
      />
      <Card className="flex flex-col gap-5 p-5">
        <div>
          <Checkbox
            checked={autostart}
            disabled={autostartBusy}
            onChange={(checked) => void handleAutostart(checked)}
          >
            {t("settings.automation.autostart")}
          </Checkbox>
          <p className="mt-2 pl-6 text-[11px] leading-relaxed text-ink-faint">
            {t("settings.automation.autostartHint")}
          </p>
        </div>

        <div className="border-t border-line pt-5">
          <Checkbox
            checked={settings.scheduledShutdownEnabled}
            onChange={(checked) =>
              void save({ scheduledShutdownEnabled: checked }, { notify: false })
            }
          >
            {t("settings.shutdown.dailyEnable")}
          </Checkbox>
          <div className="mt-3 grid grid-cols-1 gap-4 md:grid-cols-2">
            <Field label={t("settings.shutdown.time")}>
              <Input
                type="time"
                value={shutdownTime}
                onChange={(event) => {
                  setShutdownTime(event.target.value);
                  // 时间选完整即保存，避免改完直接关窗导致修改丢失。
                  if (/^\d{1,2}:\d{2}$/.test(event.target.value)) {
                    void save({ scheduledShutdownTime: event.target.value }, { notify: false });
                  }
                }}
                onBlur={() => {
                  // 清空或非法值时回填已保存的时间，避免界面与摘要不一致。
                  if (!/^\d{1,2}:\d{2}$/.test(shutdownTime)) {
                    setShutdownTime(settings.scheduledShutdownTime);
                  }
                }}
              />
            </Field>
            {!autostart && settings.scheduledShutdownEnabled && (
              <Field label={t("settings.shutdown.autostartLabel")}>
                <Button
                  variant="secondary"
                  size="sm"
                  className="justify-start"
                  disabled={autostartBusy}
                  onClick={() => void handleAutostart(true)}
                >
                  {t("settings.shutdown.enableAutostart")}
                </Button>
              </Field>
            )}
          </div>
          <p className="mt-2 text-[11px] leading-relaxed text-ink-faint">
            {settings.scheduledShutdownEnabled
              ? t("settings.shutdown.summary", {
                  time: settings.scheduledShutdownTime,
                  secs: shutdownStatus?.cancelWindowSecs ?? FALLBACK_CANCEL_WINDOW_SECS,
                })
              : t("settings.shutdown.dailyDisabled")}
          </p>
        </div>

        <div className="border-t border-line pt-5">
          <div className="flex flex-wrap items-end gap-3">
            <Field
              label={t("settings.shutdown.countdownLabel")}
              hint={t("settings.shutdown.countdownHint")}
            >
              <div className="w-28">
                <Input
                  type="number"
                  min={1}
                  max={1440}
                  value={shutdownDelay}
                  onChange={(event) => setShutdownDelay(Number(event.target.value))}
                />
              </div>
            </Field>
            <Button
              variant="secondary"
              icon={<Power className="size-4" />}
              loading={shutdownBusy}
              disabled={!(shutdownDelay >= 1 && shutdownDelay <= 1440)}
              onClick={() => void handleScheduleShutdown()}
            >
              {t("settings.shutdown.start")}
            </Button>
          </div>
          {shutdownStatus?.pending && (
            <div className="mt-3 flex flex-wrap items-center gap-3 rounded-md border border-warn/35 bg-warn-soft px-3 py-2">
              <span className="min-w-0 flex-1 text-[12px] text-ink">
                {t("settings.shutdown.armed", {
                  time: formatClockTime(shutdownStatus.pending.atMs),
                  source:
                    shutdownStatus.pending.source === "scheduled"
                      ? t("settings.shutdown.sourceScheduled")
                      : t("settings.shutdown.sourceManual"),
                })}
              </span>
              <Button variant="danger" size="sm" onClick={() => void cancelShutdown()}>
                {t("settings.shutdown.cancel")}
              </Button>
            </div>
          )}
          <p className="mt-2 text-[11px] leading-relaxed text-ink-faint">
            {t("settings.shutdown.countdownDescription")}
          </p>
        </div>
      </Card>
    </section>
  );
}
