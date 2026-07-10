// One system run (check/apply against an inventory system): live step
// progress driven by the sysrun:{id} topic (engine events relayed by the
// server, plus server-synthesized deploy_phase events for direct
// systems), a raw log, and the final JsonRunReport.

import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import {
  Badge,
  Button,
  Card,
  Empty,
  LogLine,
  Logs,
  PageHead,
  StatusDot,
  Table,
  toast,
} from "@forge/ui";
import type { StatusTone } from "@forge/ui";
import { Square } from "lucide-solid";
import { api, cancelSystemRun, getSystemRun } from "../api";

interface StepState {
  name: string;
  resource: string;
  containerPath: string[];
  phase: string; // queued | checking | applying | re-checking | done
  status: string | null;
  message: string | null;
  duration: number | null;
}

const STATUS_TONE: Record<string, StatusTone> = {
  already_configured: "success",
  configured: "success",
  not_configured: "warning",
  reboot_required: "warning",
  skipped: "neutral",
  not_run: "neutral",
  error: "danger",
};

const DEPLOY_PHASES = [
  "connect",
  "stage_binary",
  "stage_playbook",
  "run",
  "fetch_report",
  "cleanup",
];

export default function SystemRunView(props: { id: string; system: string; action: string }) {
  const [steps, setSteps] = createStore<StepState[]>([]);
  const [logs, setLogs] = createStore<{ ts: string; text: string }[]>([]);
  const [status, setStatus] = createSignal("running");
  const [deployPhase, setDeployPhase] = createSignal<string | null>(null);
  const [isDirect, setIsDirect] = createSignal(false);
  const [report, setReport] = createSignal<any | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const apply = (e: any) => {
    switch (e.event) {
      case "run_started":
        setSteps(
          (e.steps ?? []).map((s: any) => ({
            name: s.name,
            resource: s.resource,
            containerPath: s.container_path ?? [],
            phase: "queued",
            status: null,
            message: null,
            duration: null,
          })),
        );
        break;
      case "deploy_phase":
        setIsDirect(true);
        setDeployPhase(e.phase);
        break;
      case "deploy_error":
        setError(`${e.phase}: ${e.message}`);
        break;
      case "step_started":
        if (steps[e.idx]) setSteps(e.idx, "phase", "checking");
        break;
      case "step_phase":
        if (steps[e.idx]) setSteps(e.idx, "phase", e.phase);
        break;
      case "step_finished":
        if (steps[e.idx])
          setSteps(
            e.idx,
            produce((s) => {
              s.phase = "done";
              s.status = e.status;
              s.message = e.message ?? null;
              s.duration = e.duration_secs ?? null;
            }),
          );
        break;
      case "step_resolved":
        if (steps[e.idx])
          setSteps(
            e.idx,
            produce((s) => {
              s.phase = "done";
              s.status = e.status;
            }),
          );
        break;
      case "run_closed":
        setStatus(e.status);
        void refresh();
        break;
      case "stage_kept":
        setLogs(logs.length, {
          ts: "",
          text: `staging kept for debugging: ${e.host}:${e.stage}`,
        });
        break;
      case "raw":
        setLogs(logs.length, { ts: "", text: e.line });
        break;
    }
  };

  const refresh = async () => {
    try {
      const snap = await getSystemRun(props.id);
      setStatus(snap.status);
      setReport(snap.report);
      if (snap.kind === "direct") setIsDirect(true);
    } catch {
      /* run may be gone after a server restart */
    }
  };

  /// Rebuild everything from the snapshot's event buffer (the SSE
  /// stream has no replay, so missed events are recovered here).
  const seed = (snap: Awaited<ReturnType<typeof getSystemRun>>) => {
    setSteps(reconcile([]));
    setLogs(reconcile([]));
    setError(null);
    if (snap.kind === "direct") setIsDirect(true);
    for (const e of snap.events) apply(e);
    setStatus(snap.status);
    setReport(snap.report);
  };

  onMount(async () => {
    const pending: any[] = [];
    let replaying = true;
    const unsub = api.events.on(`sysrun:${props.id}`, (data) => {
      if (replaying) pending.push(data);
      else apply(data);
    });
    onCleanup(unsub);

    // Events published before the EventSource finishes connecting are
    // gone (SSE has no replay), so live state can miss the head or the
    // tail of a fast run. The buffer snapshot is authoritative: seed
    // from it now, and keep polling until one final reseed of a
    // *finished* run has happened.
    let sealed = false;
    try {
      const snap = await getSystemRun(props.id);
      seed(snap);
      sealed = snap.status !== "running";
    } catch (e: any) {
      toast(e?.message ?? "cannot load the run", { tone: "danger" });
    }
    replaying = false;
    for (const e of pending) apply(e);

    const timer = setInterval(async () => {
      if (sealed) return;
      try {
        const snap = await getSystemRun(props.id);
        if (snap.status !== "running") {
          sealed = true;
          seed(snap);
        }
      } catch {
        /* transient */
      }
    }, 1200);
    onCleanup(() => clearInterval(timer));
  });

  const cancel = async () => {
    try {
      await cancelSystemRun(props.id);
    } catch (e: any) {
      toast(e?.message ?? "cancel failed", { tone: "danger" });
    }
  };

  const statusTone = (): StatusTone =>
    status() === "running"
      ? "warning"
      : status() === "succeeded"
        ? "success"
        : status() === "reboot_required"
          ? "warning"
          : status() === "cancelled"
            ? "neutral"
            : "danger";

  const stepPath = (s: StepState) =>
    s.containerPath.length ? `${s.containerPath.join("/")}/${s.name}` : s.name;

  return (
    <>
      <PageHead
        title={`${props.system} — ${props.action}`}
        sub={
          <span class="run-status">
            <StatusDot tone={statusTone()} /> {status().replace("_", " ")}
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

      <Show when={error()}>
        <Card>
          <div class="error-text">{error()}</div>
        </Card>
      </Show>

      <Show when={isDirect()}>
        <Card title="Deployment">
          <span class="phase-track">
            <For each={DEPLOY_PHASES}>
              {(p) => (
                <span
                  class="phase-dot"
                  classList={{
                    "is-done":
                      DEPLOY_PHASES.indexOf(deployPhase() ?? "") > DEPLOY_PHASES.indexOf(p),
                    "is-now": deployPhase() === p,
                  }}
                  title={p}
                />
              )}
            </For>
            <span class="sub">{(deployPhase() ?? "waiting").replace("_", " ")}</span>
          </span>
        </Card>
      </Show>

      <Card title="Steps">
        <Show when={steps.length > 0} fallback={<Empty title="Waiting for the plan…" />}>
          <Table>
            <thead>
              <tr>
                <th>Step</th>
                <th>Resource</th>
                <th>Progress</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              <For each={steps}>
                {(s) => (
                  <tr>
                    <td>{stepPath(s)}</td>
                    <td class="mono">{s.resource}</td>
                    <td>
                      <Show
                        when={!s.status}
                        fallback={<span class="sub">{fmtSecs(s.duration)}</span>}
                      >
                        <span class="sub">{s.phase}</span>
                      </Show>
                    </td>
                    <td>
                      <Show
                        when={s.status}
                        fallback={
                          <Badge tone={s.phase === "queued" ? "neutral" : "warning"}>
                            {s.phase === "queued" ? "queued" : "running"}
                          </Badge>
                        }
                      >
                        <Badge tone={STATUS_TONE[s.status!] ?? "neutral"}>
                          {s.status!.replaceAll("_", " ")}
                        </Badge>
                      </Show>
                      <Show when={s.message}>
                        <div class="sub">{s.message}</div>
                      </Show>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>

      <Show when={logs.length > 0}>
        <Card title="Log">
          <Logs class="run-logs">
            <For each={logs}>
              {(l) => (
                <LogLine time={l.ts} level="debug">
                  <pre class="log-chunk">{l.text}</pre>
                </LogLine>
              )}
            </For>
          </Logs>
        </Card>
      </Show>

      <Show when={report()} keyed>
        {(r) => (
          <Card title={`Report — ${r.mode} exit ${r.exit_code}`}>
            <Table>
              <thead>
                <tr>
                  <th>Step</th>
                  <th>Resource</th>
                  <th>Status</th>
                  <th>Duration</th>
                  <th>Message</th>
                </tr>
              </thead>
              <tbody>
                <For each={r.steps ?? []}>
                  {(s: any) => (
                    <tr>
                      <td>
                        {(s.container_path ?? []).length
                          ? `${s.container_path.join("/")}/${s.name}`
                          : s.name}
                      </td>
                      <td class="mono">{s.resource}</td>
                      <td>
                        <Badge tone={STATUS_TONE[s.status] ?? "neutral"}>
                          {String(s.status).replaceAll("_", " ")}
                        </Badge>
                      </td>
                      <td>{fmtSecs(s.duration_secs)}</td>
                      <td class="sub">{s.message ?? ""}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </Table>
          </Card>
        )}
      </Show>
    </>
  );
}

function fmtSecs(s: number | null): string {
  if (s == null) return "";
  return s >= 60 ? `${Math.floor(s / 60)}m${Math.round(s % 60)}s` : `${s.toFixed(1)}s`;
}
