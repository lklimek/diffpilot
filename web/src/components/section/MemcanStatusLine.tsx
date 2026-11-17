import type { MemcanStatus } from "../../api/generated/types";
/** Last observed connection state; rendering never calls MemCan. */
export function MemcanStatusLine({ status }: { status?: MemcanStatus }) {
  const labels = {
    reachable: "reachable",
    not_reachable: "not reachable",
    not_configured: "not configured",
    not_checked: "not checked",
  };
  return (
    <>
      MemCan: {labels[status?.state ?? "not_checked"]}
      {status?.last_success_at && (
        <>
          ; last success <time dateTime={status.last_success_at}>{status.last_success_at}</time>
        </>
      )}
    </>
  );
}
