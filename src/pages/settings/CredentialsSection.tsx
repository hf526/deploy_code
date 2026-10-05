import { Save } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Field, Input, SectionTitle } from "../../components/ui";
import { useSettingsForm } from "../../lib/useSettingsForm";
import type { Settings } from "../../lib/types";

const CLOUDFLARE_KEYS: readonly (keyof Settings)[] = [
  "cloudflareApiToken",
  "cloudflareAccountId",
];
const GITHUB_KEYS: readonly (keyof Settings)[] = ["githubToken"];
const CRONJOB_KEYS: readonly (keyof Settings)[] = ["cronjobApiKey"];

/** 云端凭据：三家各存各的，保存其中一家不许把另一家没提交的草稿写下去。 */
export function CredentialsSection() {
  return (
    <>
      <CloudflareCard />
      <GithubCard />
      <CronJobCard />
    </>
  );
}

function CloudflareCard() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(CLOUDFLARE_KEYS);
  return (
    <section>
      <SectionTitle
        title={t("settings.cloudflare.title")}
        description={t("settings.cloudflare.description")}
      />
      <Card className="p-5">
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field
            label={t("settings.cloudflare.apiToken")}
            hint={t("settings.cloudflare.apiTokenHint")}
          >
            <Input
              type="password"
              value={draft.cloudflareApiToken}
              onChange={(event) =>
                setDraft({ ...draft, cloudflareApiToken: event.target.value })
              }
              placeholder="Cloudflare API Token"
            />
          </Field>
          <Field label={t("settings.cloudflare.accountId")}>
            <Input
              value={draft.cloudflareAccountId}
              onChange={(event) =>
                setDraft({ ...draft, cloudflareAccountId: event.target.value })
              }
              placeholder="Cloudflare Account ID"
            />
          </Field>
        </div>
        <div className="mt-4 flex justify-end border-t border-line pt-4">
          <Button
            variant="secondary"
            size="sm"
            icon={<Save className="size-3.5" />}
            onClick={() => void save()}
          >
            {t("settings.cloudflare.save")}
          </Button>
        </div>
      </Card>
    </section>
  );
}

function GithubCard() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(GITHUB_KEYS);
  return (
    <section>
      <SectionTitle
        title={t("settings.github.title")}
        description={t("settings.github.description")}
      />
      <Card className="p-5">
        <Field label={t("settings.github.token")} hint={t("settings.github.tokenHint")}>
          <Input
            type="password"
            value={draft.githubToken}
            onChange={(event) => setDraft({ ...draft, githubToken: event.target.value })}
            placeholder="ghp_... / github_pat_..."
          />
        </Field>
        <div className="mt-4 flex justify-end border-t border-line pt-4">
          <Button
            variant="secondary"
            size="sm"
            icon={<Save className="size-3.5" />}
            onClick={() => void save()}
          >
            {t("settings.github.save")}
          </Button>
        </div>
      </Card>
    </section>
  );
}

function CronJobCard() {
  const { t } = useTranslation();
  const { draft, setDraft, save } = useSettingsForm(CRONJOB_KEYS);
  return (
    <section>
      <SectionTitle
        title={t("settings.cronjob.title")}
        description={t("settings.cronjob.description")}
      />
      <Card className="p-5">
        <Field label={t("settings.cronjob.apiKey")} hint={t("settings.cronjob.apiKeyHint")}>
          <Input
            type="password"
            value={draft.cronjobApiKey}
            onChange={(event) => setDraft({ ...draft, cronjobApiKey: event.target.value })}
            placeholder="cron-job.org API Key"
          />
        </Field>
        <div className="mt-4 flex justify-end border-t border-line pt-4">
          <Button
            variant="secondary"
            size="sm"
            icon={<Save className="size-3.5" />}
            onClick={() => void save()}
          >
            {t("settings.cronjob.save")}
          </Button>
        </div>
      </Card>
    </section>
  );
}
