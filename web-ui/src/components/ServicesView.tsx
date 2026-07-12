import { For, Show, createResource, createSignal, onCleanup } from "solid-js";
import { createStore, unwrap } from "solid-js/store";
import { Badge, Button, Card, Checkbox, Empty, Input, Modal, PageHead, Select, Table, Textarea, toast } from "@forge/ui";
import { Activity, CalendarClock, CheckCircle2, Eye, EyeOff, Pencil, Play, Plus, Server, Trash2 } from "lucide-solid";
import type { AssignmentDef, ScheduleDef, ServiceDef, SystemDef } from "../api";
import { createSchedule, createService, createSystem, deleteSchedule, deleteService, deleteSystem, listRunbooks, listServices, listSystemRuns, runScheduleNow, runbookInventory, startSystemRun, updateSchedule, updateService, updateSystem } from "../api";
import { notifyServicesChanged, setView } from "../store";
import ServiceLogs from "./ServiceLogs";
import ServiceMonitoring from "./ServiceMonitoring";

const emptyService = (): ServiceDef => ({ name: "", description: null, systems: [], schedules: [] });
const emptySystem = (): SystemDef => ({
  name: "", description: null, kind: "direct", os: "linux", arch: "x86_64",
  transport: { kind: "ssh", host: "", port: null, user: "", password: null, private_key: null, use_tls: false },
  assignments: [],
});

export default function ServicesView() {
  const [services, { refetch }] = createResource(listServices);
  const [runs] = createResource(listSystemRuns);
  const [editing, setEditing] = createSignal<ServiceDef | null | undefined>(undefined);
  const remove = async (service: ServiceDef) => {
    if (!confirm(`Delete service "${service.name}" and its ${service.systems.length} system(s)?`)) return;
    try { await deleteService(service.name); notifyServicesChanged(); void refetch(); }
    catch (e: any) { toast(e?.message ?? "delete failed", { tone: "danger" }); }
  };
  return <>
    <PageHead title="Services" sub="Operational boundaries for systems, configuration, and activity" actions={<Button size="sm" icon={Plus} onClick={() => setEditing(null)}>New service</Button>} />
    <div class="service-grid">
      <For each={services() ?? []} fallback={<Card><Empty title="No services yet"><span class="sub">Create a service, then add the systems that deliver it.</span></Empty></Card>}>
        {(service) => {
          const recent = () => (runs() ?? []).filter((r) => r.service === service.name);
          const assignments = () => service.systems.reduce((n, s) => n + s.assignments.length, 0);
          return <Card>
            <button class="service-card-main" onClick={() => setView({ kind: "service", name: service.name })}>
              <span class="service-mark"><Server size={18} /></span>
              <span class="service-card-copy"><strong>{service.name}</strong><span class="sub">{service.description || "No description"}</span></span>
              <span class="service-metric"><strong>{service.systems.length}</strong><small>systems</small></span>
              <span class="service-metric"><strong>{assignments()}</strong><small>assignments</small></span>
              <span class="service-metric"><strong>{recent().length}</strong><small>runs</small></span>
            </button>
            <div class="service-card-actions"><Button size="sm" variant="ghost" icon={Pencil} onClick={() => setEditing(service)}>Edit</Button><Button size="sm" variant="ghost" icon={Trash2} onClick={() => remove(service)} /></div>
          </Card>;
        }}
      </For>
    </div>
    <Show when={editing() !== undefined}><ServiceForm initial={editing() ?? emptyService()} original={editing()?.name ?? ""} onDone={(saved) => { setEditing(undefined); if (saved) void refetch(); }} /></Show>
  </>;
}

export function ServiceView(props: { name: string; tab?: "overview" | "systems" | "schedules" | "monitoring" | "logs" }) {
  const [services, { refetch }] = createResource(listServices);
  const [runs, { refetch: refetchRuns }] = createResource(listSystemRuns);
  const [editing, setEditing] = createSignal<string | null | undefined>(undefined);
  const [editingSchedule, setEditingSchedule] = createSignal<string | null | undefined>(undefined);
  const [busy, setBusy] = createSignal<string | null>(null);
  const service = () => (services() ?? []).find((s) => s.name === props.name);
  const serviceRuns = () => (runs() ?? []).filter((r) => r.service === props.name);
  const tab = () => props.tab ?? "overview";
  const timer = setInterval(() => void refetchRuns(), 2500);
  onCleanup(() => clearInterval(timer));
  const launch = async (system: SystemDef, assignment: AssignmentDef, action: "check" | "apply") => {
    setBusy(`${system.name}:${assignment.playbook}:${assignment.play}:${action}`);
    try {
      const { id } = await startSystemRun(props.name, system.name, { ...assignment, action });
      setView({ kind: "sysrun", id, service: props.name, system: system.name, action, ...assignment });
    } catch (e: any) { toast(e?.message ?? "cannot start the run", { tone: "danger" }); }
    finally { setBusy(null); }
  };
  const removeSystem = async (name: string) => {
    if (!confirm(`Delete system "${name}" from ${props.name}?`)) return;
    try { await deleteSystem(props.name, name); notifyServicesChanged(); void refetch(); }
    catch (e: any) { toast(e?.message ?? "delete failed", { tone: "danger" }); }
  };
  const removeSchedule = async (name: string) => {
    if (!confirm(`Delete schedule "${name}"? Its run history remains available this session.`)) return;
    try { await deleteSchedule(props.name, name); notifyServicesChanged(); void refetch(); }
    catch (e: any) { toast(e?.message ?? "delete failed", { tone: "danger" }); }
  };
  const runNow = async (schedule: ScheduleDef) => {
    setBusy(`schedule:${schedule.name}`);
    try {
      const { id } = await runScheduleNow(props.name, schedule.name);
      setView({ kind: "sysrun", id, service: props.name, system: schedule.system, action: schedule.action, playbook: schedule.playbook, play: schedule.play });
    } catch (e: any) { toast(e?.message ?? "cannot start the schedule", { tone: "danger" }); }
    finally { setBusy(null); }
  };
  return <Show when={service()} fallback={<Empty title="Service not found" action={<Button onClick={() => setView({ kind: "services" })}>Back to services</Button>} />} keyed>
    {(svc) => <>
      <PageHead title={svc.name} sub={svc.description || "Service workspace"} actions={<><Show when={tab() === "systems"}><Button size="sm" icon={Plus} onClick={() => setEditing(null)}>Add system</Button></Show><Show when={tab() === "schedules"}><Button size="sm" icon={Plus} onClick={() => setEditingSchedule(null)}>New schedule</Button></Show></>} />
      <nav class="service-tabs" aria-label="Service sections">
        <button classList={{ active: tab() === "overview" }} onClick={() => setView({ kind: "service", name: props.name, tab: "overview" })}>Overview</button>
        <button classList={{ active: tab() === "systems" }} onClick={() => setView({ kind: "service", name: props.name, tab: "systems" })}>Systems <Badge tone="neutral">{svc.systems.length}</Badge></button>
        <button classList={{ active: tab() === "schedules" }} onClick={() => setView({ kind: "service", name: props.name, tab: "schedules" })}>Schedules <Badge tone="neutral">{svc.schedules.length}</Badge></button>
        <button classList={{ active: tab() === "monitoring" }} onClick={() => setView({ kind: "service", name: props.name, tab: "monitoring" })}>Monitoring</button>
        <button classList={{ active: tab() === "logs" }} onClick={() => setView({ kind: "service", name: props.name, tab: "logs" })}>Logs</button>
      </nav>
      <Show when={tab() === "overview"}>
        <div class="overview-strip"><Card title="Systems"><div class="big-number">{svc.systems.length}</div><span class="sub">managed targets</span></Card><Card title="Assignments"><div class="big-number">{svc.systems.reduce((n, s) => n + s.assignments.length, 0)}</div><span class="sub">configuration paths</span></Card><Card title="Schedules"><div class="big-number">{svc.schedules.length}</div><span class="sub">automated actions</span></Card><Card title="Activity"><div class="big-number">{serviceRuns().length}</div><span class="sub">runs this session</span></Card></div>
        <ActivityTable runs={serviceRuns()} empty="No service activity yet" onRefresh={() => void refetchRuns()} />
      </Show>
      <Show when={tab() === "systems"}>
        <For each={svc.systems} fallback={<Card><Empty title="No systems"><span class="sub">Add the first target for this service.</span></Empty></Card>}>
          {(system) => <Card>
            <div class="system-head"><span class="service-mark"><Server size={16} /></span><div><strong>{system.name}</strong><div class="sub">{system.description || `${system.transport.user}@${system.transport.host}`}</div></div><Badge tone={system.kind === "direct" ? "neutral" : "info"}>{system.kind}</Badge><span class="mono">{system.os}/{system.arch}</span><span class="ve-spacer" /><Button size="sm" variant="ghost" icon={Pencil} onClick={() => setEditing(system.name)}>Edit</Button><Button size="sm" variant="ghost" icon={Trash2} onClick={() => removeSystem(system.name)} /></div>
            <div class="assignment-list"><For each={system.assignments} fallback={<div class="empty-assignment">No playbooks assigned</div>}>
              {(a) => <div class="assignment-row"><div><strong>{a.playbook}</strong><span class="sub"> / {a.play}</span></div><Button size="sm" variant="ghost" icon={CheckCircle2} disabled={busy() !== null} onClick={() => launch(system, a, "check")}>Check</Button><Button size="sm" icon={Play} disabled={busy() !== null} onClick={() => launch(system, a, "apply")}>Apply</Button></div>}
            </For></div>
          </Card>}
        </For>
      </Show>
      <Show when={tab() === "schedules"}>
        <For each={svc.schedules} fallback={<Card><Empty title="No schedules"><span class="sub">Automate checks and applies for assigned playbooks.</span></Empty></Card>}>
          {(schedule) => {
            const history = () => serviceRuns().filter((r) => r.schedule === schedule.name);
            return <Card>
              <div class="schedule-head"><span class="service-mark"><CalendarClock size={16} /></span><div><strong>{schedule.name}</strong><div class="sub mono">{schedule.cron} · UTC</div></div><Badge tone={schedule.enabled ? "success" : "neutral"}>{schedule.enabled ? "enabled" : "paused"}</Badge><span class="mono">{schedule.system} · {schedule.playbook}:{schedule.play}</span><Badge tone={schedule.action === "apply" ? "warning" : "info"}>{schedule.action}</Badge><span class="ve-spacer" /><Button size="sm" icon={Play} disabled={busy() !== null} onClick={() => runNow(schedule)}>Run now</Button><Button size="sm" variant="ghost" icon={Pencil} onClick={() => setEditingSchedule(schedule.name)}>Edit</Button><Button size="sm" variant="ghost" icon={Trash2} onClick={() => removeSchedule(schedule.name)} /></div>
              <div class="schedule-history"><Show when={history().length} fallback={<div class="empty-assignment">No runs from this schedule yet</div>}><Table><thead><tr><th>Started</th><th>Trigger</th><th>Status</th><th>Run</th></tr></thead><tbody><For each={history()}>{(run) => <tr class="clickable-row" onClick={() => setView({ kind: "sysrun", id: run.id, service: run.service, system: run.system, action: run.action, playbook: run.playbook, play: run.play })}><td>{new Date(run.started_at).toLocaleString()}</td><td>{run.trigger === "scheduled" ? "Schedule" : "Run now"}</td><td><Badge tone={run.status === "succeeded" ? "success" : run.status === "running" ? "warning" : "danger"}>{run.status}</Badge></td><td class="mono">{run.id.slice(0, 8)}</td></tr>}</For></tbody></Table></Show></div>
            </Card>;
          }}
        </For>
      </Show>
      <Show when={tab() === "monitoring"}>
        <ServiceMonitoring service={props.name} systems={svc.systems.map((s) => s.name)} />
      </Show>
      <Show when={tab() === "logs"}>
        <ServiceLogs service={props.name} systems={svc.systems.map((s) => s.name)} />
      </Show>
      <Show when={editing() !== undefined}><SystemForm service={props.name} original={editing() ?? ""} initial={editing() ? svc.systems.find((s) => s.name === editing()) ?? emptySystem() : emptySystem()} onDone={(saved) => { setEditing(undefined); if (saved) void refetch(); }} /></Show>
      <Show when={editingSchedule() !== undefined}><ScheduleForm service={props.name} systems={svc.systems} original={editingSchedule() ?? ""} initial={editingSchedule() ? svc.schedules.find((s) => s.name === editingSchedule())! : null} onDone={(saved) => { setEditingSchedule(undefined); if (saved) void refetch(); }} /></Show>
    </>}
  </Show>;
}

export function ActivityTable(props: { runs: any[]; empty: string; onRefresh?: () => void }) {
  return <Card title="Recent activity" action={<Activity size={16} />}><Show when={props.runs.length} fallback={<Empty title={props.empty} />}><Table><thead><tr><th>Target</th><th>Assignment</th><th>Action</th><th>Status</th></tr></thead><tbody><For each={props.runs}>{(r) => <tr class="clickable-row" onClick={() => setView({ kind: "sysrun", id: r.id, service: r.service, system: r.system, action: r.action, playbook: r.playbook, play: r.play })}><td>{r.system}</td><td class="mono">{r.playbook}:{r.play}</td><td>{r.action}</td><td><Badge tone={r.status === "succeeded" ? "success" : r.status === "running" ? "warning" : "danger"}>{r.status}</Badge></td></tr>}</For></tbody></Table></Show></Card>;
}

function ServiceForm(props: { initial: ServiceDef; original: string; onDone: (saved: boolean) => void }) {
  const [def, setDef] = createStore<ServiceDef>(structuredClone(props.initial)); const [saving, setSaving] = createSignal(false);
  const save = async () => { setSaving(true); try { const payload = unwrap(def); props.original ? await updateService(props.original, payload) : await createService(payload); notifyServicesChanged(); toast(`saved ${def.name}`, { tone: "success" }); props.onDone(true); } catch (e: any) { toast(e?.message ?? "save failed", { tone: "danger" }); } finally { setSaving(false); } };
  return <Modal open title={props.original ? `Edit ${props.original}` : "New service"} onClose={() => props.onDone(false)} footer={<><Button variant="ghost" onClick={() => props.onDone(false)}>Cancel</Button><Button disabled={saving() || !def.name} onClick={save}>Save</Button></>}><div class="system-form"><Input label="Name" value={def.name} onInput={(e) => setDef("name", e.currentTarget.value)} /><Textarea label="Description" rows={3} value={def.description ?? ""} onInput={(e) => setDef("description", e.currentTarget.value || null)} /></div></Modal>;
}

function ScheduleForm(props: { service: string; systems: SystemDef[]; original: string; initial: ScheduleDef | null; onDone: (saved: boolean) => void }) {
  const [def, setDef] = createStore<ScheduleDef>(structuredClone(props.initial ?? { name: "", system: "", playbook: "", play: "", action: "check", cron: "0 0 * * * *", enabled: true }));
  const [saving, setSaving] = createSignal(false);
  const system = () => props.systems.find((s) => s.name === def.system);
  const assignmentValue = () => def.playbook && def.play ? `${def.playbook}\u0000${def.play}` : "";
  const save = async () => {
    setSaving(true);
    try {
      const payload = unwrap(def);
      props.original ? await updateSchedule(props.service, props.original, payload) : await createSchedule(props.service, payload);
      notifyServicesChanged(); toast(`saved ${def.name}`, { tone: "success" }); props.onDone(true);
    } catch (e: any) { toast(e?.message ?? "save failed", { tone: "danger" }); }
    finally { setSaving(false); }
  };
  return <Modal open size="lg" title={props.original ? `Edit ${props.original}` : "New schedule"} onClose={() => props.onDone(false)} footer={<><Button variant="ghost" onClick={() => props.onDone(false)}>Cancel</Button><Button disabled={saving() || !def.name || !def.system || !def.playbook || !def.play || !def.cron} onClick={save}>Save schedule</Button></>}>
    <div class="system-form">
      <Input label="Name" value={def.name} onInput={(e) => setDef("name", e.currentTarget.value)} />
      <div class="form-grid"><Select label="System" placeholder="choose system" options={props.systems.map((s) => ({ value: s.name, label: s.name }))} value={def.system} onChange={(system) => { setDef("system", system); setDef("playbook", ""); setDef("play", ""); }} /><Select label="Assigned playbook / play" placeholder={def.system ? "choose assignment" : "choose a system first"} options={(system()?.assignments ?? []).map((a) => ({ value: `${a.playbook}\u0000${a.play}`, label: `${a.playbook} / ${a.play}` }))} value={assignmentValue()} onChange={(value) => { const [playbook, play] = value.split("\u0000"); setDef("playbook", playbook); setDef("play", play); }} /><Select label="Action" options={[{ value: "check", label: "Check configuration" }, { value: "apply", label: "Apply configuration" }]} value={def.action} onChange={(action) => setDef("action", action as ScheduleDef["action"])} /></div>
      <div class="form-grid"><Select label="Common schedule" options={[{ value: "0 */15 * * * *", label: "Every 15 minutes" }, { value: "0 0 * * * *", label: "Every hour" }, { value: "0 0 2 * * *", label: "Daily at 02:00 UTC" }, { value: "0 0 2 * * Sun", label: "Weekly Sunday at 02:00 UTC" }]} value={def.cron} onChange={(cron) => setDef("cron", cron)} /><Input label="Cron expression (UTC, seconds first)" value={def.cron} onInput={(e) => setDef("cron", e.currentTarget.value)} /></div>
      <Checkbox checked={def.enabled} onChange={(enabled) => setDef("enabled", enabled)}>Enabled</Checkbox>
      <div class="sub">Six-field cron: second, minute, hour, day of month, month, day of week.</div>
    </div>
  </Modal>;
}

function SystemForm(props: { service: string; original: string; initial: SystemDef; onDone: (saved: boolean) => void }) {
  const [def, setDef] = createStore<SystemDef>(structuredClone(props.initial)); const [saving, setSaving] = createSignal(false); const [showPassword, setShowPassword] = createSignal(false); const [playbooks] = createResource(() => listRunbooks().then((r) => r.runbooks));
  const save = async () => { setSaving(true); try { const payload = unwrap(def); props.original ? await updateSystem(props.service, props.original, payload) : await createSystem(props.service, payload); notifyServicesChanged(); toast(`saved ${def.name}`, { tone: "success" }); props.onDone(true); } catch (e: any) { toast(e?.message ?? "save failed", { tone: "danger" }); } finally { setSaving(false); } };
  const addAssignment = () => setDef("assignments", def.assignments.length, { playbook: "", play: "" });
  return <Modal open size="lg" title={props.original ? `Edit ${props.original}` : "Add system"} onClose={() => props.onDone(false)} footer={<><Button variant="ghost" onClick={() => props.onDone(false)}>Cancel</Button><Button disabled={saving() || !def.name} onClick={save}>Save</Button></>}><div class="system-form">
    <Input label="Name" value={def.name} onInput={(e) => setDef("name", e.currentTarget.value)} /><Textarea label="Description" rows={2} value={def.description ?? ""} onInput={(e) => setDef("description", e.currentTarget.value || null)} />
    <div class="form-grid"><Select label="Kind" options={[{ value: "direct", label: "direct — runs on target" }, { value: "remote", label: "remote — runs on server" }]} value={def.kind} onChange={(v) => setDef("kind", v as SystemDef["kind"])} /><Select label="OS" options={[{ value: "linux", label: "linux" }, { value: "windows", label: "windows" }]} value={def.os} onChange={(v) => setDef("os", v as SystemDef["os"])} /><Select label="Arch" options={[{ value: "x86_64", label: "x86_64" }]} value={def.arch} onChange={(v) => setDef("arch", v)} /></div>
    <div class="form-section">Transport</div><div class="form-grid"><Select label="Protocol" options={[{ value: "ssh", label: "ssh" }, { value: "winrm", label: "winrm" }]} value={def.transport.kind} onChange={(v) => setDef("transport", "kind", v as any)} /><Input label="Host" value={def.transport.host} onInput={(e) => setDef("transport", "host", e.currentTarget.value)} /><Input label="Port" type="number" value={def.transport.port ?? ""} onInput={(e) => setDef("transport", "port", e.currentTarget.value ? Number(e.currentTarget.value) : null)} /></div>
    <div class="form-grid"><Input label="User" value={def.transport.user} onInput={(e) => setDef("transport", "user", e.currentTarget.value)} /><Input label={<span>Password <a class="crumb-link" onClick={() => setShowPassword(!showPassword())}>{showPassword() ? <EyeOff size={12} /> : <Eye size={12} />}</a></span>} type={showPassword() ? "text" : "password"} value={def.transport.password ?? ""} onInput={(e) => setDef("transport", "password", e.currentTarget.value || null)} /></div>
    <Show when={def.transport.kind === "ssh"}><Textarea label="Private key (path or inline PEM)" rows={3} class="mono" value={def.transport.private_key ?? ""} onInput={(e) => setDef("transport", "private_key", e.currentTarget.value || null)} /></Show><Show when={def.transport.kind === "winrm"}><Checkbox checked={def.transport.use_tls} onChange={(v) => setDef("transport", "use_tls", v)}>Use TLS</Checkbox></Show>
    <div class="form-section assignment-heading"><span>Playbook assignments</span><Button size="sm" variant="ghost" icon={Plus} onClick={addAssignment}>Assign</Button></div>
    <For each={def.assignments}>{(assignment, index) => <AssignmentEditor assignment={assignment} playbooks={playbooks() ?? []} onChange={(next) => setDef("assignments", index(), next)} onRemove={() => setDef("assignments", def.assignments.filter((_, i) => i !== index()))} />}</For>
  </div></Modal>;
}

function AssignmentEditor(props: { assignment: AssignmentDef; playbooks: { name: string }[]; onChange: (a: AssignmentDef) => void; onRemove: () => void }) {
  const [inventory] = createResource(() => props.assignment.playbook || undefined, runbookInventory);
  return <div class="assignment-editor"><Select label="Playbook" placeholder="choose playbook" options={props.playbooks.map((p) => ({ value: p.name, label: p.name }))} value={props.assignment.playbook} onChange={(playbook) => props.onChange({ playbook, play: "" })} /><Select label="Play" placeholder="choose play" options={(inventory()?.plays ?? []).map((p) => ({ value: p.name, label: p.name }))} value={props.assignment.play} onChange={(play) => props.onChange({ ...props.assignment, play })} /><Button size="sm" variant="ghost" icon={Trash2} onClick={props.onRemove} /></div>;
}
