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
  /// "local" or the remote repository's name (repo listings only).
  source?: string;
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

export const listRunbooks = () => api.request<RunbookEntry[]>("GET", "/api/playbooks");
export const validateRunbook = (rb: string) =>
  api.request<ValidateResult>("POST", `/api/playbooks/${encodeURIComponent(rb)}/validate`);
export const runbookInventory = (rb: string) =>
  api.request<Inventory>("GET", `/api/playbooks/${encodeURIComponent(rb)}/inventory`);

// --- graphical editors (DocJson) --------------------------------------------

export type Val = { lit: any } | { expr: string };
export interface Kv {
  key: string;
  value: Val;
}

export interface StepDoc {
  name: string;
  _orig?: string;
  description: string;
  resource: string;
  condition?: string;
  requires: string[];
  concurrency?: string;
  properties: Kv[];
}

export interface ContainerDoc {
  name: string;
  _orig?: string;
  description: string;
  condition?: string;
  items: PlayItemDoc[];
}

export type PlayItemDoc = { step: StepDoc } | { container: ContainerDoc };

export interface PlayDoc {
  name: string;
  _orig?: string;
  description: string;
  parallel?: boolean;
  items: PlayItemDoc[];
}

export interface GatherDoc {
  name: string;
  _orig?: string;
  description?: string;
  from: string;
  params: Kv[];
}

export interface PlaybookDoc {
  name: string;
  description: string;
  version?: string;
  gathers: GatherDoc[];
  vars: Kv[];
  plays: PlayDoc[];
}

export interface ParamDoc {
  name: string;
  _orig?: string;
  description: string;
  type: string;
  required?: boolean;
  default?: Val;
}

export interface ResourceDoc {
  name: string;
  _orig?: string;
  description: string;
  script: string;
  concurrency?: string;
  params: ParamDoc[];
}

export interface GathererDoc {
  name: string;
  _orig?: string;
  description: string;
  script: string;
  params: ParamDoc[];
}

export interface TestStepDoc {
  name: string;
  _orig?: string;
  description: string;
  resource: string;
  expect?: string;
  condition?: string;
  requires: string[];
  properties: Kv[];
}

export interface TestGatherDoc {
  name: string;
  _orig?: string;
  description: string;
  from: string;
  params: Kv[];
  expect: Kv[];
}

export interface TestDocEd {
  name: string;
  _orig?: string;
  description: string;
  backend?: string;
  image: string;
  group?: string;
  setup?: string;
  verify?: string;
  steps: TestStepDoc[];
  gathers: TestGatherDoc[];
}

export interface ScenarioDoc {
  name: string;
  _orig?: string;
  description: string;
  lab: string;
  script: string;
}

export interface PackageDoc {
  name: string;
  description: string;
  gatherers: GathererDoc[];
  resources: ResourceDoc[];
  tests: TestDocEd[];
  scenarios: ScenarioDoc[];
}

export interface DocParseResult {
  ok: boolean;
  kind?: "playbook" | "package";
  doc?: PlaybookDoc | PackageDoc;
  diags?: string[];
  base_hash?: string;
}

export interface DocRenderResult {
  ok: boolean;
  source?: string;
  diags?: string[];
}

export interface DocSaveResult {
  ok: boolean;
  path?: string;
  content?: string;
  base_hash?: string;
  diags?: string[];
}

export const getTemplates = () =>
  api.request<Record<string, string>>("GET", "/api/templates");

// --- editing workspaces ------------------------------------------------------
//
// Runbook roots and repo-package roots expose identical tree/file/doc
// endpoint shapes, so one URL-prefixed scope serves both; prefixedScope
// re-roots a runbook scope at pkgs/<name> for installed package copies.

export interface WorkspaceScope {
  tree(): Promise<TreeNode[]>;
  read(path: string): Promise<{ path: string; content: string }>;
  write(path: string, content: string): Promise<{ path: string }>;
  docParse(path: string, content?: string): Promise<DocParseResult>;
  docRender(path: string, doc: any, baseContent?: string): Promise<DocRenderResult>;
  docSave(path: string, doc: any, baseHash?: string): Promise<DocSaveResult>;
}

const scopeAt = (base: string): WorkspaceScope => ({
  tree: () => api.request<TreeNode[]>("GET", `${base}/tree`),
  read: (path) =>
    api.request("GET", `${base}/file?path=${encodeURIComponent(path)}`),
  write: (path, content) =>
    api.request("PUT", `${base}/file?path=${encodeURIComponent(path)}`, { content }),
  docParse: (path, content) => api.request("POST", `${base}/doc/parse`, { path, content }),
  docRender: (path, doc, base_content) =>
    api.request("POST", `${base}/doc/render`, { path, doc, base_content }),
  docSave: (path, doc, base_hash) => api.request("PUT", `${base}/doc`, { path, doc, base_hash }),
});

export const runbookScope = (rb: string) => scopeAt(`/api/playbooks/${encodeURIComponent(rb)}`);
export const packageScope = (name: string) =>
  scopeAt(`/api/packages/${encodeURIComponent(name)}`);

/// View a subdirectory of `inner` as the workspace root.
export const prefixedScope = (inner: WorkspaceScope, prefix: string): WorkspaceScope => ({
  tree: async () => {
    let nodes = await inner.tree();
    for (const seg of prefix.split("/")) {
      nodes = nodes.find((n) => n.dir && n.name === seg)?.children ?? [];
    }
    return nodes;
  },
  read: async (p) => ({ ...(await inner.read(`${prefix}/${p}`)), path: p }),
  write: (p, c) => inner.write(`${prefix}/${p}`, c),
  docParse: (p, c) => inner.docParse(`${prefix}/${p}`, c),
  docRender: (p, d, b) => inner.docRender(`${prefix}/${p}`, d, b),
  docSave: (p, d, h) => inner.docSave(`${prefix}/${p}`, d, h),
});

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
  kind: "direct" | "remote";
  os: "linux" | "windows";
  arch: string;
  transport: TransportConfig;
  assignments: AssignmentDef[];
}

export interface AssignmentDef {
  playbook: string;
  play: string;
}

export interface ServiceDef {
  name: string;
  description: string | null;
  systems: SystemDef[];
  schedules: ScheduleDef[];
}

export interface ScheduleDef {
  name: string;
  system: string;
  playbook: string;
  play: string;
  action: "check" | "apply";
  cron: string;
  enabled: boolean;
}

export interface SysRunSummary {
  id: string;
  started_at: string;
  system: string;
  service: string;
  playbook: string;
  play: string;
  trigger: "manual" | "scheduled";
  schedule: string | null;
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

export const listServices = () => api.request<ServiceDef[]>("GET", "/api/services");
export const createService = (def: ServiceDef) => api.request<ServiceDef>("POST", "/api/services", def);
export const updateService = (name: string, def: ServiceDef) => api.request<ServiceDef>("PUT", `/api/services/${encodeURIComponent(name)}`, def);
export const deleteService = (name: string) => api.request<{ deleted: string }>("DELETE", `/api/services/${encodeURIComponent(name)}`);
export const createSystem = (service: string, def: SystemDef) => api.request<SystemDef>("POST", `/api/services/${encodeURIComponent(service)}/systems`, def);
export const updateSystem = (service: string, name: string, def: SystemDef) => api.request<SystemDef>("PUT", `/api/services/${encodeURIComponent(service)}/systems/${encodeURIComponent(name)}`, def);
export const deleteSystem = (service: string, name: string) => api.request<{ deleted: string }>("DELETE", `/api/services/${encodeURIComponent(service)}/systems/${encodeURIComponent(name)}`);
export const createSchedule = (service: string, def: ScheduleDef) => api.request<ScheduleDef>("POST", `/api/services/${encodeURIComponent(service)}/schedules`, def);
export const updateSchedule = (service: string, name: string, def: ScheduleDef) => api.request<ScheduleDef>("PUT", `/api/services/${encodeURIComponent(service)}/schedules/${encodeURIComponent(name)}`, def);
export const deleteSchedule = (service: string, name: string) => api.request<{ deleted: string }>("DELETE", `/api/services/${encodeURIComponent(service)}/schedules/${encodeURIComponent(name)}`);
export const runScheduleNow = (service: string, name: string) => api.request<{ id: string }>("POST", `/api/services/${encodeURIComponent(service)}/schedules/${encodeURIComponent(name)}/run`);

export const startSystemRun = (service: string, name: string, req: { action: "check" | "apply"; playbook: string; play: string; keep?: boolean }) =>
  api.request<{ id: string }>("POST", `/api/services/${encodeURIComponent(service)}/systems/${encodeURIComponent(name)}/runs`, req);
export const listSystemRuns = () => api.request<SysRunSummary[]>("GET", "/api/system-runs");
export const getSystemRun = (id: string) =>
  api.request<SysRunSnapshot>("GET", `/api/system-runs/${encodeURIComponent(id)}`);
export const cancelSystemRun = (id: string) =>
  api.request<{ id: string; status: string }>(
    "POST",
    `/api/system-runs/${encodeURIComponent(id)}/cancel`,
  );

// Monitoring/Logs tabs: the server proxies PromQL/LogQL to the backends
// named by --prometheus-url/--loki-url; unconfigured backends 503.
export interface MonitoringStatus {
  prometheus: boolean;
  loki: boolean;
}

export interface MonitoringSummary {
  range: string;
  run_counts: Record<string, number>;
  active: number;
  p95_duration_s: number | null;
}

export interface TimeseriesResponse {
  step: number;
  series: { name: string; points: [number, number][] }[];
}

export interface LogEntry {
  ts: number;
  level: string | null;
  message: string;
  target?: string | null;
  service?: string | null;
  system?: string | null;
  run_id?: string | null;
  playbook?: string | null;
  play?: string | null;
  action?: string | null;
}

const queryString = (params: Record<string, string | number | undefined>) => {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? `?${s}` : "";
};

export const getMonitoringStatus = () => api.request<MonitoringStatus>("GET", "/api/monitoring/status");
export const getServiceMonitoringSummary = (service: string, range: string) =>
  api.request<MonitoringSummary>("GET", `/api/services/${encodeURIComponent(service)}/monitoring/summary${queryString({ range })}`);
export const getServiceMonitoringTimeseries = (service: string, range: string, system?: string) =>
  api.request<TimeseriesResponse>("GET", `/api/services/${encodeURIComponent(service)}/monitoring/timeseries${queryString({ range, system })}`);
export const getServiceLogs = (service: string, params: { range?: string; limit?: number; system?: string; run?: string; level?: string; search?: string; source?: string }) =>
  api.request<{ entries: LogEntry[] }>("GET", `/api/services/${encodeURIComponent(service)}/logs${queryString(params)}`);

export const startRun = (req: {
  runbook: string;
  filter?: string;
  backend?: string;
  image?: string;
  keep?: boolean;
}) => api.request<{ id: string }>("POST", "/api/runs", req);
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

/// A remote package shadowed by a same-named package from an earlier
/// source (local wins over remotes, remotes follow repos.wcl order).
export interface ShadowedPackage {
  name: string;
  by: string;
  source: string;
}

export const listPackages = () =>
  api.request<{ packages: PackageEntry[]; shadowed?: ShadowedPackage[]; error?: string }>(
    "GET",
    "/api/packages",
  );
export const getPackage = (name: string) =>
  api.request<PackageEntry>("GET", `/api/packages/${encodeURIComponent(name)}`);
// API docs: the server extracts the DocJson from package.wcl in-process.
export const getPackageDocs = (name: string, runbook?: string) =>
  api.request<{ doc: PackageDoc }>(
    "GET",
    runbook
      ? `/api/playbooks/${encodeURIComponent(runbook)}/packages/${encodeURIComponent(name)}/docs`
      : `/api/packages/${encodeURIComponent(name)}/docs`,
  );
export const addPackageToRunbook = (name: string, runbook: string, overwrite = false) =>
  api.request<{ playbook: string; package: string; path: string }>(
    "POST",
    `/api/packages/${encodeURIComponent(name)}/add-to-playbook`,
    { playbook: runbook, overwrite },
  );
export const startPackageTest = (
  name: string,
  req: { test?: string; backend?: string; image?: string; keep?: boolean },
) => api.request<{ id: string }>("POST", `/api/packages/${encodeURIComponent(name)}/test`, req);
export const removePackageFromRunbook = (rb: string, name: string) =>
  api.request<{ removed: string }>(
    "DELETE",
    `/api/playbooks/${encodeURIComponent(rb)}/packages/${encodeURIComponent(name)}`,
  );
export const importPackageToRepo = (rb: string, name: string) =>
  api.request<{ imported: string }>(
    "POST",
    `/api/playbooks/${encodeURIComponent(rb)}/packages/${encodeURIComponent(name)}/import`,
  );

// --- remote repositories -----------------------------------------------------

export interface RepoDef {
  name: string;
  url: string;
  subdir: string | null;
  branch: string | null;
  cloned: boolean;
  packages: number | null;
  /// Set when the initial clone failed (the entry still persists; Sync
  /// retries).
  error?: string;
}

export const listRepos = () => api.request<RepoDef[]>("GET", "/api/repos");
export const addRepo = (def: { name: string; url: string; subdir?: string; branch?: string }) =>
  api.request<RepoDef>("POST", "/api/repos", def);
export const removeRepo = (name: string) =>
  api.request<{ deleted: string }>("DELETE", `/api/repos/${encodeURIComponent(name)}`);
export const syncRepo = (name: string) =>
  api.request<RepoDef>("POST", `/api/repos/${encodeURIComponent(name)}/sync`);
export const syncAllRepos = () =>
  api.request<{ name: string; ok: boolean; error?: string }[]>("POST", "/api/repos/sync");

// --- playbook zip transfer ---------------------------------------------------
//
// Raw fetch, not api.request: the download is a binary body and the
// upload posts zip bytes — neither fits the JSON envelope helper.

const authHeaders = (): Record<string, string> => {
  const token = api.auth.token();
  return token ? { Authorization: `Bearer ${token}` } : {};
};

const envelopeError = async (res: Response, fallback: string): Promise<Error> => {
  let message = `${fallback} (${res.status})`;
  try {
    const body = await res.json();
    if (typeof body?.error === "string") message = body.error;
  } catch {
    // non-JSON body: keep the fallback
  }
  const e: any = new Error(message);
  e.status = res.status;
  return e;
};

export async function downloadRunbookZip(rb: string): Promise<void> {
  const res = await fetch(`/api/playbooks/${encodeURIComponent(rb)}/download`, {
    headers: authHeaders(),
  });
  if (!res.ok) throw await envelopeError(res, "download failed");
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement("a");
  a.href = url;
  a.download = `${rb}.zip`;
  a.click();
  URL.revokeObjectURL(url);
}

export async function uploadRunbookZip(file: File, name?: string): Promise<{ name: string }> {
  const query = name ? `?name=${encodeURIComponent(name)}` : "";
  const res = await fetch(`/api/playbooks/upload${query}`, {
    method: "POST",
    headers: { ...authHeaders(), "Content-Type": "application/zip" },
    body: file,
  });
  if (!res.ok) throw await envelopeError(res, "upload failed");
  const body = await res.json();
  return body?.data ?? body;
}
