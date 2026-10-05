import { Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Checkbox, Field, Input, SectionTitle } from "../../components/ui";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { Settings } from "../../lib/types";

const DEPLOY_DEFAULTS_KEYS: readonly (keyof Settings)[] = [
  "scriptDir",
  "historyLimit",
  "connectTimeoutSecs",
  "scriptTimeoutSecs",
  "runScripts",
  "keepRemoteArchive",
  "atomicRelease",
  "releaseKeep",
  "pagesHistoryLimit",
];

/** 部署的全局默认值：新建部署配置时按这套兜底，改这里不影响已保存的配置。 */
export function DeployDefaultsCard() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(DEPLOY_DEFAULTS_KEYS);

  return (
    <section>
      <SectionTitle
        title={t("settings.deploy.title")}
        description={t("settings.deploy.description")}
        actions={
          <Button icon={<Settings2 className="size-4" />} onClick={() => void save()}>
            {t("settings.deploy.save")}
          </Button>
        }
      />
      <Card className="p-5">
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("settings.deploy.scriptDir")} hint={t("settings.deploy.scriptDirHint")}>
            <Input
              value={draft.scriptDir}
              onChange={(event) => setDraft({ ...draft, scriptDir: event.target.value })}
              placeholder="docker"
            />
          </Field>
          <Field label={t("settings.deploy.historyLimit")}>
            <Input
              type="number"
              value={draft.historyLimit}
              onChange={(event) =>
                setDraft({ ...draft, historyLimit: Number(event.target.value) })
              }
            />
          </Field>
          <Field label={t("settings.cloudflare.historyLimit")}>
            <Input
              type="number"
              value={draft.pagesHistoryLimit}
              onChange={(event) =>
                setDraft({ ...draft, pagesHistoryLimit: Number(event.target.value) })
              }
            />
          </Field>
          <Field label={t("settings.deploy.connectTimeout")}>
            <Input
              type="number"
              value={draft.connectTimeoutSecs}
              onChange={(event) =>
                setDraft({ ...draft, connectTimeoutSecs: Number(event.target.value) })
              }
            />
          </Field>
          <Field label={t("settings.deploy.scriptTimeout")}>
            <Input
              type="number"
              value={draft.scriptTimeoutSecs}
              onChange={(event) =>
                setDraft({ ...draft, scriptTimeoutSecs: Number(event.target.value) })
              }
            />
          </Field>
        </div>

        <div className="mt-5 flex flex-col gap-3 border-t border-line pt-5">
          <Checkbox
            checked={draft.runScripts}
            onChange={(checked) => setDraft({ ...draft, runScripts: checked })}
          >
            {t("settings.deploy.runScripts")}
          </Checkbox>
          <Checkbox
            checked={draft.keepRemoteArchive}
            onChange={(checked) => setDraft({ ...draft, keepRemoteArchive: checked })}
          >
            {t("settings.deploy.keepRemoteArchive")}
          </Checkbox>
          <Checkbox
            checked={draft.atomicRelease}
            onChange={(checked) => setDraft({ ...draft, atomicRelease: checked })}
          >
            {t("settings.deploy.atomicRelease")}
          </Checkbox>
          {draft.atomicRelease && (
            <div className="flex items-center gap-3 pl-6">
              <span className="text-[11px] text-ink-faint">{t("settings.deploy.releaseKeep")}</span>
              <div className="w-24">
                <Input
                  type="number"
                  value={draft.releaseKeep}
                  onChange={(event) =>
                    setDraft({ ...draft, releaseKeep: Number(event.target.value) })
                  }
                />
              </div>
            </div>
          )}
          <p className="pl-6 text-[11px] leading-relaxed text-ink-faint">
            {t("settings.deploy.atomicReleaseHint")}
          </p>
        </div>
      </Card>
    </section>
  );
}
