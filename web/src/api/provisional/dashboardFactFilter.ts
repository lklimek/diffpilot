// TODO(v1.4 phase B): replace with generated contract type (plan C4)
import type { FactTag } from "../generated/types";

export type DashboardFactTag = FactTag & { filter?: { key: string; label: string } };
