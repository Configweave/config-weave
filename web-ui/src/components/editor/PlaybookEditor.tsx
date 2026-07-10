// The graphical playbook editor: metadata, gathers, vars, and plays
// with nested step/container forms. Edits flow through `mutate`, which
// the owner applies to its store draft (produce), so nested updates
// stay cheap and the doc object is the single source of truth.

import { For, Show, createSignal } from "solid-js";
import { Badge, Button, Checkbox, Input, Select, Textarea, Toggle } from "@forge/ui";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Plus, Trash2 } from "lucide-solid";
import type {
  ContainerDoc,
  GatherDoc,
  Inventory,
  PlayDoc,
  PlayItemDoc,
  PlaybookDoc,
  StepDoc,
} from "../../api";
import type { ParamSchema } from "./PropertiesEditor";
import KeyValueEditor from "./KeyValueEditor";
import PropertiesEditor from "./PropertiesEditor";

type Mutate = (fn: (d: PlaybookDoc) => void) => void;

export function move<T>(arr: T[], i: number, delta: number) {
  const j = i + delta;
  if (j < 0 || j >= arr.length) return;
  const [x] = arr.splice(i, 1);
  arr.splice(j, 0, x);
}

/// "pkg.resource" → its declared params, from the inventory.
function resourceParams(inv: Inventory | undefined, ref: string): ParamSchema[] {
  const [pkg, res] = ref.split(".", 2);
  const p = inv?.packages.find((x) => x.name === pkg);
  const r = p?.resources?.find((x) => x.name === res);
  return (r?.params ?? []).map((x) => ({
    name: x.name,
    description: x.description,
    type: x.type,
    required: x.required,
    default: x.default,
  }));
}

function resourceOptions(inv: Inventory | undefined) {
  return (inv?.packages ?? []).flatMap((p) =>
    (p.resources ?? []).map((r) => ({
      value: `${p.name}.${r.name}`,
      label: `${p.name}.${r.name}`,
    })),
  );
}

function gathererOptions(inv: Inventory | undefined) {
  return (inv?.packages ?? []).flatMap((p) =>
    (p.gatherers ?? []).map((g) => ({
      value: `${p.name}.${g.name}`,
      label: `${p.name}.${g.name}`,
    })),
  );
}

/// Every step name in a play (containers included) except `self`.
function stepNames(items: PlayItemDoc[], self?: string): string[] {
  const out: string[] = [];
  const walk = (list: PlayItemDoc[]) => {
    for (const it of list) {
      if ("step" in it) {
        if (it.step.name && it.step.name !== self) out.push(it.step.name);
      } else {
        walk(it.container.items);
      }
    }
  };
  walk(items);
  return out;
}

export default function PlaybookEditor(props: {
  doc: PlaybookDoc;
  inventory: Inventory | undefined;
  mutate: Mutate;
}) {
  return (
    <div class="visual-editor">
      <section class="ve-section">
        <div class="ve-heading">Playbook</div>
        <div class="form-grid">
          <Input
            label="Name"
            value={props.doc.name}
            onInput={(e) => props.mutate((d) => (d.name = e.currentTarget.value))}
          />
          <Input
            label="Version"
            value={props.doc.version ?? ""}
            onInput={(e) =>
              props.mutate((d) => (d.version = e.currentTarget.value || undefined))
            }
          />
        </div>
        <Textarea
          label="Description"
          rows={2}
          value={props.doc.description}
          onInput={(e) => props.mutate((d) => (d.description = e.currentTarget.value))}
        />
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Gathers
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.gathers.push({ name: "", from: "", params: [] } as GatherDoc),
              )
            }
          >
            Add gather
          </Button>
        </div>
        <For each={props.doc.gathers}>
          {(g, i) => (
            <div class="ve-item">
              <div class="form-grid">
                <Input
                  label="Variable"
                  value={g.name}
                  onInput={(e) =>
                    props.mutate((d) => (d.gathers[i()].name = e.currentTarget.value))
                  }
                />
                <Select
                  label="Gatherer"
                  placeholder="pkg.gatherer"
                  options={gathererOptions(props.inventory)}
                  value={g.from}
                  onChange={(v) => props.mutate((d) => (d.gathers[i()].from = v))}
                />
                <div class="ve-item-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Trash2}
                    onClick={() => props.mutate((d) => d.gathers.splice(i(), 1))}
                  />
                </div>
              </div>
              <KeyValueEditor
                kvs={g.params}
                keyPlaceholder="param"
                onChange={(kvs) => props.mutate((d) => (d.gathers[i()].params = kvs))}
              />
            </div>
          )}
        </For>
      </section>

      <section class="ve-section">
        <div class="ve-heading">Vars</div>
        <KeyValueEditor
          kvs={props.doc.vars}
          exprOnly
          keyPlaceholder="variable"
          onChange={(kvs) => props.mutate((d) => (d.vars = kvs))}
        />
      </section>

      <section class="ve-section">
        <div class="ve-heading">
          Plays
          <Button
            size="sm"
            variant="ghost"
            icon={Plus}
            onClick={() =>
              props.mutate((d) =>
                d.plays.push({ name: "", description: "", items: [] } as PlayDoc),
              )
            }
          >
            Add play
          </Button>
        </div>
        <For each={props.doc.plays}>
          {(p, i) => (
            <PlayForm
              play={p}
              inventory={props.inventory}
              onRemove={() => props.mutate((d) => d.plays.splice(i(), 1))}
              mutatePlay={(fn) => props.mutate((d) => fn(d.plays[i()]))}
            />
          )}
        </For>
      </section>
    </div>
  );
}

function PlayForm(props: {
  play: PlayDoc;
  inventory: Inventory | undefined;
  onRemove: () => void;
  mutatePlay: (fn: (p: PlayDoc) => void) => void;
}) {
  const [open, setOpen] = createSignal(true);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone="info">play</Badge>
        <span class="mono">{props.play.name || "(unnamed)"}</span>
        <span class="sub">{props.play.description}</span>
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
              value={props.play.name}
              onInput={(e) => props.mutatePlay((p) => (p.name = e.currentTarget.value))}
            />
            <Input
              label="Description"
              value={props.play.description}
              onInput={(e) => props.mutatePlay((p) => (p.description = e.currentTarget.value))}
            />
            <label class="ffield">
              <span class="ffield-label">Parallel (default on)</span>
              <Toggle
                checked={props.play.parallel !== false}
                onChange={(checked) =>
                  props.mutatePlay((p) => (p.parallel = checked ? undefined : false))
                }
              />
            </label>
          </div>
          <PlayItems
            items={props.play.items}
            allSteps={props.play.items}
            inventory={props.inventory}
            mutateItems={(fn) => props.mutatePlay((p) => fn(p.items))}
          />
        </div>
      </Show>
    </div>
  );
}

function PlayItems(props: {
  items: PlayItemDoc[];
  /// The play-level items, for the requires picker (names are play-wide).
  allSteps: PlayItemDoc[];
  inventory: Inventory | undefined;
  mutateItems: (fn: (items: PlayItemDoc[]) => void) => void;
}) {
  return (
    <div class="ve-items">
      <For each={props.items}>
        {(item, i) => (
          <div class="ve-item-row">
            <div class="ve-reorder">
              <Button
                size="sm"
                variant="ghost"
                icon={ArrowUp}
                onClick={() => props.mutateItems((items) => move(items, i(), -1))}
              />
              <Button
                size="sm"
                variant="ghost"
                icon={ArrowDown}
                onClick={() => props.mutateItems((items) => move(items, i(), 1))}
              />
            </div>
            <div class="ve-item-main">
              <Show
                when={"step" in item ? item : undefined}
                keyed
                fallback={
                  <ContainerForm
                    container={(item as { container: ContainerDoc }).container}
                    allSteps={props.allSteps}
                    inventory={props.inventory}
                    onRemove={() => props.mutateItems((items) => items.splice(i(), 1))}
                    mutateContainer={(fn) =>
                      props.mutateItems((items) =>
                        fn((items[i()] as { container: ContainerDoc }).container),
                      )
                    }
                  />
                }
              >
                {(it) => (
                  <StepForm
                    step={it.step}
                    allSteps={props.allSteps}
                    inventory={props.inventory}
                    onRemove={() => props.mutateItems((items) => items.splice(i(), 1))}
                    mutateStep={(fn) =>
                      props.mutateItems((items) => fn((items[i()] as { step: StepDoc }).step))
                    }
                  />
                )}
              </Show>
            </div>
          </div>
        )}
      </For>
      <div class="ve-add-row">
        <Button
          size="sm"
          variant="ghost"
          icon={Plus}
          onClick={() =>
            props.mutateItems((items) =>
              items.push({
                step: { name: "", description: "", resource: "", requires: [], properties: [] },
              }),
            )
          }
        >
          Add step
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={Plus}
          onClick={() =>
            props.mutateItems((items) =>
              items.push({ container: { name: "", description: "", items: [] } }),
            )
          }
        >
          Add container
        </Button>
      </div>
    </div>
  );
}

function StepForm(props: {
  step: StepDoc;
  allSteps: PlayItemDoc[];
  inventory: Inventory | undefined;
  onRemove: () => void;
  mutateStep: (fn: (s: StepDoc) => void) => void;
}) {
  const [open, setOpen] = createSignal(false);
  const siblings = () => stepNames(props.allSteps, props.step.name);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone="neutral">step</Badge>
        <span class="mono">{props.step.name || "(unnamed)"}</span>
        <span class="sub mono">{props.step.resource}</span>
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
              label="Resource"
              placeholder="pkg.resource"
              options={resourceOptions(props.inventory)}
              value={props.step.resource}
              onChange={(v) => props.mutateStep((s) => (s.resource = v))}
            />
            <Select
              label="Concurrency (default: resource's)"
              options={[
                { value: "", label: "inherit" },
                { value: "parallel", label: "parallel" },
                { value: "exclusive", label: "exclusive" },
                { value: "global", label: "global" },
              ]}
              value={props.step.concurrency ?? ""}
              onChange={(v) => props.mutateStep((s) => (s.concurrency = v || undefined))}
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
          <Show when={siblings().length > 0}>
            <div class="ffield">
              <span class="ffield-label">Requires</span>
              <div class="requires-picker">
                <For each={siblings()}>
                  {(name) => (
                    <Checkbox
                      checked={props.step.requires.includes(name)}
                      onChange={(checked) =>
                        props.mutateStep((s) => {
                          s.requires = checked
                            ? [...s.requires, name]
                            : s.requires.filter((r) => r !== name);
                        })
                      }
                    >
                      <span class="mono">{name}</span>
                    </Checkbox>
                  )}
                </For>
              </div>
            </div>
          </Show>
          <div class="ffield">
            <span class="ffield-label">Properties</span>
            <PropertiesEditor
              params={resourceParams(props.inventory, props.step.resource)}
              kvs={props.step.properties}
              onChange={(kvs) => props.mutateStep((s) => (s.properties = kvs))}
            />
          </div>
        </div>
      </Show>
    </div>
  );
}

function ContainerForm(props: {
  container: ContainerDoc;
  allSteps: PlayItemDoc[];
  inventory: Inventory | undefined;
  onRemove: () => void;
  mutateContainer: (fn: (c: ContainerDoc) => void) => void;
}) {
  const [open, setOpen] = createSignal(true);
  return (
    <div class="ve-block">
      <div class="ve-block-head" onClick={() => setOpen(!open())}>
        {open() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Badge tone="info">container</Badge>
        <span class="mono">{props.container.name || "(unnamed)"}</span>
        <span class="sub">{props.container.description}</span>
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
              value={props.container.name}
              onInput={(e) => props.mutateContainer((c) => (c.name = e.currentTarget.value))}
            />
            <Input
              label="Description"
              value={props.container.description}
              onInput={(e) =>
                props.mutateContainer((c) => (c.description = e.currentTarget.value))
              }
            />
          </div>
          <Input
            class="mono-input"
            label="Condition (applies to all children)"
            value={props.container.condition ?? ""}
            onInput={(e) =>
              props.mutateContainer((c) => (c.condition = e.currentTarget.value || undefined))
            }
          />
          <PlayItems
            items={props.container.items}
            allSteps={props.allSteps}
            inventory={props.inventory}
            mutateItems={(fn) => props.mutateContainer((c) => fn(c.items))}
          />
        </div>
      </Show>
    </div>
  );
}
