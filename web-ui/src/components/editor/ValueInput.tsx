// The Val (lit-or-expr) widget every form leaf uses: a typed input for
// literals, a monospace expression input in fx mode, and a toggle
// between the two. Switching lit → expr keeps the value as quoted
// source; expr → lit only when the expression is a plain literal.

import { Show } from "solid-js";
import { Checkbox, Input } from "@forge/ui";
import type { Val } from "../../api";

export function isExpr(v: Val | undefined): v is { expr: string } {
  return !!v && "expr" in v;
}

export function litOf(v: Val | undefined): any {
  return v && "lit" in v ? v.lit : undefined;
}

export default function ValueInput(props: {
  value: Val | undefined;
  onChange: (v: Val | undefined) => void;
  /// Coarse type driving the literal widget: string|int|float|bool|list|map.
  type?: string;
  placeholder?: string;
  /// Lock to expression mode (vars, conditions).
  exprOnly?: boolean;
}) {
  const mode = () => (props.exprOnly || isExpr(props.value) ? "expr" : "lit");

  const toExpr = () => {
    const lit = litOf(props.value);
    props.onChange({
      expr: typeof lit === "string" ? JSON.stringify(lit) : lit != null ? String(lit) : "",
    });
  };
  const toLit = () => {
    props.onChange({ lit: props.type === "bool" ? false : "" });
  };

  return (
    <span class="value-input">
      <Show
        when={mode() === "expr"}
        fallback={
          <Show
            when={props.type === "bool"}
            fallback={
              <Input
                class="mono-input"
                type={props.type === "int" || props.type === "float" ? "number" : "text"}
                step={props.type === "float" ? "any" : undefined}
                placeholder={props.placeholder}
                value={litOf(props.value) ?? ""}
                onInput={(e) => {
                  const raw = e.currentTarget.value;
                  if (raw === "" && props.placeholder != null) {
                    props.onChange(undefined);
                  } else if (props.type === "int") {
                    props.onChange({ lit: raw === "" ? 0 : parseInt(raw, 10) || 0 });
                  } else if (props.type === "float") {
                    props.onChange({ lit: raw === "" ? 0 : parseFloat(raw) || 0 });
                  } else {
                    props.onChange({ lit: raw });
                  }
                }}
              />
            }
          >
            <Checkbox
              checked={!!litOf(props.value)}
              onChange={(checked) => props.onChange({ lit: checked })}
            />
          </Show>
        }
      >
        <Input
          class="mono-input expr-input"
          placeholder={props.placeholder ?? "WCL expression"}
          value={isExpr(props.value) ? props.value.expr : ""}
          onInput={(e) => props.onChange({ expr: e.currentTarget.value })}
        />
      </Show>
      <Show when={!props.exprOnly}>
        <button
          type="button"
          class="fx-toggle"
          classList={{ "is-on": mode() === "expr" }}
          title={mode() === "expr" ? "switch to a plain value" : "switch to a WCL expression"}
          onClick={() => (mode() === "expr" ? toLit() : toExpr())}
        >
          fx
        </button>
      </Show>
    </span>
  );
}
