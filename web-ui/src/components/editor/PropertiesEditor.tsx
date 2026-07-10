// Schema-driven properties editor: one row per declared param of the
// selected resource (typed widget, required badge, default as
// placeholder, description as help), plus flagged rows for keys not in
// the schema (kept editable — validation reports them anyway).

import { For, Show } from "solid-js";
import { Badge, Button } from "@forge/ui";
import { Trash2 } from "lucide-solid";
import type { Kv, Val } from "../../api";
import ValueInput from "./ValueInput";

export interface ParamSchema {
  name: string;
  description: string;
  type: string;
  required: boolean;
  default: any | null;
}

export default function PropertiesEditor(props: {
  params: ParamSchema[];
  kvs: Kv[];
  onChange: (kvs: Kv[]) => void;
}) {
  const valueFor = (name: string): Val | undefined =>
    props.kvs.find((kv) => kv.key === name)?.value;

  const setValue = (name: string, v: Val | undefined) => {
    const rest = props.kvs.filter((kv) => kv.key !== name);
    if (v === undefined) {
      props.onChange(rest);
      return;
    }
    // Keep declaration order stable: replace in place when present.
    const i = props.kvs.findIndex((kv) => kv.key === name);
    if (i === -1) props.onChange([...props.kvs, { key: name, value: v }]);
    else props.onChange(props.kvs.map((kv, j) => (j === i ? { key: name, value: v } : kv)));
  };

  const undeclared = () =>
    props.kvs.filter((kv) => !props.params.some((p) => p.name === kv.key));

  return (
    <div class="props-editor">
      <For each={props.params}>
        {(p) => (
          <div class="prop-row">
            <div class="prop-label">
              <span class="mono">{p.name}</span>
              <Show when={p.required}>
                <Badge tone="warning">required</Badge>
              </Show>
              <div class="sub">{p.description}</div>
            </div>
            <ValueInput
              value={valueFor(p.name)}
              type={p.type}
              placeholder={p.default != null ? `default: ${JSON.stringify(p.default)}` : undefined}
              onChange={(v) => setValue(p.name, v)}
            />
          </div>
        )}
      </For>
      <For each={undeclared()}>
        {(kv) => (
          <div class="prop-row">
            <div class="prop-label">
              <span class="mono">{kv.key}</span>
              <Badge tone="danger">not in schema</Badge>
            </div>
            <ValueInput value={kv.value} onChange={(v) => setValue(kv.key, v)} />
            <Button
              size="sm"
              variant="ghost"
              icon={Trash2}
              onClick={() => setValue(kv.key, undefined)}
            />
          </div>
        )}
      </For>
    </div>
  );
}
