import { Show } from "solid-js";
import type { JSX } from "solid-js";
import { Crumbs } from "@forge/ui";
import { setView, view } from "../store";

export default function Topbar() {
  const crumbs = (): JSX.Element[] => {
    const v = view();
    const items: JSX.Element[] = [
      <a class="crumb-link" onClick={() => setView({ kind: "runbooks" })}>
        runbooks
      </a>,
    ];
    if (v.kind === "systems") {
      items.length = 0;
      items.push(<span>systems</span>);
    }
    if (v.kind === "sysrun") {
      items.length = 0;
      items.push(
        <a class="crumb-link" onClick={() => setView({ kind: "systems" })}>
          systems
        </a>,
      );
      items.push(
        <span>
          {v.system} · {v.action}
        </span>,
      );
    }
    if (v.kind === "packages") {
      items.length = 0;
      items.push(<span>packages</span>);
    }
    if (v.kind === "package") {
      items.length = 0;
      items.push(
        <a class="crumb-link" onClick={() => setView({ kind: "packages" })}>
          packages
        </a>,
      );
      items.push(<span>{v.name}</span>);
    }
    if (v.kind === "runbook") items.push(<span>{v.name}</span>);
    if (v.kind === "run") {
      items.push(
        <a class="crumb-link" onClick={() => setView({ kind: "runbook", name: v.runbook })}>
          {v.runbook}
        </a>,
      );
      items.push(<span>run {v.id.slice(0, 8)}</span>);
    }
    return items;
  };
  return (
    <div class="topbar-inner">
      <strong>config-weave</strong>
      <Show when={view().kind !== "runbooks"}>
        <Crumbs items={crumbs()} />
      </Show>
    </div>
  );
}
