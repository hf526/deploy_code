import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, ShieldCheck, X } from "lucide-react";

import { Badge, Button, Field, Input } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import { WHITELIST_MAX, addWhitelistEntry, isWhitelisted, type WhitelistDraftReason } from "../../lib/security";
import type { SecurityReport, ServerConfig } from "../../lib/types";

const DRAFT_ERROR_KEYS: Record<WhitelistDraftReason, string> = {
  invalid: "servers.guardWhitelistInvalid",
  duplicate: "servers.guardWhitelistDuplicate",
  full: "servers.guardWhitelistFull",
};

const DEFAULT_THRESHOLD = 5;
const DEFAULT_WINDOW_MINS = 10;

type GuardDraft = {
  threshold: number;
  windowMins: number;
  whitelist: string[];
};

function draftOf(report: SecurityReport): GuardDraft {
  return {
    threshold: report.guardThreshold || DEFAULT_THRESHOLD,
    windowMins: report.guardWindowMins || DEFAULT_WINDOW_MINS,
    whitelist: report.whitelist,
  };
}

function sameDraft(left: GuardDraft, right: GuardDraft): boolean {
  return (
    left.threshold === right.threshold &&
    left.windowMins === right.windowMins &&
    left.whitelist.length === right.whitelist.length &&
    left.whitelist.every((entry, index) => entry === right.whitelist[index])
  );
}

/**
 * 服务器端自动防护：阈值 / 窗口 / 免封白名单。
 *
 * 三项都存在服务器那份 `/etc/deploycode-guard.conf` 里，界面上的初值一律来自扫描结果，
 * 本地不另存一份——否则改了远端又不知道，下次下发会把别人的改动盖回去。
 */
export function GuardSection({
  server,
  report,
  onChanged,
}: {
  server: ServerConfig;
  report: SecurityReport;
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);

  const initial = draftOf(report);
  const [threshold, setThreshold] = useState(initial.threshold);
  const [windowMins, setWindowMins] = useState(initial.windowMins);
  const [whitelist, setWhitelist] = useState<string[]>(initial.whitelist);
  const [draft, setDraft] = useState("");
  const [draftError, setDraftError] = useState<WhitelistDraftReason | null>(null);
  const [busy, setBusy] = useState(false);

  // 已经和服务器对齐的那份值。只有当前输入还等于它时才用新扫描结果覆盖，
  // 否则一次「解除 / 拉黑」后的重扫就把用户刚敲进去、还没下发的白名单整段抹掉了。
  const syncedRef = useRef(initial);
  useEffect(() => {
    const serverDraft = draftOf(report);
    if (!sameDraft({ threshold, windowMins, whitelist }, syncedRef.current)) return;
    syncedRef.current = serverDraft;
    setThreshold(serverDraft.threshold);
    setWindowMins(serverDraft.windowMins);
    setWhitelist(serverDraft.whitelist);
    setDraft("");
    setDraftError(null);
    // 有意只跟 report：输入变化时不该重新对齐。
  }, [report]);

  const selfIp = report.selfIp;
  const selfGuarded = selfIp !== "" && isWhitelisted(whitelist, selfIp);
  // 未启用的那台机器上白名单还不存在，空列表不算风险。
  const whitelistEmpty = report.guardEnabled && whitelist.length === 0;

  function addEntry() {
    const result = addWhitelistEntry(whitelist, draft);
    if (!result.ok) {
      setDraftError(result.reason);
      return;
    }
    setWhitelist(result.entries);
    setDraft("");
    setDraftError(null);
  }

  async function apply(action: "enable" | "disable") {
    setBusy(true);
    try {
      const message =
        action === "enable"
          ? await api.enableServerGuard(server, threshold, windowMins, whitelist)
          : await api.disableServerGuard(server);
      toast("success", message);
      if (action === "enable") {
        // 这份草稿已经写到服务器上了，随后的重扫就该采纳回读值（服务端会规范化 IP 写法）。
        syncedRef.current = { threshold, windowMins, whitelist };
      }
      onChanged();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section>
      <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.guardTitle")}</h4>
      <div className="rounded-md border border-line bg-field p-3.5">
        <div className="flex flex-wrap items-center gap-2">
          <Badge kind={report.guardEnabled ? "green" : "gray"}>
            {report.guardEnabled ? t("servers.guardEnabled") : t("servers.guardDisabled")}
          </Badge>
          <span className="text-[11.5px] text-ink-dim">{t("servers.guardDescription")}</span>
        </div>

        {selfIp !== "" && (
          <div className="mt-3 flex flex-wrap items-center gap-2 text-[11.5px]">
            <span className="text-ink-dim">
              {t("servers.guardConnectedFrom")}{" "}
              <b className="font-mono text-ink">{selfIp}</b>
            </span>
            {selfGuarded ? (
              <Badge kind="green">{t("servers.guardWhitelistBadge")}</Badge>
            ) : (
              <>
                {/* 防护没开时不存在"被自己封掉"的风险，别报警，只留入口。 */}
                {report.guardEnabled && (
                  <span className="text-warn">{t("servers.guardSelfNotWhitelisted")}</span>
                )}
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy}
                  icon={<Plus className="size-3.5" />}
                  onClick={() => {
                    const result = addWhitelistEntry(whitelist, selfIp);
                    // 加不进去要说得出原因（多半是满了），静默忽略等于白点一下。
                    if (result.ok) {
                      setWhitelist(result.entries);
                      setDraftError(null);
                    } else {
                      setDraftError(result.reason);
                    }
                  }}
                >
                  {t("servers.guardAddSelf")}
                </Button>
              </>
            )}
          </div>
        )}

        {selfIp === "" && (
          // 来源地址取自服务端回传的 SSH_CLIENT。读不到时那道"封不掉自己"的硬闸等于不存在，
          // 这里必须说出来——静默少一层保护比报错更难查。
          <p className="mt-3 text-[11px] text-warn">{t("servers.guardSelfUnknown")}</p>
        )}

        <div className="mt-3">
          <Field label={t("servers.guardWhitelist")}>
            <div className="flex flex-wrap items-center gap-2">
              {whitelist.length === 0 ? (
                <span className="text-[11px] text-ink-faint">{t("servers.guardWhitelistNone")}</span>
              ) : (
                whitelist.map((entry) => (
                  <span
                    key={entry}
                    className="inline-flex items-center gap-1 rounded-md border border-line bg-panel py-0.5 pr-1 pl-2 font-mono text-[11px] text-ink"
                  >
                    {entry}
                    <button
                      type="button"
                      disabled={busy}
                      title={t("servers.guardWhitelistRemove")}
                      onClick={() => setWhitelist(whitelist.filter((kept) => kept !== entry))}
                      className="rounded px-1 py-0.5 text-ink-faint hover:bg-hover hover:text-ink disabled:opacity-50"
                    >
                      <X className="size-3" />
                    </button>
                  </span>
                ))
              )}
            </div>
          </Field>
          <div className="mt-2 flex flex-wrap items-end gap-2">
            <Field label={t("servers.guardWhitelistAddLabel")} className="w-56">
              <Input
                value={draft}
                placeholder={t("servers.guardWhitelistPlaceholder")}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setDraftError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    addEntry();
                  }
                }}
              />
            </Field>
            <Button variant="secondary" disabled={busy || draft.trim() === ""} onClick={addEntry}>
              {t("servers.guardWhitelistAddButton")}
            </Button>
          </div>
          <p className="mt-2 text-[11px] text-ink-faint">
            {draftError ? (
              <span className="text-neg">
                {t(DRAFT_ERROR_KEYS[draftError], { count: WHITELIST_MAX })}
              </span>
            ) : (
              t("servers.guardWhitelistHint")
            )}
          </p>
          {whitelistEmpty && (
            <p className="mt-1 text-[11px] text-warn">{t("servers.guardWhitelistEmpty")}</p>
          )}
        </div>

        <div className="mt-3 flex flex-wrap items-end gap-2">
          <Field label={t("servers.guardThreshold")} className="w-28">
            <Input
              type="number"
              value={threshold}
              onChange={(event) => setThreshold(Number(event.target.value))}
            />
          </Field>
          <Field label={t("servers.guardWindow")} className="w-32">
            <Input
              type="number"
              value={windowMins}
              onChange={(event) => setWindowMins(Number(event.target.value))}
            />
          </Field>
          <Button
            variant="secondary"
            loading={busy}
            icon={<ShieldCheck className="size-3.5" />}
            onClick={() => void apply("enable")}
          >
            {report.guardEnabled ? t("servers.guardUpdate") : t("servers.guardEnable")}
          </Button>
          {report.guardEnabled && (
            <Button
              variant="ghost"
              className="text-neg hover:bg-neg-soft hover:text-neg"
              disabled={busy}
              onClick={() => void apply("disable")}
            >
              {t("servers.guardDisable")}
            </Button>
          )}
        </div>
        {!report.isRoot && !report.hasSudo && (
          <p className="mt-2 text-[11px] text-ink-faint">{t("servers.guardNeedRoot")}</p>
        )}
      </div>
    </section>
  );
}
