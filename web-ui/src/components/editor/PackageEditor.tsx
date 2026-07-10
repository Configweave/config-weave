// The graphical package editor: metadata, gatherers, resources (params
// tables), tests (steps with expectations, gathers with expect maps),
// and scenarios. Script fields are package-relative paths with a
// scaffold action that creates a starter wscript when missing.

import { For, Show, createSignal } from "solid-js";
import { Badge, Button, Input, Select, Textarea, toast } from "@forge/ui";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FilePlus2,
  Plus,
  Trash2,
} from "lucide-solid";
import type {
  Kv,
  PackageDoc,
  ParamDoc,
  ResourceDoc,
  TestDocEd,
  TestGatherDoc,
  TestStepDoc,
  Val,
  WorkspaceScope,
} from "../../api";
import { getTemplates } from "../../api";
import type { ParamSchema } from "./PropertiesEditor";
import KeyValueEditor from "./KeyValueEditor";
import PropertiesEditor from "./PropertiesEditor";
import ValueInput from "./ValueInput";
import { move } from "./PlaybookEditor";

type Mutate = (fn: (d: PackageDoc) => void) => void;

const EXPECTS = ["converge", "already_configured", "error", "skip", "reboot_required"];
const TYPES = ["string", "int", "float", "bool", "list", "map"];

function paramSchemas(doc: PackageDoc, resource: string): ParamSchema[] {
  const r = doc.resources.find((x) => x.name === resource);
  return (r?.params ?? []).map((p) => ({
    name: p.name,
    description: p.description,
    type: p.type,
    required: p.required ?? false,
    default: p.default && "lit" in p.default ? p.default.lit : null,
  }));
}

/// Create `script` (package-relative) from a scaffold template unless
/// it already exists.
function ScriptField(props: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  scope: WorkspaceScope;
  /// The package dir inside the workspace ("" when the package is the
  /// workspace root).
  pkgDir: string;
  template: "resource_script" | "gatherer_script" | "verify_script";
}) {
  const [busy, setBusy] = createSignal(false);
  const scaffold = async () => {
    const rel = props.value.trim();
    if (!rel) {
      toast("enter a script path first", { tone: "warning" });
      return;
    }
    const full = props.pkgDir ? `${props.pkgDir}/${rel}` : rel;
    setBusy(true);
    try {
      await props.scope.read(full);
      toast(`${rel} already exists`, { tone: "info" });
    } catch {
      try {
        const templates = await getTemplates();
        await props.scope.write(full, templates[props.template] ?? "");
        toast(`scaffolded ${rel}`, { tone: "success" });
      } catch (e: any) {
        toast(e?.message ?? "scaffold failed", { tone: "danger" });
      }
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="script-field">
      <Input
        class="mono-input"
        label={props.label}
        placeholder="resources/name.wscript"
        value={props.value}
        onInput={(e) => props.onChange(e.currentTarget.value)}
      />
      <Button size="sm" variant="ghost" icon={FilePlus2} disabled={busy()} onClick={scaffold}>
        Scaffold
      </Button>
    </div>
  );
}

export default function PackageEditor(props: {
  doc: PackageDoc;
  scope: WorkspaceScope;
  /// The package's directory inside the workspace ("" at a package
  /// root, "pkgs/<name>" inside a runbook workspace).
  pkgDir: string;
  mutate: Mutate;
}) {
  return (
    <div class="visual-editor">
      <section class="ve-section">
        <div class="ve-heading">Package</div>
        <Input
          label="Name (must match the folder name)"
          value={props.doc.name}
          onInput={(e) => props.mutate((d) => (d.name = e.currentTarget.value))}
        />
        <Textarea
          label="Description"
          rows={2}
          value={props.doc.description}
          onInput={(e) => props.mutate((d) => (d.description = e.currentTarget.value))}
        />
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Resources
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.resources.push({
                  name: "",
                  description: "",
                  script: "",
                  params: [],
                } as ResourceDoc),
              )
            }
          >
            Add resource
          </Button>
        </div>
        <For each={props.doc.resources}>
          {(r, i) => (
            <ResourceForm
              resource={r}
              scope={props.scope}
              pkgDir={props.pkgDir}
              onRemove={() => props.mutate((d) => d.resources.splice(i(), 1))}
              mutateResource={(fn) => props.mutate((d) => fn(d.resources[i()]))}
            />
          )}
        </For>
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Gatherers
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.gatherers.push({ name: "", description: "", script: "", params: [] }),
              )
            }
          >
            Add gatherer
          </Button>
        </div>
        <For each={props.doc.gatherers}>
          {(g, i) => (
            <div class="ve-block">
              <div class="ve-block-head">
                <Badge tone="info">gatherer</Badge>
                <span class="mono">{g.name || "(unnamed)"}</span>
                <span class="ve-spacer" />
                <Button
                  size="sm"
                  variant="ghost"
                  icon={Trash2}
                  onClick={() => props.mutate((d) => d.gatherers.splice(i(), 1))}
                />
              </div>
              <div class="ve-block-body">
                <div class="form-grid">
                  <Input
                    label="Name"
                    value={g.name}
                    onInput={(e) =>
                      props.mutate((d) => (d.gatherers[i()].name = e.currentTarget.value))
                    }
                  />
                  <Input
                    label="Description"
                    value={g.description}
                    onInput={(e) =>
                      props.mutate((d) => (d.gatherers[i()].description = e.currentTarget.value))
                    }
                  />
                </div>
                <ScriptField
                  label="Script"
                  value={g.script}
                  onChange={(v) => props.mutate((d) => (d.gatherers[i()].script = v))}
                  scope={props.scope}
                  pkgDir={props.pkgDir}
                  template="gatherer_script"
                />
                <ParamsTable
                  params={g.params}
                  mutateParams={(fn) => props.mutate((d) => fn(d.gatherers[i()].params))}
                />
              </div>
            </div>
          )}
        </For>
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Tests
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.tests.push({
                  name: "",
                  description: "",
                  image: "",
                  steps: [],
                  gathers: [],
                } as TestDocEd),
              )
            }
          >
            Add test
          </Button>
        </div>
        <For each={props.doc.tests}>
          {(t, i) => (
            <TestForm
              test={t}
              doc={props.doc}
              scope={props.scope}
              pkgDir={props.pkgDir}
              onRemove={() => props.mutate((d) => d.tests.splice(i(), 1))}
              mutateTest={(fn) => props.mutate((d) => fn(d.tests[i()]))}
            />
          )}
        </For>
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Scenarios
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.scenarios.push({ name: "", description: "", lab: "", script: "" }),
              )
            }
          >
            Add scenario
          </Button>
        </div>
        <For each={props.doc.scenarios}>
          {(s, i) => (
            <div class="ve-block">
              <div class="ve-block-head">
                <Badge tone="info">scenario</Badge>
                <span class="mono">{s.name || "(unnamed)"}</span>
                <span class="ve-spacer" />
                <Button
                  size="sm"
                  variant="ghost"
                  icon={Trash2}
                  onClick={() => props.mutate((d) => d.scenarios.splice(i(), 1))}
                />
              </div>
              <div class="ve-block-body">
                <div class="form-grid">
                  <Input
                    label="Name"
                    value={s.name}
                    onInput={(e) =>
                      props.mutate((d) => (d.scenarios[i()].name = e.currentTarget.value))
                    }
                  />
                  <Input
                    label="Description"
                    value={s.description}
                    onInput={(e) =>
                      props.mutate((d) => (d.scenarios[i()].description = e.currentTarget.value))
                    }
                  />
                </div>
                <div class="form-grid">
                  <Input
                    class="mono-input"
                    label="Lab dir (contains vmlab.wcl)"
                    value={s.lab}
                    onInput={(e) =>
                      props.mutate((d) => (d.scenarios[i()].lab = e.currentTarget.value))
                    }
                  />
                  <Input
                    class="mono-input"
                    label="Driver script"
                    value={s.script}
                    onInput={(e) =>
                      props.mutate((d) => (d.scenarios[i()].script = e.currentTarget.value))
                    }
                  />
                </div>
              </div>
            </div>
          )}
        </For>
      </section>
    </div>
  );
}

function ResourceForm(props: {
  resource: ResourceDoc;
  scope: WorkspaceScope;
  pkgDir: string;
  onRemove: () => void;
  mutateResource: (fn: (r: ResourceDoc) => void) => void;
}) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone="neutral">resource</Badge>
        <span class="mono">{props.resource.name || "(unnamed)"}</span>
        <span class="sub">{props.resource.description}</span>
        <span class="ve-spacer" />
        <Button
          size="sm"
          variant="ghost"
          icon={Trash2}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            props.onRemove();
          }}
        />
      </div>
      <Show when={open()}>
        <div class="ve-block-body">
          <div class="form-grid">
            <Input
              label="Name"
              value={props.resource.name}
              onInput={(e) => props.mutateResource((r) => (r.name = e.currentTarget.value))}
            />
            <Input
              label="Description"
              value={props.resource.description}
              onInput={(e) =>
                props.mutateResource((r) => (r.description = e.currentTarget.value))
              }
            />
            <Select
              label="Concurrency"
              options={[
                { value: "", label: "parallel (default)" },
                { value: "parallel", label: "parallel" },
                { value: "exclusive", label: "exclusive" },
                { value: "global", label: "global" },
              ]}
              value={props.resource.concurrency ?? ""}
              onChange={(v) => props.mutateResource((r) => (r.concurrency = v || undefined))}
            />
          </div>
          <ScriptField
            label="Script"
            value={props.resource.script}
            onChange={(v) => props.mutateResource((r) => (r.script = v))}
            scope={props.scope}
            pkgDir={props.pkgDir}
            template="resource_script"
          />
          <ParamsTable
            params={props.resource.params}
            mutateParams={(fn) => props.mutateResource((r) => fn(r.params))}
          />
        </div>
      </Show>
    </div>
  );
}

function ParamsTable(props: {
  params: ParamDoc[];
  mutateParams: (fn: (params: ParamDoc[]) => void) => void;
}) {
  return (
    <div class="ffield">
      <span class="ffield-label">Params</span>
      <For each={props.params}>
        {(p, i) => (
          <div class="param-row">
            <Input
              class="mono-input"
              placeholder="name"
              value={p.name}
              onInput={(e) => props.mutateParams((ps) => (ps[i()].name = e.currentTarget.value))}
            />
            <Select
              options={TYPES.map((t) => ({ value: t, label: t }))}
              value={p.type}
              onChange={(v) => props.mutateParams((ps) => (ps[i()].type = v))}
            />
            <Select
              options={[
                { value: "optional", label: "optional" },
                { value: "required", label: "required" },
              ]}
              value={p.required ? "required" : "optional"}
              onChange={(v) =>
                props.mutateParams((ps) => (ps[i()].required = v === "required" || undefined))
              }
            />
            <ValueInput
              value={p.default}
              type={p.type}
              placeholder="no default"
              onChange={(v: Val | undefined) => props.mutateParams((ps) => (ps[i()].default = v))}
            />
            <Input
              placeholder="description"
              value={p.description}
              onInput={(e) =>
                props.mutateParams((ps) => (ps[i()].description = e.currentTarget.value))
              }
            />
            <Button
              size="sm"
              variant="ghost"
              icon={Trash2}
              onClick={() => props.mutateParams((ps) => ps.splice(i(), 1))}
            />
          </div>
        )}
      </For>
      <Button
        size="sm"
        variant="ghost"
        icon={Plus}
        onClick={() =>
          props.mutateParams((ps) => ps.push({ name: "", description: "", type: "string" }))
        }
      >
        Add param
      </Button>
    </div>
  );
}

function TestForm(props: {
  test: TestDocEd;
  doc: PackageDoc;
  scope: WorkspaceScope;
  pkgDir: string;
  onRemove: () => void;
  mutateTest: (fn: (t: TestDocEd) => void) => void;
}) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone={props.test.backend === "vmlab" ? "info" : "neutral"}>
          {props.test.backend ?? "docker"}
        </Badge>
        <span class="mono">{props.test.name || "(unnamed)"}</span>
        <span class="sub">{props.test.description}</span>
        <span class="ve-spacer" />
        <Button
          size="sm"
          variant="ghost"
          icon={Trash2}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            props.onRemove();
          }}
        />
      </div>
      <Show when={open()}>
        <div class="ve-block-body">
          <div class="form-grid">
            <Input
              label="Name"
              value={props.test.name}
              onInput={(e) => props.mutateTest((t) => (t.name = e.currentTarget.value))}
            />
            <Input
              label="Description"
              value={props.test.description}
              onInput={(e) => props.mutateTest((t) => (t.description = e.currentTarget.value))}
            />
          </div>
          <div class="form-grid">
            <Select
              label="Backend"
              options={[
                { value: "", label: "docker (default)" },
                { value: "docker", label: "docker" },
                { value: "vmlab", label: "vmlab" },
              ]}
              value={props.test.backend ?? ""}
              onChange={(v) => props.mutateTest((t) => (t.backend = v || undefined))}
            />
            <Input
              class="mono-input"
              label="Image / template"
              placeholder='debian:12 or "x86_64/linux-modern"'
              value={props.test.image}
              onInput={(e) => props.mutateTest((t) => (t.image = e.currentTarget.value))}
            />
            <Input
              label="Group (share one instance)"
              value={props.test.group ?? ""}
              onInput={(e) =>
                props.mutateTest((t) => (t.group = e.currentTarget.value || undefined))
              }
            />
          </div>
          <Textarea
            class="mono"
            label="Setup (shell run in the instance before the test)"
            rows={2}
            value={props.test.setup ?? ""}
            onInput={(e) =>
              props.mutateTest((t) => (t.setup = e.currentTarget.value || undefined))
            }
          />
          <ScriptField
            label="Verify script (optional)"
            value={props.test.verify ?? ""}
            onChange={(v) => props.mutateTest((t) => (t.verify = v || undefined))}
            scope={props.scope}
            pkgDir={props.pkgDir}
            template="verify_script"
          />

          <div class="ffield">
            <span class="ffield-label">Steps</span>
            <For each={props.test.steps}>
              {(s, i) => (
                <TestStepForm
                  step={s}
                  doc={props.doc}
                  siblings={props.test.steps
                    .filter((x) => x.name && x.name !== s.name)
                    .map((x) => x.name)}
                  onMove={(delta) => props.mutateTest((t) => move(t.steps, i(), delta))}
                  onRemove={() => props.mutateTest((t) => t.steps.splice(i(), 1))}
                  mutateStep={(fn) => props.mutateTest((t) => fn(t.steps[i()]))}
                />
              )}
            </For>
            <Button
              size="sm"
              variant="ghost"
              icon={Plus}
              onClick={() =>
                props.mutateTest((t) =>
                  t.steps.push({
                    name: "",
                    description: "",
                    resource: "",
                    requires: [],
                    properties: [],
                  }),
                )
              }
            >
              Add step
            </Button>
          </div>

          <div class="ffield">
            <span class="ffield-label">Gathers</span>
            <For each={props.test.gathers}>
              {(g, i) => (
                <div class="ve-item">
                  <div class="form-grid">
                    <Input
                      label="Name"
                      value={g.name}
                      onInput={(e) =>
                        props.mutateTest((t) => (t.gathers[i()].name = e.currentTarget.value))
                      }
                    />
                    <Input
                      label="Description"
                      value={g.description}
                      onInput={(e) =>
                        props.mutateTest(
                          (t) => (t.gathers[i()].description = e.currentTarget.value),
                        )
                      }
                    />
                    <Select
                      label="Gatherer"
                      options={props.doc.gatherers.map((x) => ({
                        value: x.name,
                        label: x.name,
                      }))}
                      value={g.from}
                      onChange={(v) => props.mutateTest((t) => (t.gathers[i()].from = v))}
                    />
                  </div>
                  <div class="ffield">
                    <span class="ffield-label">Params</span>
                    <KeyValueEditor
                      kvs={g.params}
                      onChange={(kvs) => props.mutateTest((t) => (t.gathers[i()].params = kvs))}
                    />
                  </div>
                  <div class="ffield">
                    <span class="ffield-label">Expect (key equality assertions)</span>
                    <KeyValueEditor
                      kvs={g.expect}
                      onChange={(kvs) => props.mutateTest((t) => (t.gathers[i()].expect = kvs))}
                    />
                  </div>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Trash2}
                    onClick={() => props.mutateTest((t) => t.gathers.splice(i(), 1))}
                  >
                    Remove gather
                  </Button>
                </div>
              )}
            </For>
            <Button
              size="sm"
              variant="ghost"
              icon={Plus}
              onClick={() =>
                props.mutateTest((t) =>
                  t.gathers.push({
                    name: "",
                    description: "",
                    from: "",
                    params: [],
                    expect: [],
                  } as TestGatherDoc),
                )
              }
            >
              Add gather
            </Button>
          </div>
        </div>
      </Show>
    </div>
  );
}

function TestStepForm(props: {
  step: TestStepDoc;
  doc: PackageDoc;
  siblings: string[];
  onMove: (delta: number) => void;
  onRemove: () => void;
  mutateStep: (fn: (s: TestStepDoc) => void) => void;
}) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone="neutral">step</Badge>
        <span class="mono">{props.step.name || "(unnamed)"}</span>
        <Badge tone="info">{props.step.expect ?? "converge"}</Badge>
        <span class="ve-spacer" />
        <Button
          size="sm"
          variant="ghost"
          icon={ArrowUp}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            props.onMove(-1);
          }}
        />
        <Button
          size="sm"
          variant="ghost"
          icon={ArrowDown}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            props.onMove(1);
          }}
        />
        <Button
          size="sm"
          variant="ghost"
          icon={Trash2}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            props.onRemove();
          }}
        />
      </div>
      <Show when={open()}>
        <div class="ve-block-body">
          <div class="form-grid">
            <Input
              label="Name"
              value={props.step.name}
              onInput={(e) => props.mutateStep((s) => (s.name = e.currentTarget.value))}
            />
            <Input
              label="Description"
              value={props.step.description}
              onInput={(e) => props.mutateStep((s) => (s.description = e.currentTarget.value))}
            />
          </div>
          <div class="form-grid">
            <Select
              label="Resource (this package)"
              options={props.doc.resources.map((r) => ({ value: r.name, label: r.name }))}
              value={props.step.resource}
              onChange={(v) => props.mutateStep((s) => (s.resource = v))}
            />
            <Select
              label="Expect"
              options={EXPECTS.map((x) => ({ value: x, label: x }))}
              value={props.step.expect ?? "converge"}
              onChange={(v) =>
                props.mutateStep((s) => (s.expect = v === "converge" ? undefined : v))
              }
            />
          </div>
          <Input
            class="mono-input"
            label="Condition (WCL expression, empty = always)"
            value={props.step.condition ?? ""}
            onInput={(e) =>
              props.mutateStep((s) => (s.condition = e.currentTarget.value || undefined))
            }
          />
          <div class="ffield">
            <span class="ffield-label">Properties</span>
            <PropertiesEditor
              params={paramSchemas(props.doc, props.step.resource)}
              kvs={props.step.properties}
              onChange={(kvs) => props.mutateStep((s) => (s.properties = kvs))}
            />
          </div>
        </div>
      </Show>
    </div>
  );
}
