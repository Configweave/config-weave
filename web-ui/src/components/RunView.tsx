// One test run: live per-test progress driven by the run:{id} event
// topic (seeded from the snapshot buffer), a streaming log, per-instance
// troubleshooting tabs (terminal into docker containers, VNC into vmlab
// VMs), and the final report.

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
  Tabs,
  toast,
} from "@forge/ui";
import type { StatusTone } from "@forge/ui";
import { Terminal } from "@forge/term";
import { DesktopViewer } from "@forge/desktop";
import { Square } from "lucide-solid";
import type { InstanceInfo } from "../api";
import { api, cancelRun, getRun } from "../api";

interface TestState {
  package: string;
  test: string;
  backend: string;
  image: string;
  phase: string;
  outcome: string | null;
  duration: number | null;
  error: string | null;
}

interface LogEntry {
  ts: string;
  test: string;
  context: string;
  text: string;
}

const OUTCOME_TONE: Record<string, StatusTone> = {
  passed: "success",
  failed: "danger",
  error: "danger",
};

const PHASES = ["queued", "setup", "gather", "check", "first_apply", "second_apply", "verify"];

export default function RunView(props: { id: string; runbook: string }) {
  const [tests, setTests] = createStore<Record<string, TestState>>({});
  const [logs, setLogs] = createStore<LogEntry[]>([]);
  const [instances, setInstances] = createStore<InstanceInfo[]>([]);
  const [status, setStatus] = createSignal<string>("running");
  const [report, setReport] = createSignal<any | null>(null);
  const [attachTab, setAttachTab] = createSignal<string | null>(null);

  const key = (e: any) => `${e.package}:${e.test}`;

  const apply = (e: any) => {
    switch (e.event) {
      case "run_started":
        for (const t of e.tests ?? []) {
          setTests(`${t.package}:${t.test}`, {
            package: t.package,
            test: t.test,
            backend: t.backend,
            image: t.image,
            phase: "queued",
            outcome: null,
            duration: null,
            error: null,
          });
        }
        break;
      case "test_started":
        if (tests[key(e)]) setTests(key(e), "phase", "starting");
        break;
      case "phase":
        if (tests[key(e)])
          setTests(key(e), "phase", e.phase === "gather" ? `gather ${e.name}` : e.phase);
        break;
      case "test_finished":
        if (tests[key(e)])
          setTests(
            key(e),
            produce((t) => {
              t.outcome = e.outcome;
              t.duration = e.duration_secs;
              t.error = e.error ?? null;
              t.phase = "done";
            }),
          );
        break;
      case "log":
        setLogs(logs.length, {
          ts: e.ts ? new Date(e.ts).toLocaleTimeString() : "",
          test: key(e),
          context: e.context,
          text: e.chunk,
        });
        break;
      case "instance_ready":
        setInstances(instances.length, { group: e.group, torn_down: false, ...e.attach });
        break;
      case "group_teardown":
        setInstances((i) => i.group === e.group, "torn_down", true);
        break;
      case "run_closed":
        setStatus(e.status);
        void refresh();
        break;
      case "raw":
        setLogs(logs.length, { ts: "", test: "", context: "raw", text: e.line });
        break;
    }
  };

  const refresh = async () => {
    try {
      const snap = await getRun(props.id);
      setStatus(snap.status);
      setReport(snap.report);
      setInstances(reconcile(snap.instances));
    } catch {
      /* run may be gone after a server restart */
    }
  };

  onMount(async () => {
    // Subscribe first, buffering, so nothing falls between the snapshot
    // and the live stream; the buffer replays after the snapshot.
    const pending: any[] = [];
    let replaying = true;
    const unsub = api.events.on(`run:${props.id}`, (data) => {
      if (replaying) pending.push(data);
      else apply(data);
    });
    onCleanup(unsub);

    try {
      const snap = await getRun(props.id);
      for (const e of snap.events) apply(e);
      setStatus(snap.status);
      setReport(snap.report);
    } catch (e: any) {
      toast(e?.message ?? "cannot load the run", { tone: "danger" });
    }
    replaying = false;
    for (const e of pending) apply(e);
  });

  const cancel = async () => {
    try {
      await cancelRun(props.id);
    } catch (e: any) {
      toast(e?.message ?? "cancel failed", { tone: "danger" });
    }
  };

  const statusTone = (): StatusTone =>
    status() === "running"
      ? "warning"
      : status() === "passed"
        ? "success"
        : status() === "cancelled"
          ? "neutral"
          : "danger";

  const attachable = () => instances.filter((i) => !i.torn_down);
  const instanceId = (i: InstanceInfo) =>
    i.kind === "docker" ? (i.container_id ?? "") : `${i.lab}/${i.machine}`;

  return (
    <>
      <PageHead
        title={`${props.runbook} — test run`}
        sub={
          <span class="run-status">
            <StatusDot tone={statusTone()} /> {status()}
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

      <Card title="Tests">
        <Show when={Object.keys(tests).length > 0} fallback={<Empty title="Waiting for the plan…" />}>
          <Table>
            <thead>
              <tr>
                <th>Test</th>
                <th>Backend</th>
                <th>Image</th>
                <th>Progress</th>
                <th>Outcome</th>
              </tr>
            </thead>
            <tbody>
              <For each={Object.values(tests)}>
                {(t) => (
                  <tr>
                    <td>
                      {t.package}:{t.test}
                    </td>
                    <td>
                      <Badge tone={t.backend === "vmlab" ? "info" : "neutral"}>{t.backend}</Badge>
                    </td>
                    <td class="mono">{t.image}</td>
                    <td>
                      <Show when={!t.outcome} fallback={<span class="sub">{fmtSecs(t.duration)}</span>}>
                        <span class="phase-track">
                          <For each={PHASES.slice(1)}>
                            {(p) => (
                              <span
                                class="phase-dot"
                                classList={{
                                  "is-done": phaseRank(t.phase) > PHASES.indexOf(p),
                                  "is-now": t.phase.startsWith(p) || (p === "gather" && t.phase.startsWith("gather")),
                                }}
                                title={p}
                              />
                            )}
                          </For>
                          <span class="sub">{t.phase}</span>
                        </span>
                      </Show>
                    </td>
                    <td>
                      <Show when={t.outcome} fallback={<Badge tone="warning">running</Badge>}>
                        <Badge tone={OUTCOME_TONE[t.outcome!] ?? "neutral"}>{t.outcome}</Badge>
                      </Show>
                      <Show when={t.error}>
                        <div class="sub error-text">{t.error}</div>
                      </Show>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>

      <Show when={attachable().length > 0}>
        <Card title="Instances — troubleshoot">
          <Tabs
            tabs={attachable().map((i) => ({
              id: instanceId(i),
              label:
                i.kind === "docker"
                  ? `⬒ ${i.container_id!.slice(0, 12)} (${i.image})`
                  : `🖵 ${i.machine} (${i.template})`,
            }))}
            active={attachTab() ?? instanceId(attachable()[0])}
            onChange={setAttachTab}
          />
          <For each={attachable()}>
            {(i) => (
              <Show when={(attachTab() ?? instanceId(attachable()[0])) === instanceId(i)}>
                <Show
                  when={i.kind === "docker"}
                  fallback={
                    <DesktopViewer
                      url={api.wsUrl(
                        `/api/desktop/vnc/${encodeURIComponent(props.id)}/${encodeURIComponent(i.machine!)}`,
                      )}
                      autoConnect
                      scale="fit"
                      height="55vh"
                    />
                  }
                >
                  <Terminal
                    url={api.wsUrl(`/api/term/docker/${i.container_id}`)}
                    mode="local"
                    autoConnect
                    height="45vh"
                  />
                </Show>
              </Show>
            )}
          </For>
        </Card>
      </Show>

      <Card title="Log">
        <Show when={logs.length > 0} fallback={<Empty title="No script output yet" />}>
          <Logs class="run-logs">
            <For each={logs}>
              {(l) => (
                <LogLine time={l.ts} level={l.context === "raw" ? "debug" : "info"}>
                  <Show when={l.test}>
                    <strong>{l.test}</strong> [{l.context}]{" "}
                  </Show>
                  <pre class="log-chunk">{l.text}</pre>
                </LogLine>
              )}
            </For>
          </Logs>
        </Show>
      </Card>

      <Show when={report()} keyed>
        {(r) => (
          <Card title={`Report — exit ${r.exit_code}`}>
            <Table>
              <thead>
                <tr>
                  <th>Test</th>
                  <th>Outcome</th>
                  <th>Duration</th>
                  <th>Details</th>
                </tr>
              </thead>
              <tbody>
                <For each={r.tests ?? []}>
                  {(t: any) => (
                    <tr>
                      <td>
                        {t.package}:{t.name}
                      </td>
                      <td>
                        <Badge tone={OUTCOME_TONE[t.outcome] ?? "neutral"}>{t.outcome}</Badge>
                      </td>
                      <td>{fmtSecs(t.duration_secs)}</td>
                      <td class="sub">
                        {t.error ??
                          (t.steps ?? [])
                            .flatMap((s: any) => s.failures ?? [])
                            .join("; ") ??
                          ""}
                        {t.kept ? ` (kept: ${t.kept})` : ""}
                      </td>
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

function phaseRank(phase: string): number {
  const base = phase.split(" ")[0];
  const i = PHASES.indexOf(base);
  return i === -1 ? 0 : i;
}

function fmtSecs(s: number | null): string {
  if (s == null) return "";
  return s >= 60 ? `${Math.floor(s / 60)}m${Math.round(s % 60)}s` : `${s.toFixed(1)}s`;
}
