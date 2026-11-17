import type { ModelChoice } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
/** Model choices recorded in claimed job specs, including Ask and repairs. */
export function ModelsUsed({ models }: { models?: ModelChoice[] }) {
  if (!models?.length) return null;
  return (
    <p className="text-xs text-muted [overflow-wrap:anywhere]">
      <Tooltip tip="Model and effort recorded in the job specifications, including Ask and repair jobs.">
        <span tabIndex={0}>
          Analysed with {models.map((m) => `${m.model}, effort ${m.reasoning_effort}`).join("; ")}
        </span>
      </Tooltip>
    </p>
  );
}
