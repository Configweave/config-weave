// Sidebar: runbooks, systems, plus the session's runs (test + system),
// kept fresh by polling the cheap summary endpoints while a run is live.

import { For, Show, createResource, onCleanup } from "solid-js";
import { Badge, NavLink, NavSection, StatusDot } from "@forge/ui";
import type { StatusTone } from "@forge/ui";
import { BookOpen, FlaskConical, PackageOpen, Server } from "lucide-solid";
import { listPackages, listRunbooks, listRuns, listSystemRuns } from "../api";
import { setView, view } from "../store";

const RUN_TONE: Record<string, StatusTone> = {
  running: "warning",
  passed: "success",
  succeeded: "success",
  reboot_required: "warning",
  failed: "danger",
  error: "danger",
  cancelled: "neutral",
};

export default function Sidebar() {
  const [runbooks] = createResource(listRunbooks);
  const [runs, { refetch }] = createResource(listRuns);
  const [sysRuns, { refetch: refetchSys }] = createResource(listSystemRuns);
  // 404 = no --packages-dir configured → hide the section entirely.
  const [repo] = createResource(() => listPackages().catch(() => null));
  const timer = setInterval(() => {
    if ((runs() ?? []).some((r) => r.status === "running") || view().kind === "run") refetch();
    if ((sysRuns() ?? []).some((r) => r.status === "running") || view().kind === "sysrun")
      refetchSys();
  }, 2000);
  onCleanup(() => clearInterval(timer));

  return (
    <>
      <NavSection>
        <NavLink
          icon={BookOpen}
          active={view().kind === "runbooks"}
          onClick={() => setView({ kind: "runbooks" })}
        >
          Runbooks
        </NavLink>
        <For each={runbooks() ?? []}>
          {(rb) => (
            <NavLink
              active={view().kind === "runbook" && (view() as any).name === rb.name}
              onClick={() => setView({ kind: "runbook", name: rb.name })}
              style={{ "padding-left": "24px" }}
            >
              {rb.name}
            </NavLink>
          )}
        </For>
      </NavSection>
      <NavSection>
        <NavLink
          icon={Server}
          active={view().kind === "systems"}
          onClick={() => setView({ kind: "systems" })}
        >
          Systems
        </NavLink>
        <For each={sysRuns() ?? []}>
          {(r) => (
            <NavLink
              active={view().kind === "sysrun" && (view() as any).id === r.id}
              onClick={() =>
                setView({ kind: "sysrun", id: r.id, system: r.system, action: r.action })
              }
              style={{ "padding-left": "24px" }}
            >
              <StatusDot tone={RUN_TONE[r.status] ?? "neutral"} />
              <span class="run-label">
                {r.system} · {r.action}
              </span>
            </NavLink>
          )}
        </For>
      </NavSection>
      <Show when={repo()}>
        <NavSection>
          <NavLink
            icon={PackageOpen}
            active={view().kind === "packages" || view().kind === "package"}
            onClick={() => setView({ kind: "packages" })}
          >
            Packages
          </NavLink>
        </NavSection>
      </Show>
      <NavSection>
        <div class="sidebar-heading">
          <FlaskConical size={14} /> Runs
        </div>
        <For each={runs() ?? []}>
          {(r) => (
            <NavLink
              active={view().kind === "run" && (view() as any).id === r.id}
              onClick={() => setView({ kind: "run", id: r.id, runbook: r.runbook })}
            >
              <StatusDot tone={RUN_TONE[r.status] ?? "neutral"} />
              <span class="run-label">
                {r.runbook}
                {r.filter ? `:${r.filter}` : ""}
              </span>
              <Show when={(r.kept_alive ?? 0) > 0 && r.status !== "running"}>
                <Badge tone="warning">kept</Badge>
              </Show>
            </NavLink>
          )}
        </For>
      </NavSection>
    </>
  );
}
