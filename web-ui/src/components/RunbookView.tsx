// One runbook: file tree | tabbed editor (source CodeMirror, plus a
// visual form editor for playbook.wcl / package.wcl backed by the
// DocJson endpoints), with validate and a tests panel (from the CLI
// inventory) to launch runs.

import { For, Show, createResource, createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Empty,
  PageHead,
  SplitPane,
  Tabs,
  ToggleGroup,
  toast,
} from "@forge/ui";
import { CodeEditor } from "@forge/code";
import { ChevronDown, ChevronRight, FileText, FlaskConical, Play } from "lucide-solid";
import type { PackageDoc, PlaybookDoc, TreeNode, ValidateResult } from "../api";
import {
  docParse,
  docRender,
  docSave,
  readFile,
  runbookInventory,
  runbookTree,
  startRun,
  validateRunbook,
  writeFile,
} from "../api";
import { setView } from "../store";
import { wclLanguage } from "../wcl-language";
import PlaybookEditor from "./editor/PlaybookEditor";
import PackageEditor from "./editor/PackageEditor";

interface OpenFile {
  path: string;
  content: string;
  dirty: boolean;
  /// Visual editing state (playbook.wcl / package.wcl only).
  mode: "source" | "visual";
  docKind?: "playbook" | "package";
  doc?: PlaybookDoc | PackageDoc;
  docDirty?: boolean;
  baseHash?: string;
  visualError?: string;
}

/// Which DocJson kind a path edits (mirrors the server's rule).
function docKindFor(path: string): "playbook" | "package" | undefined {
  const base = path.split("/").pop();
  if (base === "playbook.wcl") return "playbook";
  if (base === "package.wcl") return "package";
  return undefined;
}

function languageFor(path: string) {
  if (/\.(wcl|wisp|wscript|wscripti|wispi)$/.test(path)) return wclLanguage;
  if (path.endsWith(".json")) return "json" as const;
  if (path.endsWith(".md")) return "markdown" as const;
  return undefined;
}

function TreeEntry(props: {
  node: TreeNode;
  prefix: string;
  open: (path: string) => void;
  selected: string | null;
}) {
  const [expanded, setExpanded] = createSignal(true);
  const path = () => (props.prefix ? `${props.prefix}/${props.node.name}` : props.node.name);
  return (
    <Show
      when={props.node.dir}
      fallback={
        <button
          type="button"
          class="tree-file"
          classList={{ "is-active": props.selected === path() }}
          onClick={() => props.open(path())}
        >
          <FileText size={13} /> {props.node.name}
        </button>
      }
    >
      <div>
        <button type="button" class="tree-dir" onClick={() => setExpanded(!expanded())}>
          {expanded() ? <ChevronDown size={13} /> : <ChevronRight size={13} />} {props.node.name}
        </button>
        <Show when={expanded()}>
          <div class="tree-children">
            <For each={props.node.children ?? []}>
              {(child) => (
                <TreeEntry
                  node={child}
                  prefix={path()}
                  open={props.open}
                  selected={props.selected}
                />
              )}
            </For>
          </div>
        </Show>
      </div>
    </Show>
  );
}

export default function RunbookView(props: { name: string }) {
  const [tree] = createResource(() => props.name, runbookTree);
  const [inventory] = createResource(() => props.name, runbookInventory);
  const [files, setFiles] = createStore<OpenFile[]>([]);
  const [active, setActive] = createSignal<string | null>(null);
  const [diags, setDiags] = createSignal<ValidateResult | null>(null);
  const [validating, setValidating] = createSignal(false);
  const [keep, setKeep] = createSignal(false);

  const current = () => files.find((f) => f.path === active()) ?? null;

  const open = async (path: string) => {
    if (!files.some((f) => f.path === path)) {
      try {
        const { content } = await readFile(props.name, path);
        const kind = docKindFor(path);
        setFiles(files.length, {
          path,
          content,
          dirty: false,
          mode: "source",
          docKind: kind,
        });
        // Doc-kind files open in the visual editor when they parse.
        if (kind) await toVisual(path);
      } catch (e: any) {
        toast(e?.message ?? `cannot open ${path}`, { tone: 'danger' });
        return;
      }
    }
    setActive(path);
  };

  /// Source → Visual: parse the current buffer into a DocJson.
  const toVisual = async (path: string) => {
    const file = files.find((f) => f.path === path);
    if (!file?.docKind) return;
    try {
      const res = await docParse(props.name, path, file.dirty ? file.content : undefined);
      setFiles(
        (f) => f.path === path,
        produce((f) => {
          if (res.ok && res.doc) {
            f.mode = "visual";
            f.doc = res.doc;
            f.docDirty = f.dirty;
            f.baseHash = res.base_hash;
            f.visualError = undefined;
          } else {
            f.mode = "source";
            f.visualError = (res.diags ?? ["file cannot be edited visually"]).join("\n");
          }
        }),
      );
    } catch (e: any) {
      setFiles((f) => f.path === path, "visualError", e?.message ?? "parse failed");
    }
  };

  /// Visual → Source: dry-render the doc into the text buffer.
  const toSource = async (path: string) => {
    const file = files.find((f) => f.path === path);
    if (!file?.doc) {
      setFiles((f) => f.path === path, "mode", "source");
      return;
    }
    try {
      const res = await docRender(props.name, path, file.doc, file.content);
      setFiles(
        (f) => f.path === path,
        produce((f) => {
          if (res.ok && res.source != null) {
            f.dirty = f.dirty || !!f.docDirty || res.source !== f.content;
            f.content = res.source;
            f.mode = "source";
          } else {
            toast((res.diags ?? ["render failed"]).join("; "), { tone: "danger" });
          }
        }),
      );
    } catch (e: any) {
      toast(e?.message ?? "render failed", { tone: "danger" });
    }
  };

  /// Apply a form edit to the active file's doc.
  const mutateDoc = (fn: (doc: any) => void) => {
    const path = active();
    if (!path) return;
    setFiles(
      (f) => f.path === path,
      produce((f) => {
        if (f.doc) {
          fn(f.doc);
          f.docDirty = true;
        }
      }),
    );
  };

  const saveVisual = async () => {
    const file = current();
    if (!file?.doc) return;
    try {
      const res = await docSave(props.name, file.path, file.doc, file.baseHash);
      if (!res.ok) {
        toast((res.diags ?? ["save failed"]).join("; "), { tone: "danger" });
        return;
      }
      const reformatted = !file.dirty && !file.docDirty ? false : res.content !== file.content;
      setFiles(
        (f) => f.path === file.path,
        produce((f) => {
          f.content = res.content ?? f.content;
          f.baseHash = res.base_hash;
          f.dirty = false;
          f.docDirty = false;
        }),
      );
      toast(
        reformatted ? `saved ${file.path} (canonical formatting applied)` : `saved ${file.path}`,
        { tone: "success" },
      );
    } catch (e: any) {
      toast(e?.message ?? "save failed", { tone: "danger" });
    }
  };

  const edit = (value: string) => {
    const path = active();
    if (!path) return;
    setFiles(
      (f) => f.path === path,
      produce((f) => {
        f.dirty = f.content !== value;
        f.content = value;
      }),
    );
  };

  const save = async () => {
    const file = current();
    if (!file) return;
    try {
      await writeFile(props.name, file.path, file.content);
      setFiles((f) => f.path === file.path, "dirty", false);
      toast(`saved ${file.path}`, { tone: "success" });
    } catch (e: any) {
      toast(e?.message ?? "save failed", { tone: "danger" });
    }
  };

  const close = (path: string) => {
    setFiles(files.filter((f) => f.path !== path));
    if (active() === path) setActive(files[0]?.path ?? null);
  };

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

  return (
    <>
      <PageHead
        title={props.name}
        sub={inventory()?.description || "runbook"}
        actions={
          <div class="head-actions">
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
          <Show
            when={!v.ok}
            fallback={<Alert tone="success" title="Validation passed" />}
          >
            <Alert tone="danger" title={`Validation failed (${v.diags.length} error(s))`}>
              <pre class="diag-pre">{v.diags.map((d) => d.rendered).join("\n\n")}</pre>
            </Alert>
          </Show>
        )}
      </Show>

      <SplitPane
        class="runbook-split"
        initial={260}
        first={
          <div class="file-tree">
            <For each={tree() ?? []}>
              {(node) => <TreeEntry node={node} prefix="" open={open} selected={active()} />}
            </For>
          </div>
        }
        second={
          <div class="editor-pane">
            <Show
              when={files.length > 0}
              fallback={<Empty title="No file open">Pick a file from the tree.</Empty>}
            >
              <Tabs
                tabs={files.map((f) => ({
                  id: f.path,
                  label: `${f.path.split("/").pop()}${f.dirty || f.docDirty ? " •" : ""}`,
                }))}
                active={active() ?? undefined}
                onChange={setActive}
              />
              <Show when={current()} keyed>
                {(file) => (
                  <>
                    <Show when={file.docKind}>
                      <div class="editor-mode-row">
                        <ToggleGroup
                          options={[
                            { value: "visual", label: "Visual" },
                            { value: "source", label: "Source" },
                          ]}
                          value={file.mode}
                          onChange={(m) =>
                            m === "visual" ? void toVisual(file.path) : void toSource(file.path)
                          }
                        />
                        <Show when={file.visualError}>
                          <span class="sub error-text">{file.visualError}</span>
                        </Show>
                      </div>
                    </Show>
                    <Show
                      when={file.mode === "visual" && file.doc}
                      fallback={
                        <CodeEditor
                          value={file.content}
                          onChange={edit}
                          language={languageFor(file.path)}
                          height="52vh"
                        />
                      }
                    >
                      <div class="visual-pane">
                        <Show
                          when={file.docKind === "playbook"}
                          fallback={
                            <PackageEditor
                              doc={file.doc as PackageDoc}
                              runbook={props.name}
                              pkgDir={file.path.split("/").slice(0, -1).join("/")}
                              mutate={mutateDoc}
                            />
                          }
                        >
                          <PlaybookEditor
                            doc={file.doc as PlaybookDoc}
                            inventory={inventory()}
                            mutate={mutateDoc}
                          />
                        </Show>
                      </div>
                    </Show>
                    <div class="editor-actions">
                      <span class="editor-path">{file.path}</span>
                      <Button size="sm" variant="ghost" onClick={() => close(file.path)}>
                        Close
                      </Button>
                      <Show
                        when={file.mode === "visual"}
                        fallback={
                          <Button
                            size="sm"
                            variant="primary"
                            onClick={save}
                            disabled={!file.dirty}
                          >
                            Save
                          </Button>
                        }
                      >
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={saveVisual}
                          disabled={!file.docDirty && !file.dirty}
                        >
                          Save
                        </Button>
                      </Show>
                    </div>
                  </>
                )}
              </Show>
            </Show>
          </div>
        }
      />

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
