// One package, from either source: the repository (--packages-dir,
// edited in place) or a runbook's installed copy (edited inside the
// runbook's pkgs/). Docs rendered natively from the inventory, test
// running/debugging, the full editing workspace, and add-to-playbook
// (repo) or remove-from-runbook (installed copy).

import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import {
  Badge,
  Button,
  Card,
  Empty,
  PageHead,
  Select,
  Table,
  toast,
} from "@forge/ui";
import { Bug, ChevronDown, ChevronRight, Play, Plus, Trash2 } from "lucide-solid";
import type { GathererDecl, PackageEntry, ParamDecl, ResourceDecl } from "../api";
import {
  addPackageToRunbook,
  getPackage,
  listRunbooks,
  packageScope,
  prefixedScope,
  removePackageFromRunbook,
  runbookInventory,
  runbookScope,
  startPackageTest,
  startRun,
} from "../api";
import { setView } from "../store";
import FileWorkspace from "./FileWorkspace";

export default function PackageView(props: { name: string; runbook?: string }) {
  // Repo source: the package's own inventory entry. Runbook source: the
  // runbook inventory, filtered.
  const [repoPkg] = createResource(
    () => (props.runbook ? undefined : props.name),
    (name) => getPackage(name).catch(() => null as PackageEntry | null),
  );
  const [rbInventory] = createResource(
    () => props.runbook,
    (rb) => runbookInventory(rb),
  );

  const info = (): PackageEntry | undefined | null =>
    props.runbook
      ? rbInventory() && (rbInventory()!.packages.find((p) => p.name === props.name) ?? null)
      : repoPkg();

  const loaded = () => (props.runbook ? rbInventory() !== undefined : repoPkg() !== undefined);

  const scope = createMemo(() =>
    props.runbook
      ? prefixedScope(runbookScope(props.runbook), `pkgs/${props.name}`)
      : packageScope(props.name),
  );

  const [busy, setBusy] = createSignal(false);

  const runTest = async (test: string | undefined, keep: boolean) => {
    setBusy(true);
    try {
      const { id } = props.runbook
        ? await startRun({
            runbook: props.runbook,
            filter: test ? `${props.name}:${test}` : props.name,
            keep,
          })
        : await startPackageTest(props.name, { test, keep });
      setView({ kind: "run", id, runbook: props.runbook ?? `pkgs:${props.name}` });
    } catch (e: any) {
      toast(e?.message ?? "cannot start the test run", { tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const removeFromRunbook = async () => {
    if (!props.runbook) return;
    if (!confirm(`Remove ${props.name} from ${props.runbook}? Its pkgs/ copy is deleted.`))
      return;
    try {
      await removePackageFromRunbook(props.runbook, props.name);
      toast(`removed ${props.name}`, { tone: "success" });
      setView({ kind: "runbook", name: props.runbook });
    } catch (e: any) {
      toast(e?.message ?? "remove failed", { tone: "danger" });
    }
  };

  return (
    <Show
      when={!loaded() || info()}
      fallback={
        <Empty
          title={props.runbook ? "Package not installed" : "No such package"}
          action={
            <Button
              size="sm"
              onClick={() =>
                props.runbook
                  ? setView({ kind: "runbook", name: props.runbook })
                  : setView({ kind: "packages" })
              }
            >
              {props.runbook ? `Back to ${props.runbook}` : "Back to packages"}
            </Button>
          }
        >
          <span class="sub">It may have been removed.</span>
        </Empty>
      }
    >
      <PageHead
        title={props.name}
        sub={
          <span>
            {info()?.description}{" "}
            <Badge tone={props.runbook ? "info" : "neutral"}>
              {props.runbook ? `installed in ${props.runbook}` : "repository"}
            </Badge>
          </span>
        }
        actions={
          <div class="head-actions">
            <Show when={(info()?.tests ?? []).length > 0}>
              <Button
                size="sm"
                icon={Play}
                disabled={busy()}
                onClick={() => runTest(undefined, false)}
              >
                Run all tests
              </Button>
            </Show>
            <Show when={props.runbook}>
              <Button size="sm" variant="ghost" icon={Trash2} onClick={removeFromRunbook}>
                Remove from runbook
              </Button>
            </Show>
          </div>
        }
      />

      <Card title="Resources">
        <Show
          when={(info()?.resources ?? []).length > 0}
          fallback={<Empty title="No resources declared" />}
        >
          <For each={info()?.resources ?? []}>{(r) => <ResourceDocs resource={r} />}</For>
        </Show>
      </Card>

      <Show when={(info()?.gatherers ?? []).length > 0}>
        <Card title="Gatherers">
          <For each={info()?.gatherers ?? []}>{(g) => <GathererDocs gatherer={g} />}</For>
        </Card>
      </Show>

      <Card title="Tests">
        <Show
          when={(info()?.tests ?? []).length > 0}
          fallback={<Empty title="No tests declared" />}
        >
          <Table>
            <thead>
              <tr>
                <th>Test</th>
                <th>Backend</th>
                <th>Image</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <For each={info()?.tests ?? []}>
                {(t) => (
                  <tr>
                    <td>
                      <div>{t.name}</div>
                      <div class="sub">{t.description}</div>
                    </td>
                    <td>
                      <Badge tone={t.backend === "vmlab" ? "info" : "neutral"}>{t.backend}</Badge>
                    </td>
                    <td class="mono">{t.image}</td>
                    <td>
                      <div class="row-actions">
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={Play}
                          disabled={busy()}
                          onClick={() => runTest(t.name, false)}
                        >
                          Run
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={Bug}
                          disabled={busy()}
                          title="Run and keep the instance for troubleshooting"
                          onClick={() => runTest(t.name, true)}
                        >
                          Debug
                        </Button>
                      </div>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>

      <Card title="Files">
        <FileWorkspace scope={scope()} />
      </Card>

      <Show when={!props.runbook}>
        <AddToPlaybook package={props.name} />
      </Show>
    </Show>
  );
}

function ParamsTable(props: { params: ParamDecl[] }) {
  return (
    <Show when={props.params.length > 0} fallback={<div class="sub">No parameters.</div>}>
      <Table>
        <thead>
          <tr>
            <th>Param</th>
            <th>Type</th>
            <th>Required</th>
            <th>Default</th>
            <th>Description</th>
          </tr>
        </thead>
        <tbody>
          <For each={props.params}>
            {(p) => (
              <tr>
                <td class="mono">{p.name}</td>
                <td class="mono">{p.type}</td>
                <td>
                  <Show when={p.required} fallback={<span class="sub">optional</span>}>
                    <Badge tone="warning">required</Badge>
                  </Show>
                </td>
                <td class="mono">{p.default == null ? "" : JSON.stringify(p.default)}</td>
                <td class="sub">{p.description}</td>
              </tr>
            )}
          </For>
        </tbody>
      </Table>
    </Show>
  );
}

function ResourceDocs(props: { resource: ResourceDecl }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="doc-block">
      <div class="doc-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span class="mono">{props.resource.name}</span>
        <Badge tone="neutral">{props.resource.concurrency}</Badge>
        <span class="sub">{props.resource.description}</span>
      </div>
      <Show when={open()}>
        <div class="doc-body">
          <ParamsTable params={props.resource.params} />
        </div>
      </Show>
    </div>
  );
}

function GathererDocs(props: { gatherer: GathererDecl }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="doc-block">
      <div class="doc-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span class="mono">{props.gatherer.name}</span>
        <span class="sub">{props.gatherer.description}</span>
      </div>
      <Show when={open()}>
        <div class="doc-body">
          <ParamsTable params={props.gatherer.params} />
        </div>
      </Show>
    </div>
  );
}

function AddToPlaybook(props: { package: string }) {
  const [runbooks] = createResource(listRunbooks);
  const [target, setTarget] = createSignal("");
  const [adding, setAdding] = createSignal(false);

  const add = async (overwrite: boolean) => {
    if (!target()) return;
    setAdding(true);
    try {
      const res = await addPackageToRunbook(props.package, target(), overwrite);
      toast(`copied to ${res.runbook}/${res.path}`, { tone: "success" });
    } catch (e: any) {
      const msg: string = e?.message ?? "copy failed";
      if (!overwrite && msg.includes("already in the runbook")) {
        if (confirm(`${props.package} is already in ${target()} — overwrite it?`)) {
          setAdding(false);
          return add(true);
        }
      } else {
        toast(msg, { tone: "danger" });
      }
    } finally {
      setAdding(false);
    }
  };

  return (
    <Card title="Add to playbook">
      <div class="add-to-playbook">
        <Select
          placeholder="pick a runbook"
          options={(runbooks() ?? []).map((r) => ({ value: r.name, label: r.name }))}
          value={target()}
          onChange={setTarget}
        />
        <Button icon={Plus} disabled={!target() || adding()} onClick={() => add(false)}>
          Add
        </Button>
      </div>
      <div class="sub" style={{ "margin-top": "8px" }}>
        Copies the package into the runbook's <span class="mono">pkgs/</span> folder.
      </div>
    </Card>
  );
}
