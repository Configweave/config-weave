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

export interface ParamDecl {
  name: string;
  description: string;
  type: string;
  required: boolean;
  default: any | null;
}

export interface ResourceDecl {
  name: string;
  description: string;
  concurrency: string;
  params: ParamDecl[];
}

export interface GathererDecl {
  name: string;
  description: string;
  params: ParamDecl[];
}

export interface PackageEntry {
  name: string;
  description: string;
  resources?: ResourceDecl[];
  gatherers?: GathererDecl[];
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
  kept_alive?: number;
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

// --- systems ---------------------------------------------------------------

export interface TransportConfig {
  kind: "ssh" | "winrm";
  host: string;
  port: number | null;
  user: string;
  password: string | null;
  private_key: string | null;
  use_tls: boolean;
}

export interface SystemDef {
  name: string;
  description: string | null;
  playbook: string;
  play: string;
  kind: "direct" | "remote";
  os: "linux" | "windows";
  arch: string;
  transport: TransportConfig;
}

export interface SysRunSummary {
  id: string;
  system: string;
  action: "check" | "apply";
  status: string;
  phase: string;
  exit_code: number | null;
}

export interface SysRunSnapshot extends SysRunSummary {
  kind: "direct" | "remote";
  keep: boolean;
  playbook: string;
  play: string;
  events: any[];
  dropped_events: number;
  report: any | null;
}

export const listSystems = () => api.request<SystemDef[]>("GET", "/api/systems");
export const createSystem = (def: SystemDef) =>
  api.request<SystemDef>("POST", "/api/systems", def);
export const updateSystem = (name: string, def: SystemDef) =>
  api.request<SystemDef>("PUT", `/api/systems/${encodeURIComponent(name)}`, def);
export const deleteSystem = (name: string) =>
  api.request<{ deleted: string }>("DELETE", `/api/systems/${encodeURIComponent(name)}`);

export const startSystemRun = (name: string, req: { action: "check" | "apply"; keep?: boolean }) =>
  api.request<{ id: string }>("POST", `/api/systems/${encodeURIComponent(name)}/runs`, req);
export const listSystemRuns = () => api.request<SysRunSummary[]>("GET", "/api/system-runs");
export const getSystemRun = (id: string) =>
  api.request<SysRunSnapshot>("GET", `/api/system-runs/${encodeURIComponent(id)}`);
export const cancelSystemRun = (id: string) =>
  api.request<{ id: string; status: string }>(
    "POST",
    `/api/system-runs/${encodeURIComponent(id)}/cancel`,
  );

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
export const teardownRun = (id: string) =>
  api.request<RunSnapshot>("POST", `/api/runs/${encodeURIComponent(id)}/teardown`);

// --- package repository ------------------------------------------------------

export const listPackages = () =>
  api.request<{ packages: PackageEntry[]; error?: string }>("GET", "/api/packages");
export const getPackage = (name: string) =>
  api.request<PackageEntry>("GET", `/api/packages/${encodeURIComponent(name)}`);
export const addPackageToRunbook = (name: string, runbook: string, overwrite = false) =>
  api.request<{ runbook: string; package: string; path: string }>(
    "POST",
    `/api/packages/${encodeURIComponent(name)}/add-to-runbook`,
    { runbook, overwrite },
  );
export const startPackageTest = (
  name: string,
  req: { test?: string; backend?: string; image?: string; keep?: boolean },
) => api.request<{ id: string }>("POST", `/api/packages/${encodeURIComponent(name)}/test`, req);
