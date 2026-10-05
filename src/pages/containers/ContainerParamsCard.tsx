import { Save } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Field, Input, SectionTitle } from "../../components/ui";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { Settings } from "../../lib/types";

const CONTAINER_PARAMS_KEYS: readonly (keyof Settings)[] = [
  "containerHistoryLimit",
  "containerTimeoutSecs",
  "containerBundleKeep",
];

/** 容器备份/迁移的保留条数、超时与备份包轮转个数。 */
export function ContainerParamsCard() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(CONTAINER_PARAMS_KEYS);

  return (
    <section>
      <SectionTitle
        title={t("settings.containers.title")}
        description={t("settings.containers.description")}
      />
      <Card className="flex flex-col gap-3 p-5">
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field label={t("settings.containers.historyLimit")}>
            <Input
              type="number"
              value={draft.containerHistoryLimit}
              onChange={(event) =>
                setDraft({ ...draft, containerHistoryLimit: Number(event.target.value) })
              }
            />
          </Field>
          <Field label={t("settings.containers.timeout")}>
            <Input
              type="number"
              value={draft.containerTimeoutSecs}
              onChange={(event) =>
                setDraft({ ...draft, containerTimeoutSecs: Number(event.target.value) })
              }
            />
          </Field>
        </div>
        <Field
          label={t("settings.containers.bundleKeep")}
          hint={t("settings.containers.bundleKeepHint")}
        >
          <Input
            className="max-w-40"
            type="number"
            min={0}
            max={999}
            value={draft.containerBundleKeep}
            onChange={(event) =>
              setDraft({ ...draft, containerBundleKeep: Number(event.target.value) })
            }
          />
        </Field>
        <div className="flex justify-end border-t border-line pt-3">
          <Button
            variant="secondary"
            size="sm"
            icon={<Save className="size-3.5" />}
            onClick={() => void save()}
          >
            {t("settings.saveParams")}
          </Button>
        </div>
      </Card>
    </section>
  );
}
