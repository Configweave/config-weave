// Systems inventory: the machines configuration is applied to. Each row
// carries its playbook:play assignment, kind (direct = config-weave runs
// on the target; remote = the playbook runs on the server and connects
// out), os/arch (binary selection), and ssh/winrm transport. Check and
// Apply launch system runs with live progress.

import { For, Show, createResource, createSignal } from "solid-js";
import { createStore, unwrap } from "solid-js/store";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Empty,
  Input,
  Modal,
  PageHead,
  Select,
  Table,
  Textarea,
  toast,
} from "@forge/ui";
import { CheckCircle2, Pencil, Play, Plus, Trash2 } from "lucide-solid";
import type { Inventory, SystemDef } from "../api";
import {
  createSystem,
  deleteSystem,
  listRunbooks,
  listSystems,
  runbookInventory,
  startSystemRun,
  updateSystem,
} from "../api";
import { setView } from "../store";

const emptySystem = (): SystemDef => ({
  name: "",
  description: null,
  playbook: "",
  play: "",
  kind: "direct",
  os: "linux",
  arch: "x86_64",
  transport: {
    kind: "ssh",
    host: "",
    port: null,
    user: "",
    password: null,
    private_key: null,
    use_tls: false,
  },
});

export default function SystemsView() {
  const [systems, { refetch }] = createResource(listSystems);
  const [editing, setEditing] = createSignal<string | null>(null); // original name, "" = new
  const [busy, setBusy] = createSignal<string | null>(null);

  const run = async (name: string, action: "check" | "apply") => {
    setBusy(`${name}:${action}`);
    try {
      const { id } = await startSystemRun(name, { action });
      setView({ kind: "sysrun", id, system: name, action });
    } catch (e: any) {
      toast(e?.message ?? "cannot start the run", { tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const remove = async (name: string) => {
    if (!confirm(`Delete system "${name}"? The entry is removed from systems.wcl.`)) return;
    try {
      await deleteSystem(name);
      void refetch();
    } catch (e: any) {
      toast(e?.message ?? "delete failed", { tone: "danger" });
    }
  };

  const transportLabel = (s: SystemDef) => {
    const port = s.transport.port ? `:${s.transport.port}` : "";
    return `${s.transport.kind} ${s.transport.user}@${s.transport.host}${port}`;
  };

  return (
    <>
      <PageHead
        title="Systems"
        sub="Machines this server applies configuration to (systems.wcl)"
        actions={
          <Button size="sm" icon={Plus} onClick={() => setEditing("")}>
            Add system
          </Button>
        }
      />
      <Card>
        <Show
          when={(systems() ?? []).length > 0}
          fallback={
            <Empty title="No systems yet">
              <span class="sub">Add a system to assign it a playbook.</span>
            </Empty>
          }
        >
          <Table>
            <thead>
              <tr>
                <th>System</th>
                <th>Playbook</th>
                <th>Kind</th>
                <th>Target</th>
                <th>Transport</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <For each={systems() ?? []}>
                {(s) => (
                  <tr>
                    <td>
                      <div>{s.name}</div>
                      <Show when={s.description}>
                        <div class="sub">{s.description}</div>
                      </Show>
                    </td>
                    <td class="mono">
                      {s.playbook}:{s.play}
                    </td>
                    <td>
                      <Badge tone={s.kind === "direct" ? "neutral" : "info"}>{s.kind}</Badge>
                    </td>
                    <td class="mono">
                      {s.os}/{s.arch}
                    </td>
                    <td class="mono">{transportLabel(s)}</td>
                    <td>
                      <div class="row-actions">
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={CheckCircle2}
                          disabled={busy() !== null}
                          onClick={() => run(s.name, "check")}
                        >
                          Check
                        </Button>
                        <Button
                          size="sm"
                          icon={Play}
                          disabled={busy() !== null}
                          onClick={() => run(s.name, "apply")}
                        >
                          Apply
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={Pencil}
                          onClick={() => setEditing(s.name)}
                        />
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={Trash2}
                          onClick={() => remove(s.name)}
                        />
                      </div>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>

      <Show when={editing() !== null}>
        <SystemForm
          original={editing()!}
          initial={
            editing()
              ? (systems() ?? []).find((s) => s.name === editing()) ?? emptySystem()
              : emptySystem()
          }
          onDone={(saved) => {
            setEditing(null);
            if (saved) void refetch();
          }}
        />
      </Show>
    </>
  );
}

function SystemForm(props: {
  original: string; // "" = creating
  initial: SystemDef;
  onDone: (saved: boolean) => void;
}) {
  const [def, setDef] = createStore<SystemDef>(structuredClone(props.initial));
  const [saving, setSaving] = createSignal(false);
  const [runbooks] = createResource(listRunbooks);
  const [inventory] = createResource(
    () => def.playbook || undefined,
    (rb) => runbookInventory(rb).catch(() => null as Inventory | null),
  );

  const save = async () => {
    setSaving(true);
    try {
      const payload = unwrap(def);
      if (props.original === "") await createSystem(payload);
      else await updateSystem(props.original, payload);
      toast(`saved ${def.name}`, { tone: "success" });
      props.onDone(true);
    } catch (e: any) {
      toast(e?.message ?? "save failed", { tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      open
      title={props.original === "" ? "Add system" : `Edit ${props.original}`}
      onClose={() => props.onDone(false)}
      footer={
        <>
          <Button variant="ghost" onClick={() => props.onDone(false)}>
            Cancel
          </Button>
          <Button onClick={save} disabled={saving()}>
            Save
          </Button>
        </>
      }
    >
      <div class="system-form">
        <Input
          label="Name"
          value={def.name}
          onInput={(e) => setDef("name", e.currentTarget.value)}
        />
        <Textarea
          label="Description"
          rows={2}
          value={def.description ?? ""}
          onInput={(e) => setDef("description", e.currentTarget.value || null)}
        />
        <div class="form-grid">
          <Select
            label="Playbook"
            placeholder="pick a runbook"
            options={(runbooks() ?? []).map((r) => ({ value: r.name, label: r.name }))}
            value={def.playbook}
            onChange={(v) => {
              setDef("playbook", v);
              setDef("play", "");
            }}
          />
          <Select
            label="Play"
            placeholder={def.playbook ? "pick a play" : "pick a playbook first"}
            options={(inventory()?.plays ?? []).map((p) => ({ value: p.name, label: p.name }))}
            value={def.play}
            onChange={(v) => setDef("play", v)}
          />
        </div>
        <div class="form-grid">
          <Select
            label="Kind"
            options={[
              { value: "direct", label: "direct — config-weave runs on the target" },
              { value: "remote", label: "remote — playbook runs on the server" },
            ]}
            value={def.kind}
            onChange={(v) => setDef("kind", v as SystemDef["kind"])}
          />
          <Select
            label="OS"
            options={[
              { value: "linux", label: "linux" },
              { value: "windows", label: "windows" },
            ]}
            value={def.os}
            onChange={(v) => setDef("os", v as SystemDef["os"])}
          />
          <Select
            label="Arch"
            options={[{ value: "x86_64", label: "x86_64" }]}
            value={def.arch}
            onChange={(v) => setDef("arch", v)}
          />
        </div>

        <div class="form-section">Transport</div>
        <div class="form-grid">
          <Select
            label="Protocol"
            options={[
              { value: "ssh", label: "ssh" },
              { value: "winrm", label: "winrm" },
            ]}
            value={def.transport.kind}
            onChange={(v) => setDef("transport", "kind", v as "ssh" | "winrm")}
          />
          <Input
            label="Host"
            value={def.transport.host}
            onInput={(e) => setDef("transport", "host", e.currentTarget.value)}
          />
          <Input
            label={`Port (default ${def.transport.kind === "ssh" ? 22 : def.transport.use_tls ? 5986 : 5985})`}
            type="number"
            value={def.transport.port ?? ""}
            onInput={(e) =>
              setDef(
                "transport",
                "port",
                e.currentTarget.value ? Number(e.currentTarget.value) : null,
              )
            }
          />
        </div>
        <div class="form-grid">
          <Input
            label="User"
            value={def.transport.user}
            onInput={(e) => setDef("transport", "user", e.currentTarget.value)}
          />
          <PasswordInput
            value={def.transport.password ?? ""}
            onChange={(v) => setDef("transport", "password", v || null)}
          />
        </div>
        <Show when={def.transport.kind === "ssh"}>
          <Textarea
            label="Private key (path or inline PEM; empty = agent/ssh config)"
            rows={3}
            class="mono"
            value={def.transport.private_key ?? ""}
            onInput={(e) => setDef("transport", "private_key", e.currentTarget.value || null)}
          />
        </Show>
        <Show when={def.transport.kind === "winrm"}>
          <Checkbox
            checked={def.transport.use_tls}
            onChange={(checked) => setDef("transport", "use_tls", checked)}
          >
            Use TLS (HTTPS, port 5986)
          </Checkbox>
        </Show>
      </div>
    </Modal>
  );
}

function PasswordInput(props: { value: string; onChange: (v: string) => void }) {
  const [show, setShow] = createSignal(false);
  return (
    <div class="password-field">
      <Input
        label={
          <span>
            Password{" "}
            <a class="crumb-link" onClick={() => setShow(!show())}>
              ({show() ? "hide" : "show"})
            </a>
          </span>
        }
        type={show() ? "text" : "password"}
        value={props.value}
        onInput={(e) => props.onChange(e.currentTarget.value)}
      />
    </div>
  );
}
