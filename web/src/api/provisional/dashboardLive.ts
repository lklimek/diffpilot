// TODO(v1.4 phase B): replace with generated contract type (plan C3)
export interface DashboardLive {
  live_repositories: number;
  not_live_repositories: number;
  last_event_at: string | null;
}

export interface WorkItemLive {
  live: boolean;
}
