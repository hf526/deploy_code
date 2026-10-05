import { useTranslation } from "react-i18next";

import { Page } from "../components/ui";
import { AppearanceSection } from "./settings/AppearanceSection";
import { ConfigBackupSection } from "./settings/ConfigBackupSection";
import { CredentialsSection } from "./settings/CredentialsSection";
import { MachineSection } from "./settings/MachineSection";
import { StorageSection } from "./settings/StorageSection";

/**
 * 只放跨模块的本机项：外观、开机自启与定时关机、云端凭据、数据目录与备份包维护、配置导入导出。
 * 各业务模块自己的参数跟着模块页走（部署默认在部署页、备份目标在备份页、容器参数在容器页）。
 */
export default function SettingsPage() {
  const { t } = useTranslation();
  return (
    <Page title={t("settings.title")} subtitle={t("settings.subtitle")}>
      <div className="flex max-w-4xl flex-col gap-8">
        <AppearanceSection />
        <MachineSection />
        <CredentialsSection />
        <StorageSection />
        <ConfigBackupSection />
      </div>
    </Page>
  );
}
