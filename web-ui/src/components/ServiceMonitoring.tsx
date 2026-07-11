import { Show, createResource, createSignal, onCleanup } from "solid-js";
import { Card, Empty, Select } from "@forge/ui";
import { LineChart } from "@forge/charts";
import type { ChartTone, LineSeries } from "@forge/charts";
import { getMonitoringStatus, getServiceMonitoringSummary, getServiceMonitoringTimeseries } from "../api";

const RANGES = [
  { value: "15m", label: "Last 15 minutes" },
  { value: "1h", label: "Last hour" },
  { value: "6h", label: "Last 6 hours" },
  { value: "24h", label: "Last 24 hours" },
];

const STATUS_TONE: Record<string, ChartTone> = {
  succeeded: "success",
  failed: "danger",
  error: "danger",
  reboot_required: "warning",
  cancelled: "info",
};

const fmtSeconds = (s: number) =>
  s >= 90 ? `${(s / 60).toFixed(1)}m` : `${s.toFixed(s >= 10 ? 0 : 1)}s`;

export default function ServiceMonitoring(props: { service: string; systems: string[] }) {
  const [status] = createResource(getMonitoringStatus);
  const [range, setRange] = createSignal("1h");
  const [system, setSystem] = createSignal("");
  // Resources only fire once the capability probe confirms a backend.
  const summarySource = () => (status()?.prometheus ? { service: props.service, range: range() } : undefined);
  const seriesSource = () =>
    status()?.prometheus ? { service: props.service, range: range(), system: system() } : undefined;
  const [summary, { refetch: refetchSummary }] = createResource(summarySource, (k) =>
    getServiceMonitoringSummary(k.service, k.range),
  );
  const [timeseries, { refetch: refetchSeries }] = createResource(seriesSource, (k) =>
    getServiceMonitoringTimeseries(k.service, k.range, k.system || undefined),
  );
  const timer = setInterval(() => {
    if (status()?.prometheus) {
      void refetchSummary();
      void refetchSeries();
    }
  }, 15000);
  onCleanup(() => clearInterval(timer));

  const counts = () => summary()?.run_counts ?? {};
  const count = (k: string) => counts()[k] ?? 0;
  const chart = (): LineSeries[] =>
    (timeseries()?.series ?? []).map((s) => ({
      label: s.name,
      tone: STATUS_TONE[s.name],
      points: s.points.map(([x, y]) => ({ x, y })),
    }));

  return (
    <Show when={status()} keyed fallback={<Card><Empty title="Checking monitoring backends…" /></Card>}>
      {(st) => (
        <Show
          when={st.prometheus}
          fallback={
            <Card>
              <Empty title="Monitoring not configured">
                <span class="sub">
                  Start weave-server with <code>--prometheus-url</code> (or <code>PROMETHEUS_URL</code>) pointing at a
                  Prometheus that scrapes <code>/metrics</code> — <code>just stack-up</code> runs the test stack.
                </span>
              </Empty>
            </Card>
          }
        >
          <div class="monitoring-controls">
            <Select
              label="Range"
              options={RANGES}
              value={range()}
              onChange={(v) => setRange(v)}
            />
            <Select
              label="System"
              options={[{ value: "", label: "All systems" }, ...props.systems.map((s) => ({ value: s, label: s }))]}
              value={system()}
              onChange={(v) => setSystem(v)}
            />
          </div>
          <Show when={summary.error}>
            <Card><Empty title="Prometheus query failed"><span class="sub">{String(summary.error?.message ?? summary.error)}</span></Empty></Card>
          </Show>
          <div class="overview-strip">
            <Card title="Succeeded"><div class="big-number">{Math.round(count("succeeded"))}</div><span class="sub">runs in {range()}</span></Card>
            <Card title="Failed"><div class="big-number">{Math.round(count("failed") + count("error"))}</div><span class="sub">failed + errored</span></Card>
            <Card title="Active"><div class="big-number">{Math.round(summary()?.active ?? 0)}</div><span class="sub">runs in flight</span></Card>
            <Card title="p95 duration"><div class="big-number">{summary()?.p95_duration_s != null ? fmtSeconds(summary()!.p95_duration_s!) : "—"}</div><span class="sub">over {range()}</span></Card>
          </div>
          <Card title={`Runs by status — ${system() || "all systems"}`}>
            <Show
              when={chart().some((s) => s.points.length)}
              fallback={<Empty title="No runs in this range"><span class="sub">Trigger a check or apply and the chart fills in.</span></Empty>}
            >
              <LineChart series={chart()} height={240} area />
            </Show>
          </Card>
        </Show>
      )}
    </Show>
  );
}
