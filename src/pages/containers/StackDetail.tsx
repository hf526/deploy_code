import { useTranslation } from "react-i18next";

import { AlertTriangle, HardDrive, Image as ImageIcon, Layers } from "lucide-react";

import { Badge } from "../../components/ui";
import type { ComposeStackDetail } from "../../lib/types";
import { humanSize } from "../../lib/utils";

/** 快照前的项目预览：会带走哪些服务、数据卷与镜像，以及预检提示。 */
export function StackDetail({ detail }: { detail: ComposeStackDetail }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-ink-faint">
        <span>
          {t("containers.composeCommand")}:{" "}
          <span className="font-mono text-ink-dim">{detail.composeCommand}</span>
        </span>
        <span>
          {t("containers.configFiles")}: <span className="text-ink-dim">{detail.stack.files.length}</span>
        </span>
        <span>
          {t("containers.envFiles")}:{" "}
          <span className="text-ink-dim">{detail.envFiles.length || t("containers.none")}</span>
        </span>
      </div>

      {detail.warnings.length > 0 && (
        <div className="flex flex-col gap-1.5 rounded-md border border-warn/35 bg-warn-soft px-3 py-2.5">
          {detail.warnings.map((warning) => (
            <p key={warning} className="flex items-start gap-2 text-[11.5px] leading-relaxed text-ink">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-warn" />
              <span className="min-w-0 flex-1 break-words">{warning}</span>
            </p>
          ))}
        </div>
      )}

      <div>
        <p className="mb-1.5 flex items-center gap-1.5 text-[11px] font-medium tracking-wide text-ink-faint">
          <Layers className="size-3.5" />
          {t("containers.services")}
        </p>
        <div className="divide-y divide-line overflow-hidden rounded-md border border-line">
          {detail.services.map((service) => (
            <div key={service.container} className="flex items-center gap-3 px-3 py-2">
              <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-ink">
                {service.name}
              </span>
              <span className="min-w-0 max-w-[45%] truncate text-[11px] text-ink-faint">
                {service.image}
              </span>
              <span className="min-w-0 max-w-[28%] truncate text-[11px] text-ink-faint">
                {service.ports}
              </span>
              <Badge kind={service.state === "running" ? "green" : "gray"}>{service.state}</Badge>
            </div>
          ))}
        </div>
      </div>

      <div>
        <p className="mb-1.5 flex items-center gap-1.5 text-[11px] font-medium tracking-wide text-ink-faint">
          <HardDrive className="size-3.5" />
          {t("containers.volumes")}
          <span className="text-ink-faint">· {humanSize(detail.volumeBytes)}</span>
        </p>
        {detail.volumes.length === 0 ? (
          <p className="text-[11px] text-ink-faint">{t("containers.noVolumes")}</p>
        ) : (
          <div className="divide-y divide-line overflow-hidden rounded-md border border-line">
            {detail.volumes.map((volume) => (
              <div key={volume.name} className="flex items-center gap-3 px-3 py-2">
                <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-ink">
                  {volume.name}
                </span>
                <span className="shrink-0 text-[11px] text-ink-dim">{humanSize(volume.sizeBytes)}</span>
                {!volume.readable && (
                  <Badge kind="amber">{t("containers.volumeViaContainer")}</Badge>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      <div>
        <p className="mb-1.5 flex items-center gap-1.5 text-[11px] font-medium tracking-wide text-ink-faint">
          <ImageIcon className="size-3.5" />
          {t("containers.images")}
          <span className="text-ink-faint">· {humanSize(detail.imageBytes)}</span>
        </p>
        <div className="flex flex-wrap gap-1.5">
          {detail.images.map((image) => (
            <Badge key={image} kind="gray" className="max-w-full truncate font-mono text-[11px]">
              {image}
            </Badge>
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-1">
        {detail.stack.files.map((file) => (
          <p key={file} className="truncate font-mono text-[11px] text-ink-faint" title={file}>
            {file}
          </p>
        ))}
      </div>
    </div>
  );
}
