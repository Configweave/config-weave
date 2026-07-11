// The shared editing workspace: file tree | tabbed editor (source
// CodeMirror, plus the visual form editors for playbook.wcl /
// package.wcl backed by the DocJson endpoints). Parameterized by a
// WorkspaceScope so the same component serves runbook roots, repo
// package roots, and runbook-installed package copies.

import { For, Show, createResource, createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { Badge, Empty, SplitPane, Tabs, ToggleGroup, toast } from "@forge/ui";
import { Button } from "@forge/ui";
import { CodeEditor } from "@forge/code";
import { ChevronDown, ChevronRight, FileText } from "lucide-solid";
import type { Inventory, PackageDoc, PlaybookDoc, TreeNode, WorkspaceScope } from "../api";
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

export default function FileWorkspace(props: {
  scope: WorkspaceScope;
  /// Playbook-editor pickers (runbook workspaces only).
  inventory?: Inventory;
  /// Top-level tree entries to suppress (RunbookView passes ["pkgs"]).
  hideTopLevel?: string[];
  /// Bump to refetch the tree (e.g. after a package remove).
  reloadKey?: unknown;
  /// Viewing a remote repository's package: saving is hidden and the
  /// editor rejects input (the server 403s writes as the backstop).
  readOnly?: boolean;
  height?: string;
}) {
  const [tree] = createResource(
    () => [props.scope, props.reloadKey] as const,
    ([scope]) => scope.tree(),
  );
  const [files, setFiles] = createStore<OpenFile[]>([]);
  const [active, setActive] = createSignal<string | null>(null);

  const visibleTree = () =>
    (tree() ?? []).filter((n) => !props.hideTopLevel?.includes(n.name));

  const current = () => files.find((f) => f.path === active()) ?? null;

  const open = async (path: string) => {
    if (!files.some((f) => f.path === path)) {
      try {
        const { content } = await props.scope.read(path);
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
        toast(e?.message ?? `cannot open ${path}`, { tone: "danger" });
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
      const res = await props.scope.docParse(path, file.dirty ? file.content : undefined);
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
      const res = await props.scope.docRender(path, file.doc, file.content);
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
      const res = await props.scope.docSave(file.path, file.doc, file.baseHash);
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
      await props.scope.write(file.path, file.content);
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

  return (
    <SplitPane
      class="runbook-split"
      initial={260}
      first={
        <div class="file-tree">
          <For each={visibleTree()}>
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
                        onChange={props.readOnly ? undefined : edit}
                        readOnly={props.readOnly}
                        language={languageFor(file.path)}
                        height={props.height ?? "52vh"}
                      />
                    }
                  >
                    <div class="visual-pane">
                      <Show
                        when={file.docKind === "playbook"}
                        fallback={
                          <PackageEditor
                            doc={file.doc as PackageDoc}
                            scope={props.scope}
                            pkgDir={file.path.split("/").slice(0, -1).join("/")}
                            mutate={mutateDoc}
                          />
                        }
                      >
                        <PlaybookEditor
                          doc={file.doc as PlaybookDoc}
                          inventory={props.inventory}
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
                      when={!props.readOnly}
                      fallback={<Badge tone="neutral">read-only</Badge>}
                    >
                      <Show
                        when={file.mode === "visual"}
                        fallback={
                          <Button size="sm" variant="primary" onClick={save} disabled={!file.dirty}>
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
                    </Show>
                  </div>
                </>
              )}
            </Show>
          </Show>
        </div>
      }
    />
  );
}
