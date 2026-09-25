import { useEffect, useState } from "react";
import { Plus, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Checkbox, Field, inputClass, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { CRON_METHODS, CRON_PRESETS, localTimeZone } from "../../lib/cronJob";
import { useApp } from "../../lib/store";
import type { CronHeader, CronJob } from "../../lib/types";
import { cn } from "../../lib/utils";

/** 常用时区，本机时区排在最前（cron-job.org 只认 IANA 名称）。 */
function timezoneChoices(): string[] {
  const local = localTimeZone();
  return [local, "Asia/Shanghai", "Asia/Tokyo", "UTC", "Europe/Berlin", "America/New_York"].filter(
    (zone, index, all) => zone && all.indexOf(zone) === index,
  );
}

function emptyDraft() {
  return {
    title: "",
    url: "",
    method: "GET",
    cron: "*/30 * * * *",
    timezone: localTimeZone(),
    timeoutSecs: 15,
    headers: [] as CronHeader[],
    body: "",
    enabled: true,
  };
}

/** 新建 / 编辑一个 cron-job.org 任务。 */
export function CronJobModal({
  open,
  job,
  onClose,
  onSaved,
}: {
  open: boolean;
  job: CronJob | null;
  onClose: () => void;
  onSaved: (title: string) => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [draft, setDraft] = useState(emptyDraft());
  const [saving, setSaving] = useState(false);

  // 只在新建/切换目标时回填：列表刷新会换掉 job 对象引用，不能把用户正在编辑的内容冲掉。
  useEffect(() => {
    if (!open) return;
    setSaving(false);
    if (!job) {
      setDraft(emptyDraft());
      return;
    }
    setDraft({
      title: job.title,
      url: job.url,
      method: CRON_METHODS[job.requestMethod] ?? "GET",
      cron: job.cron,
      timezone: job.schedule?.timezone || localTimeZone(),
      timeoutSecs: job.requestTimeout || 15,
      headers: Object.entries(job.extendedData?.headers ?? {}).map(([key, value]) => ({
        key,
        value,
      })),
      body: job.extendedData?.body ?? "",
      enabled: job.enabled,
    });
  }, [open, job?.jobId]);

  const presetValue = CRON_PRESETS.some((preset) => preset.cron === draft.cron.trim())
    ? draft.cron.trim()
    : "custom";

  function pickPreset(value: string) {
    if (value === "custom") return;
    setDraft({ ...draft, cron: value });
  }

  function setHeader(index: number, patch: Partial<CronHeader>) {
    setDraft({
      ...draft,
      headers: draft.headers.map((header, at) => (at === index ? { ...header, ...patch } : header)),
    });
  }

  async function handleSave() {
    if (saving) return;
    setSaving(true);
    try {
      await api.saveCronJob({
        jobId: job?.jobId ?? null,
        title: draft.title.trim(),
        url: draft.url.trim(),
        method: draft.method,
        headers: draft.headers,
        body: draft.body,
        timeoutSecs: Number(draft.timeoutSecs) || 0,
        enabled: draft.enabled,
        cron: draft.cron.trim(),
        timezone: draft.timezone,
      });
      onSaved(draft.title.trim());
      onClose();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={job ? t("cronJobs.editTitle") : t("cronJobs.createTitle")}
      subtitle={t("cronJobs.modalSubtitle")}
      width="max-w-2xl"
      footer={
        <>
          <Button variant="secondary" disabled={saving} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={saving} onClick={() => void handleSave()}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("cronJobs.fieldTitle")} required>
            <Input
              autoFocus
              value={draft.title}
              onChange={(event) => setDraft({ ...draft, title: event.target.value })}
              placeholder={t("cronJobs.fieldTitlePlaceholder")}
            />
          </Field>
          <Field label={t("cronJobs.fieldMethod")} hint={t("cronJobs.fieldMethodHint")}>
            <Select
              value={draft.method}
              onChange={(event) => setDraft({ ...draft, method: event.target.value })}
            >
              {CRON_METHODS.map((method) => (
                <option key={method} value={method}>
                  {method}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <Field label={t("cronJobs.fieldUrl")} required hint={t("cronJobs.fieldUrlHint")}>
          <Input
            value={draft.url}
            onChange={(event) => setDraft({ ...draft, url: event.target.value })}
            placeholder="https://example.com/api/ping"
            className="font-mono"
          />
        </Field>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("cronJobs.fieldPreset")}>
            <Select value={presetValue} onChange={(event) => pickPreset(event.target.value)}>
              {CRON_PRESETS.map((preset) => (
                <option key={preset.cron} value={preset.cron}>
                  {t(preset.labelKey)}
                </option>
              ))}
              {/* 只在表达式不属于任何预设时才给出这一项：它只是当前状态，不是可执行动作。 */}
              {presetValue === "custom" && (
                <option value="custom">{t("cronJobs.presetCustom")}</option>
              )}
            </Select>
          </Field>
          <Field
            label={t("cronJobs.fieldCron")}
            required
            hint={t("cronJobs.fieldCronHint")}
          >
            <Input
              value={draft.cron}
              onChange={(event) => setDraft({ ...draft, cron: event.target.value })}
              placeholder="*/30 * * * *"
              className="font-mono"
            />
          </Field>
        </div>

        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("cronJobs.fieldTimezone")} hint={t("cronJobs.fieldTimezoneHint")}>
            <Select
              value={draft.timezone}
              onChange={(event) => setDraft({ ...draft, timezone: event.target.value })}
            >
              {timezoneChoices().map((zone) => (
                <option key={zone} value={zone}>
                  {zone}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("cronJobs.fieldTimeout")} hint={t("cronJobs.fieldTimeoutHint")}>
            <Input
              type="number"
              min={1}
              max={600}
              value={draft.timeoutSecs}
              onChange={(event) => setDraft({ ...draft, timeoutSecs: Number(event.target.value) })}
            />
          </Field>
        </div>

        <Field label={t("cronJobs.fieldHeaders")}>
          <div className="flex flex-col gap-2">
            {draft.headers.map((header, index) => (
              <div key={index} className="flex items-center gap-2">
                <Input
                  value={header.key}
                  onChange={(event) => setHeader(index, { key: event.target.value })}
                  placeholder={t("cronJobs.headerKey")}
                  className="font-mono"
                />
                <Input
                  value={header.value}
                  onChange={(event) => setHeader(index, { value: event.target.value })}
                  placeholder={t("cronJobs.headerValue")}
                  className="font-mono"
                />
                <Button
                  size="sm"
                  variant="ghost"
                  title={t("cronJobs.removeHeader")}
                  onClick={() =>
                    setDraft({
                      ...draft,
                      headers: draft.headers.filter((_, at) => at !== index),
                    })
                  }
                  icon={<X className="size-3.5" />}
                />
              </div>
            ))}
            <Button
              size="sm"
              variant="secondary"
              className="self-start"
              icon={<Plus className="size-3.5" />}
              onClick={() =>
                setDraft({ ...draft, headers: [...draft.headers, { key: "", value: "" }] })
              }
            >
              {t("cronJobs.addHeader")}
            </Button>
          </div>
        </Field>

        <Field label={t("cronJobs.fieldBody")} hint={t("cronJobs.fieldBodyHint")}>
          <textarea
            rows={4}
            spellCheck={false}
            value={draft.body}
            onChange={(event) => setDraft({ ...draft, body: event.target.value })}
            placeholder='{"key":"value"}'
            className={cn(inputClass, "min-h-24 h-auto resize-y py-2 font-mono text-[12px]")}
          />
        </Field>

        <Checkbox
          checked={draft.enabled}
          onChange={(checked) => setDraft({ ...draft, enabled: checked })}
        >
          {t("cronJobs.fieldEnabled")}
        </Checkbox>
      </div>
    </Modal>
  );
}
