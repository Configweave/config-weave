// One pipeline run: polls the run snapshot (the daemon's event bus is not
// weave-server's, so there is no SSE here) and renders per-step status and
// a streaming output log until the run settles.

import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { Badge, Button, Card, Empty, LogLine, Logs, PageHead, StatusDot, Table, toast } from "@forge/ui";
import type { StatusTone } from "@forge/ui";
import { Square } from "lucide-solid";
import { cancelPipelineRun, getPipelineRun } from "../api";

export function runStatusTone(status: string): StatusTone {
  switch (status) {
    case "succeeded":
      return "success";
    case "running":
      return "warning";
    case "cancelled":
      return "neutral";
    default:
      return "danger";
  }
}

const STEP_TONE: Record<string, StatusTone> = {
  succeeded: "success",
  failed: "danger",
  error: "danger",
  reboot_required: "warning",
  skipped: "neutral",
  cancelled: "neutral",
};

export default function PipelineRunView(props: { id: string; pipeline: string }) {
  const [status, setStatus] = createSignal("running");
  const [phase, setPhase] = createSignal("");
  const [steps, setSteps] = createStore<any[]>([]);
  const [logs, setLogs] = createStore<{ text: string }[]>([]);

  const seed = (snap: Awaited<ReturnType<typeof getPipelineRun>>) => {
    setStatus(snap.status);
    setPhase(snap.phase);
    setSteps(reconcile(snap.steps ?? []));
    // Flatten output/play_event lines from the event buffer into the log.
    const lines: { text: string }[] = [];
    for (const e of snap.events ?? []) {
      if (e.event === "output" && typeof e.line === "string") lines.push({ text: e.line });
      else if (e.event === "play_event" && e.data?.event === "output" && e.data.line)
        lines.push({ text: e.data.line });
    }
    setLogs(reconcile(lines));
  };

  onMount(() => {
    let sealed = false;
    const poll = async () => {
      try {
        const snap = await getPipelineRun(props.id);
        seed(snap);
        if (snap.status !== "running") sealed = true;
      } catch {
        /* transient / run gone after a restart */
      }
    };
    void poll();
    const timer = setInterval(() => {
      if (!sealed) void poll();
    }, 2500);
    onCleanup(() => clearInterval(timer));
  });

  const cancel = async () => {
    try {
      await cancelPipelineRun(props.id);
    } catch (e: any) {
      toast(e?.message ?? "cancel failed", { tone: "danger" });
    }
  };

  return (
    <>
      <PageHead
        title={`${props.pipeline} — run`}
        sub={
          <span class="run-status">
            <StatusDot tone={runStatusTone(status())} /> {status().replaceAll("_", " ")}
            <Show when={status() === "running" && phase()}>
              <span class="sub"> · {phase()}</span>
            </Show>
          </span>
        }
        actions={
          <Show when={status() === "running"}>
            <Button size="sm" variant="danger" icon={Square} onClick={cancel}>
              Cancel
            </Button>
          </Show>
        }
      />

      <Card title="Steps">
        <Show when={steps.length > 0} fallback={<Empty title="Waiting for the first step…" />}>
          <Table>
            <thead>
              <tr>
                <th>#</th>
                <th>Step</th>
                <th>Status</th>
                <th>Detail</th>
              </tr>
            </thead>
            <tbody>
              <For each={steps}>
                {(s) => (
                  <tr>
                    <td class="sub">{s.index}</td>
                    <td>{s.name}</td>
                    <td>
                      <Badge tone={STEP_TONE[s.status] ?? "neutral"}>
                        {String(s.status).replaceAll("_", " ")}
                      </Badge>
                    </td>
                    <td class="sub">
                      {s.message ?? (s.exit_code != null ? `exit ${s.exit_code}` : "")}
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>

      <Show when={logs.length > 0}>
        <Card title="Output">
          <Logs class="run-logs">
            <For each={logs}>
              {(l) => (
                <LogLine time="" level="debug">
                  <pre class="log-chunk">{l.text}</pre>
                </LogLine>
              )}
            </For>
          </Logs>
        </Card>
      </Show>
    </>
  );
}
