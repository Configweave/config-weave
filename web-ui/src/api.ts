// One forge client for the whole app (same-origin; Vite proxies /api in
// dev). Custom weave endpoints ride the same envelope via api.request.

import { createClient } from "@forge/client";

export const api = createClient();

// --- typed wrappers over the weave-server API ------------------------------

export interface RunbookEntry {
  name: string;
}

export interface TreeNode {
  name: string;
  dir: boolean;
  children?: TreeNode[];
}

export interface TestDecl {
  name: string;
  description: string;
  backend: string;
  image: string;
  group: string | null;
}

export interface PackageEntry {
  name: string;
  description: string;
  tests: TestDecl[];
  scenarios: { name: string; description: string }[];
}

export interface Inventory {
  playbook: string;
  version: string;
  description: string;
  plays: { name: string; description: string; steps: number }[];
  packages: PackageEntry[];
}

export interface ValidateResult {
  ok: boolean;
  diags: { message: string; rendered: string }[];
}

export interface RunSummary {
  id: string;
  runbook: string;
  filter: string | null;
  status: string;
  exit_code: number | null;
}

export interface InstanceInfo {
  group: number;
  kind: "docker" | "vmlab";
  torn_down: boolean;
  // docker
  container_id?: string;
  image?: string;
  cli?: string;
  // vmlab
  lab_dir?: string;
  lab?: string;
  machine?: string;
  template?: string;
}

export interface RunSnapshot extends RunSummary {
  backend: string | null;
  image: string | null;
  keep: boolean;
  instances: InstanceInfo[];
  events: any[];
  dropped_events: number;
  report: any | null;
}

export const listRunbooks = () => api.request<RunbookEntry[]>("GET", "/api/runbooks");
export const runbookTree = (rb: string) =>
  api.request<TreeNode[]>("GET", `/api/runbooks/${encodeURIComponent(rb)}/tree`);
export const readFile = (rb: string, path: string) =>
  api.request<{ path: string; content: string }>(
    "GET",
    `/api/runbooks/${encodeURIComponent(rb)}/file?path=${encodeURIComponent(path)}`,
  );
export const writeFile = (rb: string, path: string, content: string) =>
  api.request<{ path: string }>(
    "PUT",
    `/api/runbooks/${encodeURIComponent(rb)}/file?path=${encodeURIComponent(path)}`,
    { content },
  );
export const validateRunbook = (rb: string) =>
  api.request<ValidateResult>("POST", `/api/runbooks/${encodeURIComponent(rb)}/validate`);
export const runbookInventory = (rb: string) =>
  api.request<Inventory>("GET", `/api/runbooks/${encodeURIComponent(rb)}/inventory`);

export const startRun = (req: {
  runbook: string;
  filter?: string;
  backend?: string;
  image?: string;
  keep?: boolean;
}) => api.request<{ id: string }>("POST", "/api/runs", req);
export const listRuns = () => api.request<RunSummary[]>("GET", "/api/runs");
export const getRun = (id: string) =>
  api.request<RunSnapshot>("GET", `/api/runs/${encodeURIComponent(id)}`);
export const cancelRun = (id: string) =>
  api.request<{ id: string; status: string }>(
    "POST",
    `/api/runs/${encodeURIComponent(id)}/cancel`,
  );
