// One runbook: the shared editing workspace (pkgs/ hidden — package
// contents live in the Packages area), an installed-packages card, a
// validate action, and a tests panel (from the CLI inventory) to
// launch runs.

import { For, Show, createResource, createSignal } from "solid-js";
import { Alert, Badge, Button, Card, Checkbox, Empty, PageHead, Select, toast } from "@forge/ui";
import { Download, FileDown, Pencil, Play, Plus, Trash2 } from "lucide-solid";
import type { ValidateResult } from "../api";
import {
  addPackageToRunbook,
  downloadRunbookZip,
  importPackageToRepo,
  listPackages,
  removePackageFromRunbook,
  runbookInventory,
  runbookScope,
  startRun,
  validateRunbook,
} from "../api";
import { setView } from "../store";
import FileWorkspace from "./FileWorkspace";

export default function RunbookView(props: { name: string }) {
  const [inventory, { refetch: refetchInventory }] = createResource(
    () => props.name,
    runbookInventory,
  );
  const [diags, setDiags] = createSignal<ValidateResult | null>(null);
  const [validating, setValidating] = createSignal(false);
  const [keep, setKeep] = createSignal(false);
  const [pkgReload, setPkgReload] = createSignal(0);

  const validate = async () => {
    setValidating(true);
    try {
      setDiags(await validateRunbook(props.name));
    } catch (e: any) {
      toast(e?.message ?? "validate failed", { tone: "danger" });
    } finally {
      setValidating(false);
    }
  };

  const launch = async (filter?: string) => {
    try {
      const { id } = await startRun({ runbook: props.name, filter, keep: keep() });
      setView({ kind: "run", id, runbook: props.name });
    } catch (e: any) {
      toast(e?.message ?? "cannot start the run", { tone: "danger" });
    }
  };

  const removePackage = async (name: string) => {
    if (!confirm(`Remove package "${name}" from ${props.name}? Its pkgs/ copy is deleted.`))
      return;
    try {
      await removePackageFromRunbook(props.name, name);
      toast(`removed ${name}`, { tone: "success" });
      void refetchInventory();
      setPkgReload((n) => n + 1);
    } catch (e: any) {
      toast(e?.message ?? "remove failed", { tone: "danger" });
    }
  };

  // The repository, for the add-picker and the not-in-repo detection.
  // null = unconfigured (both affordances hidden).
  const [repo, { refetch: refetchRepo }] = createResource(() =>
    listPackages().catch((e: any) => {
      if (e?.status === 404) return null;
      throw e;
    }),
  );
  const [pickerChoice, setPickerChoice] = createSignal("");
  const [pkgBusy, setPkgBusy] = createSignal(false);

  const installedNames = () => (inventory()?.packages ?? []).map((p) => p.name);
  const addable = () =>
    (repo()?.packages ?? [])
      .map((p) => p.name)
      .filter((n) => !installedNames().includes(n));
  const inRepo = (name: string) =>
    (repo()?.packages ?? []).some((p) => p.name === name);

  const addPackage = async () => {
    const name = pickerChoice();
    if (!name) return;
    setPkgBusy(true);
    try {
      await addPackageToRunbook(name, props.name);
      toast(`added ${name}`, { tone: "success" });
      setPickerChoice("");
      void refetchInventory();
      setPkgReload((n) => n + 1);
    } catch (e: any) {
      toast(e?.message ?? "add failed", { tone: "danger" });
    } finally {
      setPkgBusy(false);
    }
  };

  const importPackage = async (name: string) => {
    setPkgBusy(true);
    try {
      await importPackageToRepo(props.name, name);
      toast(`imported ${name} into the repository`, { tone: "success" });
      void refetchRepo();
    } catch (e: any) {
      toast(e?.message ?? "import failed", { tone: "danger" });
    } finally {
      setPkgBusy(false);
    }
  };

  return (
    <>
      <PageHead
        title={props.name}
        sub={inventory()?.description || "playbook"}
        actions={
          <div class="head-actions">
            <Button
              size="sm"
              icon={FileDown}
              title="Download the playbook (pkgs/ included) as a zip"
              onClick={() =>
                downloadRunbookZip(props.name).catch((e: any) =>
                  toast(e?.message ?? "download failed", { tone: "danger" }),
                )
              }
            >
              Download
            </Button>
            <Button size="sm" onClick={validate} disabled={validating()}>
              {validating() ? "Validating…" : "Validate"}
            </Button>
            <Button size="sm" variant="primary" icon={Play} onClick={() => launch()}>
              Run all tests
            </Button>
          </div>
        }
      />

      <Show when={diags()} keyed>
        {(v) => (
          <Show when={!v.ok} fallback={<Alert tone="success" title="Validation passed" />}>
            <Alert tone="danger" title={`Validation failed (${v.diags.length} error(s))`}>
              <pre class="diag-pre">{v.diags.map((d) => d.rendered).join("\n\n")}</pre>
            </Alert>
          </Show>
        )}
      </Show>

      <FileWorkspace
        scope={runbookScope(props.name)}
        inventory={inventory()}
        hideTopLevel={["pkgs"]}
        reloadKey={pkgReload()}
      />

      <Card
        title="Installed packages"
        action={
          <Show when={repo() !== null}>
            <div class="pkg-add-row">
              <Select
                placeholder={addable().length ? "add from repository…" : "repository in sync"}
                options={addable().map((n) => ({ value: n, label: n }))}
                value={pickerChoice()}
                onChange={setPickerChoice}
              />
              <Button
                size="sm"
                icon={Plus}
                disabled={!pickerChoice() || pkgBusy()}
                onClick={addPackage}
              >
                Add
              </Button>
            </div>
          </Show>
        }
      >
        <Show
          when={(inventory()?.packages ?? []).length > 0}
          fallback={
            <Empty title="No packages installed">
              <span class="sub">Pick one from the repository above.</span>
            </Empty>
          }
        >
          <For each={inventory()?.packages ?? []}>
            {(pkg) => (
              <div class="pkg-row">
                <span class="mono">{pkg.name}</span>
                <span class="sub">{pkg.description}</span>
                <Show when={repo() !== null && repo() !== undefined && !inRepo(pkg.name)}>
                  <Badge tone="warning">not in repository</Badge>
                </Show>
                <span class="ve-spacer" />
                <Show when={repo() !== null && repo() !== undefined && !inRepo(pkg.name)}>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Download}
                    disabled={pkgBusy()}
                    title="Copy this package into the repository"
                    onClick={() => importPackage(pkg.name)}
                  >
                    Import to repo
                  </Button>
                </Show>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={Pencil}
                  onClick={() =>
                    setView({ kind: "package", name: pkg.name, runbook: props.name })
                  }
                >
                  Edit
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={Trash2}
                  onClick={() => removePackage(pkg.name)}
                >
                  Remove
                </Button>
              </div>
            )}
          </For>
        </Show>
      </Card>

      <Card
        title="Tests"
        action={
          <Checkbox checked={keep()} onChange={setKeep}>
            keep instances (post-mortem)
          </Checkbox>
        }
      >
        <Show
          when={(inventory()?.packages ?? []).some(
            (p) => p.tests.length > 0 || p.scenarios.length > 0,
          )}
          fallback={<Empty title="No tests declared" />}
        >
          <For each={inventory()?.packages ?? []}>
            {(pkg) => (
              <Show when={pkg.tests.length > 0 || pkg.scenarios.length > 0}>
                <div class="pkg-tests">
                  <div class="pkg-head">
                    <strong>{pkg.name}</strong>
                    <span class="sub">{pkg.description}</span>
                    <Button size="sm" variant="ghost" onClick={() => launch(pkg.name)}>
                      Run package
                    </Button>
                  </div>
                  <For each={pkg.tests}>
                    {(t) => (
                      <div class="test-row">
                        <span>{t.name}</span>
                        <Badge tone={t.backend === "vmlab" ? "info" : "neutral"}>
                          {t.backend}
                        </Badge>
                        <span class="test-image">{t.image}</span>
                        <Button
                          size="sm"
                          icon={Play}
                          onClick={() => launch(`${pkg.name}:${t.name}`)}
                        >
                          Run
                        </Button>
                      </div>
                    )}
                  </For>
                  <For each={pkg.scenarios}>
                    {(s) => (
                      <div class="test-row">
                        <span>{s.name}</span>
                        <Badge tone="info">scenario</Badge>
                        <span class="test-image">{s.description}</span>
                        <Button
                          size="sm"
                          icon={Play}
                          onClick={() => launch(`${pkg.name}:${s.name}`)}
                        >
                          Run
                        </Button>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            )}
          </For>
        </Show>
      </Card>
    </>
  );
}
