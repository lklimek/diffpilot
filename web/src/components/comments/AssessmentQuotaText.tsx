import type { AssessmentQuota } from "../../api/generated/types";
import { dateTimeText } from "../format/time";

export function AssessmentQuotaText({ quota }: { quota: AssessmentQuota }) {
  return `${quota.remaining} of 80 daily passes remaining · resets at ${dateTimeText(quota.resets_at)}`;
}
