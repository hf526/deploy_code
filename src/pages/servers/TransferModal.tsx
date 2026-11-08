import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, FolderOpen, Save, Upload } from "lucide-react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { Button, Field, Input, Modal } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import {
  isValidRemotePath,
  joinRemoteDir,
  readStoredRemoteDirSafe,
  writeStoredRemoteDir,
} from "../../lib/transfer";
import type { ServerConfig, TransferDirection } from "../../lib/types";

/** 远端路径的最后一段：下载的「另存为」默认名。 */
function basename(value: string): string {
  return value.split("/").filter(Boolean).pop() ?? "";
}

/** 记下「这台上次用到哪个目录」：给的是完整文件名时存它的上一级，下次还想落在同一个目录。 */
function dirOfRemotePath(value: string): string {
  if (value.endsWith("/")) return joinRemoteDir(value);
  const cut = value.lastIndexOf("/");
  // `/x.tar.gz` 的上一级就是根目录：这是算出来的，不是猜的。
  return cut <= 0 ? "/" : joinRemoteDir(value.slice(0, cut + 1));
}

/**
 * 上传 / 下载共用的弹窗（SFTP，不是 ZMODEM）。
 *
 * 远端目录的来源只有两条：这台上次真传成功过的，或 `remote_home` 现问一次的。问不到就留空
 * 并把话说清楚——不兜 `/`、不兜 `~`、也不兜服务器名：兜出来的那一个看着像「已经填好了」，
 * 而它把人送进一个谁也没确认过的目录。
 */
export function TransferModal({
  open,
  server,
  mode,
  onClose,
  onUpload,
  onDownload,
}: {
  open: boolean;
  server: ServerConfig | null;
  mode: TransferDirection;
  onClose: () => void;
  onUpload: (serverId: string, localPath: string, remotePath: string) => Promise<boolean>;
  onDownload: (serverId: string, remotePath: string, localPath: string) => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);

  const [localPath, setLocalPath] = useState("");
  const [remotePath, setRemotePath] = useState("");
  const [readingHome, setReadingHome] = useState(false);
  const [homeMissing, setHomeMissing] = useState(false);
  const [busy, setBusy] = useState(false);

  // 每一次「打开 / 换一台 / 换方向」占一个序号：慢回来的 remote_home 不能盖到新那次的输入上
  // ——关掉弹窗时还在飞的那一句，正好会落进下一次打开的状态里。
  const seq = useRef(0);
  const serverId = server?.id ?? "";

  useEffect(() => {
    const token = ++seq.current;
    if (!open || !serverId) return;
    setLocalPath("");
    setBusy(false);
    setHomeMissing(false);
    const stored = readStoredRemoteDirSafe(serverId);
    if (stored) {
      // 上次真传成功过的目录，不再问一次 remote_home：那一句还要占一路 SFTP。
      setReadingHome(false);
      setRemotePath(stored);
      return;
    }
    setRemotePath("");
    setReadingHome(true);
    void api
      .remoteHome(serverId)
      .then((dir) => {
        if (token !== seq.current) return;
        setReadingHome(false);
        setRemotePath(joinRemoteDir(dir));
      })
      .catch((error: unknown) => {
        if (token !== seq.current) return;
        setReadingHome(false);
        setHomeMissing(true);
        toast("error", String(error));
      });
    // 卸载（用户关掉弹窗）也要作废在飞的那一句：这个弹窗按 key 挂实例，关掉就是卸载，
    // 没有下一次打开来帮它递增序号。不作废的话 remoteHome 失败会在弹窗已经关掉之后弹一句
    // 没头没尾的错误，而 setState 那几下落进一个已经没了的实例。
    return () => {
      seq.current += 1;
    };
  }, [open, serverId, toast]);

  const uploading = mode === "upload";
  const remote = remotePath.trim();
  const local = localPath.trim();
  // 下载那一个是「一个文件」：目录（以 / 结尾）在这里没有意义，后端会拒绝，先别让人点。
  const remoteOk = uploading
    ? isValidRemotePath(remote)
    : isValidRemotePath(remote) && !remote.endsWith("/");
  const canStart = !!server && !readingHome && !!local && remoteOk;

  // 读不到起点时把话说在 hint 上（而不是替用户填一个）；他已经打出可用路径之后就收回来，
  // 留着会变成对一句已经填好的话重复提要求。
  const hint = readingHome
    ? t("transfer.remoteDirLoading")
    : homeMissing && !remoteOk
      ? t("transfer.remoteDirEmpty")
      : undefined;

  async function pickLocalFile() {
    const selected = await openDialog({
      multiple: false,
      directory: false,
      title: t("transfer.pickFile"),
    });
    // 取消对话框返回 null：那是「没改主意」而不是「清空」，把已经选好的路径抹掉最气人。
    if (typeof selected === "string") setLocalPath(selected);
  }

  async function pickSaveTarget() {
    const selected = await saveDialog({
      title: t("transfer.pickSaveTarget"),
      defaultPath: basename(remote),
    });
    if (typeof selected === "string") setLocalPath(selected);
  }

  async function submit() {
    if (!server || busy || readingHome) return;
    if (!local) {
      toast("error", t("transfer.errorNoLocalFile"));
      return;
    }
    if (!remoteOk) {
      toast("error", t("transfer.errorNoRemoteFile"));
      return;
    }
    setBusy(true);
    const started = uploading
      ? await onUpload(server.id, local, remote)
      : await onDownload(server.id, remote, local);
    setBusy(false);
    if (!started) return;
    // 只在真的开始传之后才记下这个目录：失败那次多半是填错了，让它变成下次的默认值等于
    // 把同一个错再预填一遍。
    writeStoredRemoteDir(server.id, dirOfRemotePath(remote));
    // 进度归下面那条带子说；弹窗不留着等它跑完——留着就挡住终端。
    onClose();
  }

  const localField = (
    <Field label={uploading ? t("transfer.localFile") : t("transfer.saveTo")} required>
      <div className="flex gap-2">
        <Input
          value={localPath}
          onChange={(event) => setLocalPath(event.target.value)}
          placeholder={uploading ? "C:\\Users\\you\\Downloads\\app.tar.gz" : "C:\\Temp\\app.tar.gz"}
        />
        <Button
          variant="secondary"
          icon={uploading ? <FolderOpen className="size-4" /> : <Save className="size-4" />}
          onClick={() => void (uploading ? pickLocalFile() : pickSaveTarget())}
        >
          {uploading ? t("transfer.pickFile") : t("transfer.pickSaveTarget")}
        </Button>
      </div>
    </Field>
  );

  const remoteField = (
    <Field
      label={uploading ? t("transfer.remoteDir") : t("transfer.remoteFile")}
      hint={hint}
      required
    >
      <Input
        value={remotePath}
        onChange={(event) => setRemotePath(event.target.value)}
        placeholder={uploading ? "/opt/app/" : "/root/app.tar.gz"}
        disabled={readingHome}
      />
    </Field>
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t(uploading ? "transfer.uploadTitle" : "transfer.downloadTitle", {
        name: server?.name ?? "",
      })}
      subtitle={t(uploading ? "transfer.uploadSubtitle" : "transfer.downloadSubtitle")}
      width="max-w-xl"
      onEnter={() => void submit()}
      footer={
        <>
          <Button variant="secondary" disabled={busy} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            loading={busy}
            disabled={!canStart}
            icon={uploading ? <Upload className="size-4" /> : <Download className="size-4" />}
            onClick={() => void submit()}
          >
            {t("transfer.startTransfer")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {/* 顺序跟着方向走：上传先看本机有什么，下载先说远端要哪一个。 */}
        {uploading ? localField : remoteField}
        {uploading ? remoteField : localField}
      </div>
    </Modal>
  );
}
