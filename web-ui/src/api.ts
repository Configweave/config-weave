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
export const validateRunbook = (rb: string) =>
  api.request<ValidateResult>("POST", `/api/runbooks/${encodeURIComponent(rb)}/validate`);
export const runbookInventory = (rb: string) =>
  api.request<Inventory>("GET", `/api/runbooks/${encodeURIComponent(rb)}/inventory`);

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

export const runbookScope = (rb: string) => scopeAt(`/api/runbooks/${encodeURIComponent(rb)}`);
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
export const removePackageFromRunbook = (rb: string, name: string) =>
  api.request<{ removed: string }>(
    "DELETE",
    `/api/runbooks/${encodeURIComponent(rb)}/packages/${encodeURIComponent(name)}`,
  );
