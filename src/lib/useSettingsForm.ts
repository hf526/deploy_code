import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { api } from "./api";
import { enqueueSettingsSave, mergedGroup } from "./settingsDraft";
import { useApp } from "./store";
import type { Settings } from "./types";

interface SaveOptions {
  /** 成功后是否弹「已保存」。开关类控件连续点按时不弹，避免一屏提示。 */
  notify?: boolean;
}

/**
 * 一块设置板块（一个卡片 / 一个 section）的编辑状态。
 *
 * groupKeys 划定这块板块能提交哪些字段：`save_settings` 是整表回写，
 * 不界定的话，A 板块的保存按钮会把 B 板块正在编辑、尚未提交的草稿一起写下去。
 */
export function useSettingsForm(groupKeys: readonly (keyof Settings)[]) {
  const { t } = useTranslation();
  const settings = useApp((state) => state.settings);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);
  const [draft, setDraft] = useState<Settings>(settings);
  const previousSettings = useRef(settings);

  // 只把本组真正变化的字段同步进草稿，别处落盘不该冲掉这里未保存的编辑。
  useEffect(() => {
    const before = previousSettings.current;
    previousSettings.current = settings;
    setDraft((current) => {
      const next: Settings = { ...current };
      const target = next as Record<keyof Settings, Settings[keyof Settings]>;
      for (const key of groupKeys) {
        if (settings[key] !== before[key]) target[key] = settings[key];
      }
      return next;
    });
  }, [settings, groupKeys]);

  /** 提交本组字段（patch 覆盖草稿，用于即时保存的控件）。 */
  async function save(patch?: Partial<Settings>, options: SaveOptions = {}) {
    const { notify = true } = options;
    try {
      // 排到队才取快照：整表回写时带着的必须是前一次提交刚落盘的那份，
      // 否则别的板块刚保存的字段会被这次原样顶回去。
      const saved = await enqueueSettingsSave(async () =>
        api.saveSettings(mergedGroup(useApp.getState().settings, patch ?? draft, groupKeys)),
      );
      setSettings(saved);
      if (notify) toast("success", t("settings.saved"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  return { settings, draft, setDraft, save };
}
