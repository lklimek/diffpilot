// TODO(v1.4 phase B): replace with generated contract type (plan C2)
import { api, routePath, type Conditional } from "../client";
import type { DashboardSummary, ItemPage, ItemQuery, RefreshStatus, WorkItem } from "../generated/types";
import type { WorkItemBranches } from "./dashboardBranches";
import type { DashboardFactTag } from "./dashboardFactFilter";
import type { Judgement } from "./dashboardJudgement";
import type { DashboardLive, WorkItemLive } from "./dashboardLive";

export type DashboardItem = WorkItem &
  Partial<WorkItemLive & WorkItemBranches> & {
    judgement?: Judgement;
    tags: DashboardFactTag[];
  };

export type MyWorkSummary = DashboardSummary &
  Partial<DashboardLive> & {
    status_counts?: DashboardStatusCounts;
  };

type DashboardItemPage = Omit<ItemPage, "items"> & { items: DashboardItem[] };

// The Phase B fields extend the current route responses. Old responses omit them.
export const fetchMyWork = (): Promise<MyWorkSummary> => api.getDashboard();
export const fetchMyWorkItems = (
  query: ItemQuery,
  etag?: string | null,
): Promise<Conditional<DashboardItemPage>> => api.listDashboardItems(query, etag);

export interface DashboardStatusCounts {
  outdated: number;
  not_judged: number;
  not_live: number;
  excluded: number;
}

export type DashboardRefreshRequest = { rejudge_outdated: boolean };

export const REJUDGE_ITEM_PATH = "/api/v1/dashboard/items/:key/rejudge";

export const refreshAndRejudgeOutdated = () =>
  api.postProvisional<RefreshStatus>(routePath("startDashboardRefresh"), {
    rejudge_outdated: true,
  } satisfies DashboardRefreshRequest);

export const rejudgeDashboardItem = (key: string) =>
  api.postProvisional<void>(REJUDGE_ITEM_PATH.replace(":key", encodeURIComponent(key)));
