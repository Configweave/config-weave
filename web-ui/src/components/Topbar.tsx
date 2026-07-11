import type { JSX } from "solid-js";
import { Crumbs } from "@forge/ui";
import type { View } from "../store";
import { setView, view } from "../store";

export default function Topbar() {
  const link = (label: string, target: View) => <a class="crumb-link" onClick={() => setView(target)}>{label}</a>;
  const crumbs = (): JSX.Element[] => {
    const v = view();
    if (v.kind === "services") return [<span>services</span>];
    if (v.kind === "service") return [link("services", { kind: "services" }), <span>{v.name}</span>];
    if (v.kind === "sysrun") return [link("services", { kind: "services" }), link(v.service, { kind: "service", name: v.service }), <span>{v.system}</span>, <span>{v.playbook}:{v.play}</span>];
    if (v.kind === "runbooks") return [<span>library</span>, <span>playbooks</span>];
    if (v.kind === "runbook") return [<span>library</span>, link("playbooks", { kind: "runbooks" }), <span>{v.name}</span>];
    if (v.kind === "packages") return [<span>library</span>, <span>packages</span>];
    if (v.kind === "package") return [<span>library</span>, link("packages", { kind: "packages" }), ...(v.runbook ? [link(v.runbook, { kind: "runbook", name: v.runbook })] : []), <span>{v.name}</span>];
    if (v.kind === "activity") return [<span>library</span>, <span>activity</span>];
    if (v.kind === "run") return [<span>library</span>, link("activity", { kind: "activity" }), <span>run {v.id.slice(0, 8)}</span>];
    return [];
  };
  return <div class="topbar-inner"><strong>config-weave</strong><Crumbs items={crumbs()} /></div>;
}
