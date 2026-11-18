import { UsageLine } from "../usage/UsageLine";
import type { UsageWindows } from "../../api/generated/types";
import type { LlmCallOutcome } from "../../api/generated/types";
import { dateTimeText } from "../format/time";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

const CHIP = "cursor-help rounded border px-1.5 text-[11px] leading-[18px] font-semibold";

/** LLM status = outcome of the most recent LLM call and its time (brief §4.4); no on-demand check. */
export function LlmStatusCard({ status, usage }: { status: LlmCallOutcome | null; usage?: UsageWindows }) {
  return (
    <SettingsCard title="LLM status" help={<HelpTip topic="llm_status" />}>
      {status === null && (
        <p>Status not known yet: no LLM call has been made. It appears after the first analysis.</p>
      )}
      {status && (
        <p className="flex flex-wrap items-baseline gap-2">
          {status.status.state === "working" ? (
            <Tooltip tip="The most recent LLM call succeeded.">
              <span tabIndex={0} className={`${CHIP} border-add-bd bg-ok-bg text-ok`}>
                working
              </span>
            </Tooltip>
          ) : (
            <Tooltip tip="The most recent LLM call failed; the message below says why.">
              <span tabIndex={0} className={`${CHIP} border-high-bd bg-high-bg text-high`}>
                not working
              </span>
            </Tooltip>
          )}
          <span className="text-xs text-muted">last LLM call: {dateTimeText(status.at)}</span>
        </p>
      )}
      {status?.status.state === "not_working" && <p className="max-w-[100ch]">{status.status.message}</p>}
      <UsageLine usage={usage?.today} label="Today" />
      <UsageLine usage={usage?.last_7_days} label="Last 7 days" />
      {usage && <p className="text-xs text-muted">Usage windows follow the server’s local calendar.</p>}
      <p className="text-xs text-muted">
        This is the outcome of the most recent LLM call; there is no on-demand check. Credentials stay on the
        server and never reach the browser. The first Codex login is a one-time command in the runner
        container (see the runbook).
      </p>
    </SettingsCard>
  );
}
