import { Save, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "./ui";

/**
 * 配置弹窗页脚：取消 / 测试环境（可选）/ 保存。
 * 各弹窗的按钮可用性差异通过 `testDisabled` / `saveDisabled` / `saveIcon` 传入。
 */
export function ConfigModalFooter({
  onClose,
  onSave,
  onTest,
  saving,
  testing,
  testDisabled = false,
  saveDisabled = false,
  saveIcon = true,
}: {
  onClose: () => void;
  onSave: () => void;
  onTest?: () => void;
  saving: boolean;
  testing: boolean;
  testDisabled?: boolean;
  saveDisabled?: boolean;
  saveIcon?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <>
      <Button variant="secondary" disabled={saving || testing} onClick={onClose}>
        {t("common.cancel")}
      </Button>
      {onTest && (
        <Button
          variant="secondary"
          loading={testing}
          disabled={saving || testDisabled}
          icon={<ShieldCheck className="size-4" />}
          onClick={onTest}
        >
          {t("common.testEnvironment")}
        </Button>
      )}
      <Button
        loading={saving}
        disabled={testing || saveDisabled}
        icon={saveIcon ? <Save className="size-4" /> : undefined}
        onClick={onSave}
      >
        {t("common.save")}
      </Button>
    </>
  );
}
