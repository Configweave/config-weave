// Auto-generated API documentation for a package. The server extracts
// the DocJson from package.wcl in-process (GET …/docs, never via the
// CLI); the usage snippets are generated client-side from the param
// schemas — required params filled with type placeholders or defaults,
// optional ones commented out.

import { For, Show, createResource, createSignal } from "solid-js";
import { Badge, Button, Card, Empty, toast } from "@forge/ui";
import { Check, Copy } from "lucide-solid";
import type { GathererDoc, PackageDoc, ParamDecl, ParamDoc, ResourceDoc } from "../api";
import { getPackageDocs } from "../api";
import { ParamsTable } from "./PackageView";

export default function PackageDocs(props: { name: string; runbook?: string }) {
  const [docs] = createResource(
    () => ({ name: props.name, runbook: props.runbook }),
    ({ name, runbook }) => getPackageDocs(name, runbook),
  );
  return (
    <Show
      when={!docs.error}
      fallback={
        <Card>
          <Empty title="Cannot generate the docs">
            <span class="sub">{docs.error?.message ?? "package.wcl extraction failed"}</span>
          </Empty>
        </Card>
      }
    >
      <Show when={docs()?.doc} keyed>
        {(doc: PackageDoc) => (
          <>
            <Card title="Resources">
              <Show
                when={doc.resources.length > 0}
                fallback={<Empty title="No resources declared" />}
              >
                <For each={doc.resources}>
                  {(r) => <ResourceSection pkg={props.name} resource={r} />}
                </For>
              </Show>
            </Card>
            <Show when={doc.gatherers.length > 0}>
              <Card title="Gatherers">
                <For each={doc.gatherers}>
                  {(g) => <GathererSection pkg={props.name} gatherer={g} />}
                </For>
              </Card>
            </Show>
          </>
        )}
      </Show>
    </Show>
  );
}

function ResourceSection(props: { pkg: string; resource: ResourceDoc }) {
  return (
    <section class="api-doc">
      <div class="api-doc-head">
        <span class="mono api-doc-name">
          {props.pkg}.{props.resource.name}
        </span>
        <Badge tone="neutral">{props.resource.concurrency ?? "parallel"}</Badge>
        <span class="sub mono">{props.resource.script}</span>
      </div>
      <p class="sub api-doc-desc">{props.resource.description}</p>
      <ParamsTable params={props.resource.params.map(toDecl)} />
      <Snippet code={resourceSnippet(props.pkg, props.resource)} />
    </section>
  );
}

function GathererSection(props: { pkg: string; gatherer: GathererDoc }) {
  return (
    <section class="api-doc">
      <div class="api-doc-head">
        <span class="mono api-doc-name">
          {props.pkg}.{props.gatherer.name}
        </span>
        <span class="sub mono">{props.gatherer.script}</span>
      </div>
      <p class="sub api-doc-desc">{props.gatherer.description}</p>
      <ParamsTable params={props.gatherer.params.map(toDecl)} />
      <Snippet code={gathererSnippet(props.pkg, props.gatherer)} />
    </section>
  );
}

/// ParamDoc (DocJson: optional fields, Val defaults) → the inventory
/// ParamDecl shape ParamsTable renders. Schema defaults are part of the
/// documented contract, so absent `required` shows as optional.
const toDecl = (p: ParamDoc): ParamDecl => ({
  name: p.name,
  description: p.description,
  type: p.type,
  required: p.required ?? false,
  default: p.default == null ? null : "lit" in p.default ? p.default.lit : p.default.expr,
});

// --- WCL usage snippets -----------------------------------------------------

const PLACEHOLDER: Record<string, string> = {
  string: '""',
  int: "0",
  float: "0.0",
  bool: "false",
  list: "[]",
  map: "{}",
};

/** A WCL literal for a Lit default; float-typed integers keep a decimal. */
function wclLit(v: any, ty: string): string {
  if (typeof v === "number" && ty === "float" && Number.isInteger(v)) return `${v}.0`;
  return JSON.stringify(v);
}

const paramValue = (p: ParamDoc): string =>
  p.default == null
    ? (PLACEHOLDER[p.type] ?? '""')
    : "lit" in p.default
      ? wclLit(p.default.lit, p.type)
      : p.default.expr;

function paramLines(params: ParamDoc[], block: string): string[] {
  const required = params.filter((p) => p.required);
  const optional = params.filter((p) => !p.required);
  return [
    `  ${block} {`,
    ...required.map((p) => `    ${p.name} = ${paramValue(p)}`),
    ...optional.map((p) => `    # ${p.name} = ${paramValue(p)}`),
    `  }`,
  ];
}

export function resourceSnippet(pkg: string, r: ResourceDoc): string {
  return [
    `step ${JSON.stringify(r.name)} {`,
    `  description = "..."`,
    `  resource = ${JSON.stringify(`${pkg}.${r.name}`)}`,
    ...(r.params.length > 0 ? paramLines(r.params, "properties") : []),
    `}`,
  ].join("\n");
}

export function gathererSnippet(pkg: string, g: GathererDoc): string {
  return [
    `gather ${JSON.stringify(g.name)} {`,
    `  description = ${JSON.stringify(g.description)}`,
    `  from = ${JSON.stringify(`${pkg}.${g.name}`)}`,
    ...(g.params.length > 0 ? paramLines(g.params, "params") : []),
    `}`,
  ].join("\n");
}

function Snippet(props: { code: string }) {
  const [copied, setCopied] = createSignal(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast("cannot access the clipboard", { tone: "danger" });
    }
  };
  return (
    <div class="wcl-snippet">
      <Button
        size="sm"
        variant="ghost"
        icon={copied() ? Check : Copy}
        title="Copy the snippet"
        onClick={copy}
      />
      <pre class="mono">{props.code}</pre>
    </div>
  );
}
