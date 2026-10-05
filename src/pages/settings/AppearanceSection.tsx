import { Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { ThemeToggle } from "../../components/ThemeToggle";
import { Button, Card, SectionTitle, Select } from "../../components/ui";
import { applyLanguage, normalizeLanguagePreference } from "../../lib/i18n";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { Settings } from "../../lib/types";

const APPEARANCE_KEYS: readonly (keyof Settings)[] = ["language"];

/** 主题与语言：语言切换当场生效，落盘由「保存设置」决定。 */
export function AppearanceSection() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(APPEARANCE_KEYS);

  return (
    <section>
      <SectionTitle
        title={t("settings.appearance.title")}
        description={t("settings.appearance.description")}
        actions={
          <Button icon={<Settings2 className="size-4" />} onClick={() => void save()}>
            {t("settings.appearance.save")}
          </Button>
        }
      />
      <Card className="flex flex-col p-4">
        <div className="flex items-center gap-4">
          <span className="min-w-0 flex-1 text-[13px] text-ink-dim">
            {t("settings.appearance.theme")}
          </span>
          <ThemeToggle className="w-72 shrink-0" />
        </div>
        <div className="mt-4 flex items-center gap-4 border-t border-line pt-4">
          <span className="min-w-0 flex-1 text-[13px] text-ink-dim">
            {t("settings.appearance.language")}
          </span>
          <div className="w-72 shrink-0">
            <Select
              value={normalizeLanguagePreference(draft.language)}
              onChange={(event) => {
                const next = { ...draft, language: event.target.value };
                setDraft(next);
                applyLanguage(next.language);
              }}
            >
              <option value="">{t("language.system")}</option>
              <option value="zh-CN">{t("language.zhCN")}</option>
              <option value="en-US">{t("language.enUS")}</option>
            </Select>
          </div>
        </div>
      </Card>
    </section>
  );
}
