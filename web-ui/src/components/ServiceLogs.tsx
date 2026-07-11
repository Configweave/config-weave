import { For, Show, createResource, createSignal, onCleanup } from "solid-js";
import { Badge, Button, Card, Empty, Input, Select, Table } from "@forge/ui";
import { RefreshCw } from "lucide-solid";
import type { LogEntry } from "../api";
import { getMonitoringStatus, getServiceLogs } from "../api";
import { setView } from "../store";

const RANGES = [
  { value: "15m", label: "Last 15 minutes" },
  { value: "1h", label: "Last hour" },
  { value: "6h", label: "Last 6 hours" },
  { value: "24h", label: "Last 24 hours" },
];

const LEVEL_TONE = (level: string | null): "danger" | "warning" | "info" | "neutral" =>
  level === "error" ? "danger" : level === "warn" ? "warning" : level === "info" ? "info" : "neutral";

export default function ServiceLogs(props: { service: string; systems: string[] }) {
  const [status] = createResource(getMonitoringStatus);
  const [range, setRange] = createSignal("1h");
  const [system, setSystem] = createSignal("");
  const [source, setSource] = createSignal("runs");
  const [level, setLevel] = createSignal("");
  // The search box applies on Enter / the refresh button, not per keystroke.
  const [searchInput, setSearchInput] = createSignal("");
  const [search, setSearch] = createSignal("");

  const logsSource = () =>
    status()?.loki
      ? { service: props.service, range: range(), system: system(), source: source(), level: level(), search: search() }
      : undefined;
  const [logs, { refetch }] = createResource(logsSource, (k) =>
    getServiceLogs(k.service, {
      range: k.range,
      system: k.system || undefined,
      source: k.source,
      level: k.level || undefined,
      search: k.search || undefined,
      limit: 500,
    }),
  );
  const timer = setInterval(() => {
    if (status()?.loki) void refetch();
  }, 10000);
  onCleanup(() => clearInterval(timer));

  const openRun = (entry: LogEntry) => {
    if (!entry.run_id || !entry.system || !entry.playbook || !entry.play) return;
    setView({
      kind: "sysrun",
      id: entry.run_id,
      service: props.service,
      system: entry.system,
      action: entry.action ?? "check",
      playbook: entry.playbook,
      play: entry.play,
    });
  };

  return (
    <Show when={status()} keyed fallback={<Card><Empty title="Checking logging backends…" /></Card>}>
      {(st) => (
        <Show
          when={st.loki}
          fallback={
            <Card>
              <Empty title="Logs not configured">
                <span class="sub">
                  Start weave-server with <code>--loki-url</code> (or <code>LOKI_URL</code>) to ship server + run logs
                  to Loki — <code>just stack-up</code> runs the test stack.
                </span>
              </Empty>
            </Card>
          }
        >
          <div class="logs-controls">
            <Select label="Range" options={RANGES} value={range()} onChange={(v) => setRange(v)} />
            <Select
              label="System"
              options={[{ value: "", label: "All systems" }, ...props.systems.map((s) => ({ value: s, label: s }))]}
              value={system()}
              onChange={(v) => setSystem(v)}
            />
            <Select
              label="Source"
              options={[
                { value: "runs", label: "Run + server logs" },
                { value: "systems", label: "System streams" },
              ]}
              value={source()}
              onChange={(v) => setSource(v)}
            />
            <Select
              label="Level"
              options={[
                { value: "", label: "All levels" },
                { value: "info", label: "info" },
                { value: "warn", label: "warn" },
                { value: "error", label: "error" },
              ]}
              value={level()}
              onChange={(v) => setLevel(v)}
            />
            <Input
              label="Search"
              placeholder="line contains…"
              value={searchInput()}
              onInput={(e) => setSearchInput(e.currentTarget.value)}
              onKeyDown={(e: KeyboardEvent) => {
                if (e.key === "Enter") setSearch(searchInput());
              }}
            />
            <Button
              size="sm"
              variant="ghost"
              icon={RefreshCw}
              onClick={() => {
                setSearch(searchInput());
                void refetch();
              }}
            >
              Refresh
            </Button>
          </div>
          <Show when={logs.error}>
            <Card><Empty title="Loki query failed"><span class="sub">{String(logs.error?.message ?? logs.error)}</span></Empty></Card>
          </Show>
          <Card title={`Log lines — ${source() === "runs" ? "run + server logs" : "system streams"}`}>
            <Show
              when={(logs()?.entries ?? []).length}
              fallback={
                <Empty title="No log lines in this range">
                  <span class="sub">
                    {source() === "systems"
                      ? `Nothing shipped with the {service="${props.service}"} label yet — point your agents' Loki labels at the inventory names.`
                      : "Run a check or apply and its output lands here."}
                  </span>
                </Empty>
              }
            >
              <Table>
                <thead>
                  <tr><th>Time</th><th>Level</th><th>System</th><th>Message</th></tr>
                </thead>
                <tbody>
                  <For each={logs()?.entries ?? []}>
                    {(entry) => (
                      <tr
                        classList={{ "clickable-row": !!entry.run_id }}
                        onClick={() => openRun(entry)}
                      >
                        <td class="mono">{new Date(entry.ts).toLocaleTimeString()}</td>
                        <td><Badge tone={LEVEL_TONE(entry.level)}>{entry.level ?? "—"}</Badge></td>
                        <td>{entry.system ?? "—"}</td>
                        <td class="mono log-message">{entry.message}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </Table>
            </Show>
          </Card>
        </Show>
      )}
    </Show>
  );
}
