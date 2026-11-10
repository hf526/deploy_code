import { useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { Trans, useTranslation } from "react-i18next";
import { useLocation, useNavigate } from "react-router-dom";
import { Plus, Server } from "lucide-react";

import { Button, ConfirmModal, EmptyState } from "../components/ui";
import { api } from "../lib/api";
import {
  copyToClipboard,
  readStoredFontSizeSafe,
  readStoredTmuxSafe,
  writeStoredFontSize,
  writeStoredTmux,
} from "../lib/terminal";
import {
  clampTreeWidth,
  readStoredCompactSafe,
  readStoredTreeWidthSafe,
  writeStoredCompact,
  writeStoredTreeWidth,
} from "../lib/serverTree";
import { useApp } from "../lib/store";
import { useAutoRefresh } from "../lib/useAutoRefresh";
import type { ServerConfig, ServerHostStatus, TransferDirection } from "../lib/types";
import { ServerFormModal } from "./servers/ServerFormModal";
import { SecurityModal } from "./servers/SecurityModal";
import { ServerTree } from "./servers/ServerTree";
import { TerminalWorkspace } from "./servers/TerminalWorkspace";
import { TransferModal } from "./servers/TransferModal";
import { TunnelModal } from "./servers/TunnelModal";
import { useTerminalTabs } from "./servers/useTerminalTabs";
import { useTransfers } from "./servers/useTransfers";

/**
 * 服务器工作台：左边一台一个会话节点，右边是它的终端标签。
 *
 * 原来「服务器」与「终端」是两页，连一台机器要在列表里找到那一行点按钮、再到另一页的下拉框
 * 里把同一台重选一次；现在双击节点即开标签，隧道 / 安全 / 配置从节点的菜单进。
 * 业务口径一点没动：状态、占用与隧道仍全部来自 `list_host_status` 那一次内存读取，界面不新增
 * 任何 SSH 往返，终端也不占任务名额。
 */
export default function ServersPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const refreshServers = useApp((state) => state.refreshServers);
  const refreshBackupConfigs = useApp((state) => state.refreshBackupConfigs);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);
  const location = useLocation();
  const navigate = useNavigate();

  const [hostStatuses, setHostStatuses] = useState<ServerHostStatus[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ServerConfig | null>(null);
  const [removing, setRemoving] = useState<ServerConfig | null>(null);
  const [tunneling, setTunneling] = useState<ServerConfig | null>(null);
  const [securing, setSecuring] = useState<ServerConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [fontSize, setFontSize] = useState(readStoredFontSizeSafe);
  const [tmux, setTmux] = useState(readStoredTmuxSafe);
  const [treeWidth, setTreeWidth] = useState(readStoredTreeWidthSafe);
  // 列表密度与树宽同口径：本机视图偏好，不参与配置导出 / 导入。
  const [compact, setCompact] = useState(readStoredCompactSafe);
  const [reconnectingId, setReconnectingId] = useState<string | null>(null);
  // 弹窗只记「哪一台 + 哪个方向」：服务器对象本身交给下面的 find，免得列表刷新后
  // 弹窗里那份旧配置盖掉刚保存过的新值。
  const [transferModal, setTransferModal] = useState<{
    serverId: string;
    mode: TransferDirection;
  } | null>(null);

  const terminal = useTerminalTabs();
  const fileTransfer = useTransfers();
  const { openFor, closeTab, closeOthers, attach, detach } = terminal;

  // 会话状态、每条规则的监听情况与资源占用都由那一条常驻连接自己维护，这一句只读内存里的
  // 快照，不含任何 SSH 往返——所以可以按秒轮询，界面上的数字就是它此刻的样子。
  const { refresh: refreshHosts } = useAutoRefresh(
    async () => {
      setHostStatuses(await api.listHostStatus());
    },
    { intervalMs: 2000, minGapMs: 1500 },
  );

  useEffect(() => {
    if (servers.length === 0) return;
    // 选中的那台被删掉之后要落回一个真实存在的：否则 `plusTarget` 拿着那个死 id 去开终端，
    // 后端只会回一句「找不到这台服务器」。
    if (!servers.some((server) => server.id === selectedId)) setSelectedId(servers[0].id);
  }, [selectedId, servers]);

  // 别的页面「在某台机器上打开终端」跳过来时带的参数：进来就开一个标签，然后把参数清掉——
  // 不清的话刷新一次就多开一条同样的会话。
  const requested = useMemo(
    () => new URLSearchParams(location.search).get("server") ?? "",
    [location.search],
  );
  useEffect(() => {
    if (!requested) return;
    setSelectedId(requested);
    void openFor(requested, tmux);
    navigate({ pathname: "/servers" }, { replace: true });
  }, [requested, openFor, navigate, tmux]);

  function openForm(server: ServerConfig | null) {
    setEditing(server);
    setFormOpen(true);
  }

  /** 字号只有一个落点：按钮与 Ctrl+滚轮都走这里，写盘也只在这一处。 */
  function applyFontSize(value: number) {
    writeStoredFontSize(value);
    setFontSize(value);
  }

  /** 列表密度：与字号 / 树宽同口径，写盘只在这一个落点。 */
  function applyCompact(value: boolean) {
    writeStoredCompact(value);
    setCompact(value);
  }

  /** tmux 开关只影响之后开的标签（含重连，它是先关再开）；写盘与字号同口径只有一个落点。 */
  function applyTmux(value: boolean) {
    writeStoredTmux(value);
    setTmux(value);
  }

  /**
   * 点树节点：这台已经开着标签就直接切过去；没有就只选中，开新的仍交给双击 / 悬停按钮。
   * 「点一下就开」会把标签开满（后端每台只让开 3 个），这里只做「切过去」的顺手动作。
   */
  function selectServer(serverId: string) {
    setSelectedId(serverId);
    const tab = [...terminal.tabs].reverse().find((item) => item.serverId === serverId);
    if (tab) terminal.setActiveId(tab.id);
  }

  async function handleCopyAddress(server: ServerConfig) {
    const address = `${server.username}@${server.host}:${server.port}`;
    const ok = await copyToClipboard(address);
    toast(
      ok ? "success" : "error",
      ok ? t("servers.sshAddressCopied", { address }) : t("servers.sshAddressCopyFailed"),
    );
  }

  /** 行内 / 状态栏的重连入口：与隧道弹窗同一条命令，等一下让 2 秒轮询把新状态读回来。 */
  async function handleReconnect(server: ServerConfig) {
    if (reconnectingId) return;
    setReconnectingId(server.id);
    try {
      await api.reconnectHost(server.id);
      toast("info", t("servers.reconnectRequested", { name: server.name }));
      await new Promise((resolve) => setTimeout(resolve, 800));
      void refreshHosts(true);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setReconnectingId(null);
    }
  }

  /** 树宽拖拽：mousemove 里只改状态，松手那一次才写盘（与字号同口径的本机偏好）。 */
  function startTreeDrag(event: ReactMouseEvent) {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = treeWidth;
    const onMove = (move: MouseEvent) => {
      setTreeWidth(clampTreeWidth(startWidth + move.clientX - startX));
    };
    const onUp = (up: MouseEvent) => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      writeStoredTreeWidth(startWidth + up.clientX - startX);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  async function handleDelete() {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteServer(removing.id);
      toast("success", t("servers.deleted", { name: removing.name }));
      setRemoving(null);
      // 后端会一并删除该服务器的备份配置并清理定时备份引用，同步刷新避免残留悬空配置 / 选择。
      await Promise.all([refreshServers(), refreshBackupConfigs()]);
      // 删掉的那台会释放它的本机端口，徽章与状态要跟着收。
      void refreshHosts(true);
      void api
        .getSettings()
        .then(setSettings)
        .catch(() => undefined);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function handleTest(server: ServerConfig) {
    if (testingId) return;
    setTestingId(server.id);
    try {
      const message = await api.testServer(server);
      toast("success", message);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTestingId(null);
    }
  }

  return (
    <div className="flex h-full flex-col">
      {/* 紧凑页头：Page 那套「标题 + 副标题 + 20px 内边距」会把终端要的纵向空间吃掉一截，
          这一页是整高工作台，排版照 RepoDetailPage 的根布局走。 */}
      <header className="ui-titlebar flex shrink-0 items-center gap-3 border-b border-line px-4 py-2.5">
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-sm font-semibold tracking-tight text-ink">
            {t("nav.servers")}
          </h1>
          <p className="truncate text-[11px] text-ink-faint">{t("servers.subtitle")}</p>
        </div>
        <Button size="sm" icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
          {t("servers.add")}
        </Button>
      </header>

      {servers.length === 0 ? (
        <div className="min-h-0 flex-1 px-6 py-5">
          <EmptyState
            icon={<Server className="size-4.5" />}
            title={t("servers.emptyTitle")}
            description={t("servers.emptyDescription")}
            action={
              <Button icon={<Plus className="size-4" />} onClick={() => openForm(null)}>
                {t("servers.add")}
              </Button>
            }
          />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <ServerTree
            servers={servers}
            statuses={hostStatuses}
            selectedId={selectedId}
            tabCounts={terminal.tabCounts}
            testingId={testingId}
            compact={compact}
            width={treeWidth}
            reconnectingId={reconnectingId}
            onSelect={selectServer}
            onOpenTerminal={(server) => void openFor(server.id, tmux)}
            onTest={(server) => void handleTest(server)}
            onTunnels={setTunneling}
            onSecurity={setSecuring}
            onEdit={openForm}
            onDelete={setRemoving}
            onTransfer={(server, mode) => setTransferModal({ serverId: server.id, mode })}
            onCopyAddress={(server) => void handleCopyAddress(server)}
            onReconnect={(server) => void handleReconnect(server)}
            onToggleCompact={() => applyCompact(!compact)}
            onAdd={() => openForm(null)}
          />

          <div
            className="w-1 shrink-0 cursor-col-resize transition-colors hover:bg-brand-line"
            onMouseDown={startTreeDrag}
            title={t("servers.resizeTree")}
            aria-label={t("servers.resizeTree")}
          />

          <TerminalWorkspace
            tabs={terminal.tabs}
            activeId={terminal.activeId}
            busy={terminal.busy}
            activityIds={terminal.activityIds}
            clearActivity={terminal.clearActivity}
            servers={servers}
            statuses={hostStatuses}
            fontSize={fontSize}
            tmux={tmux}
            selectedServerId={selectedId}
            onFontSizeChange={applyFontSize}
            onTmuxChange={applyTmux}
            onSelect={terminal.setActiveId}
            onOpen={(serverId) => void openFor(serverId, tmux)}
            onClose={(id) => {
              // 标签没了，挂在它身上的那句 ZMODEM 指路也就没人要看了；留着会让下一个标签
              // 一开就冒出一句没头没尾的提示。closeTab 的 promise 原样交出去。
              fileTransfer.dismissZmodem(id);
              return closeTab(id);
            }}
            onCloseOthers={(id) => {
              for (const tab of terminal.tabs) {
                if (tab.id !== id) fileTransfer.dismissZmodem(tab.id);
              }
              void closeOthers(id);
            }}
            attach={attach}
            detach={detach}
            transfers={fileTransfer.entries}
            zmodemHint={fileTransfer.zmodemIds.includes(terminal.activeId)}
            onPickTransfer={(serverId, mode) => setTransferModal({ serverId, mode })}
            onCancelTransfer={(transferId) => void fileTransfer.cancel(transferId)}
            onDismissZmodem={() => fileTransfer.dismissZmodem(terminal.activeId)}
            onReconnect={(serverId) => {
              const server = servers.find((item) => item.id === serverId);
              if (server) void handleReconnect(server);
            }}
            reconnectingId={reconnectingId}
          />
        </div>
      )}

      <ServerFormModal
        open={formOpen}
        initial={editing}
        onClose={() => setFormOpen(false)}
        onSaved={() => {
          setFormOpen(false);
          void refreshServers();
          // 服务器表单也能带隧道规则，保存后本机端口分配可能就变了；
          // 新加的那台也会在这一轮起会话，状态读回来就能看到占用。
          void refreshHosts(true);
        }}
      />

      {tunneling && (
        <TunnelModal
          open
          server={tunneling}
          onClose={() => setTunneling(null)}
          onSaved={(saved) => {
            setTunneling(saved);
            void refreshServers();
            void refreshHosts(true);
          }}
        />
      )}

      {/* 按 id 挂 key：换一台就是换实例，旧那台的扫描结果不会再落到新服务器上。 */}
      {securing && (
        <SecurityModal key={securing.id} server={securing} onClose={() => setSecuring(null)} />
      )}

      {/* 按 id + 方向挂 key：换一台或换方向就是换实例，那一次在飞的 remote_home 回包
          不会落到新打开的这个弹窗上（方向不同，连该校验什么都变了）。 */}
      {transferModal && (
        <TransferModal
          key={`${transferModal.serverId}:${transferModal.mode}`}
          open
          server={servers.find((item) => item.id === transferModal.serverId) ?? null}
          mode={transferModal.mode}
          onClose={() => setTransferModal(null)}
          onUpload={fileTransfer.upload}
          onDownload={fileTransfer.download}
        />
      )}

      <ConfirmModal
        open={!!removing}
        danger
        loading={busy}
        title={t("servers.deleteTitle")}
        confirmText={t("common.delete")}
        description={
          <Trans
            i18nKey="servers.deleteDescription"
            values={{ name: removing?.name }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setRemoving(null)}
        onConfirm={() => void handleDelete()}
      />
    </div>
  );
}
