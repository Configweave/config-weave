// Free-form key = value rows for schemaless map blocks (vars, gather
// params, test expect). Values ride ValueInput (lit/expr).

import { For } from "solid-js";
import { Button, Input } from "@forge/ui";
import { Plus, Trash2 } from "lucide-solid";
import type { Kv } from "../../api";
import ValueInput from "./ValueInput";

export default function KeyValueEditor(props: {
  kvs: Kv[];
  onChange: (kvs: Kv[]) => void;
  keyPlaceholder?: string;
  exprOnly?: boolean;
}) {
  const update = (i: number, kv: Kv) => {
    const next = props.kvs.slice();
    next[i] = kv;
    props.onChange(next);
  };
  return (
    <div class="kv-editor">
      <For each={props.kvs}>
        {(kv, i) => (
          <div class="kv-row">
            <Input
              class="mono-input kv-key"
              placeholder={props.keyPlaceholder ?? "key"}
              value={kv.key}
              onInput={(e) => update(i(), { ...kv, key: e.currentTarget.value })}
            />
            <span class="kv-eq">=</span>
            <ValueInput
              value={kv.value}
              exprOnly={props.exprOnly}
              onChange={(v) => update(i(), { ...kv, value: v ?? { lit: "" } })}
            />
            <Button
              size="sm"
              variant="ghost"
              icon={Trash2}
              onClick={() => props.onChange(props.kvs.filter((_, j) => j !== i()))}
            />
          </div>
        )}
      </For>
      <Button
        size="sm"
        variant="ghost"
        icon={Plus}
        onClick={() =>
          props.onChange([
            ...props.kvs,
            { key: "", value: props.exprOnly ? { expr: "" } : { lit: "" } },
          ])
        }
      >
        Add entry
      </Button>
    </div>
  );
}
