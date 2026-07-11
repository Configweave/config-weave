import { For, Show, createResource, onCleanup } from "solid-js";
import { Badge, Card, Empty, PageHead, Table } from "@forge/ui";
import { listRuns } from "../api";
import { setView } from "../store";

export default function ActivityView() {
  const [runs, { refetch }] = createResource(listRuns);
  const timer = setInterval(() => { if ((runs() ?? []).some((r) => r.status === "running")) void refetch(); }, 2000);
  onCleanup(() => clearInterval(timer));
  return <>
    <PageHead title="Library activity" sub="Test runs launched from playbooks and packages" />
    <Card>
      <Show when={(runs() ?? []).length} fallback={<Empty title="No library activity yet" />}>
        <Table><thead><tr><th>Source</th><th>Filter</th><th>Status</th><th>Run</th></tr></thead><tbody>
          <For each={runs() ?? []}>{(run) => <tr class="clickable-row" onClick={() => setView({ kind: "run", id: run.id, runbook: run.runbook })}><td>{run.runbook.startsWith("pkgs:") ? "Package" : "Playbook"} <strong>{run.runbook.replace("pkgs:", "")}</strong></td><td class="mono">{run.filter || "all tests"}</td><td><Badge tone={run.status === "passed" ? "success" : run.status === "running" ? "warning" : run.status === "cancelled" ? "neutral" : "danger"}>{run.status}</Badge></td><td class="mono">{run.id.slice(0, 8)}</td></tr>}</For>
        </tbody></Table>
      </Show>
    </Card>
  </>;
}
