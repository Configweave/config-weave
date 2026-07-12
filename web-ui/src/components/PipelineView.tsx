// One pipeline: its properties, steps, triggers and secret names, a
// Trigger button (prompts for property values), and this pipeline's recent
// runs. Authoring lives in the daemon's pipelines.wcl / the secrets API;
// this view is read + trigger + observe.

import { For, Show, createResource, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { Badge, Button, Card, Empty, Input, Modal, PageHead, Table, toast } from "@forge/ui";
import { Play } from "lucide-solid";
import type { PipelineDetail } from "../api";
import { getPipeline, listPipelineRuns, triggerPipeline } from "../api";
import { setView } from "../store";
import { runStatusTone } from "./PipelineRunView";

export default function PipelineView(props: { name: string }) {
  const [pipeline, { refetch }] = createResource(() => props.name, getPipeline);
  const [runs, { refetch: refetchRuns }] = createResource(listPipelineRuns);
  const [triggering, setTriggering] = createSignal(false);

  const pipelineRuns = () => (runs()?.runs ?? []).filter((r) => r.pipeline === props.name);

  return (
    <>
      <PageHead
        title={props.name}
        sub={pipeline()?.description || "Pipeline"}
        actions={
          <Button size="sm" icon={Play} onClick={() => setTriggering(true)}>
            Trigger
          </Button>
        }
      />

      <Show when={pipeline()} keyed>
        {(p) => (
          <>
            <Card title="Steps">
              <Show when={p.steps.length > 0} fallback={<Empty title="No steps" />}>
                <Table>
                  <thead>
                    <tr>
                      <th>#</th>
                      <th>Step</th>
                      <th>Kind</th>
                      <th>Detail</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={p.steps}>
                      {(s, i) => (
                        <tr>
                          <td class="sub">{i()}</td>
                          <td>{s.name}</td>
                          <td>
                            <Badge tone={s.kind === "play" ? "info" : "neutral"}>{s.kind}</Badge>
                          </td>
                          <td class="mono sub">
                            {s.kind === "script"
                              ? `${s.on === "local" ? "local" : `on ${s.on}`}`
                              : `${s.action} ${s.playbook}:${s.play}`}
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </Table>
              </Show>
            </Card>

            <div class="service-grid">
              <Show when={p.properties.length > 0}>
                <Card title="Properties">
                  <Table>
                    <thead>
                      <tr>
                        <th>Name</th>
                        <th>Type</th>
                        <th>Required</th>
                        <th>Default</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={p.properties}>
                        {(prop) => (
                          <tr>
                            <td>{prop.name}</td>
                            <td class="mono">{prop.type}</td>
                            <td>{prop.required ? "yes" : "no"}</td>
                            <td class="sub">{prop.default ?? ""}</td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </Table>
                </Card>
              </Show>

              <Show when={p.triggers.length > 0}>
                <Card title="Triggers">
                  <Table>
                    <thead>
                      <tr>
                        <th>Name</th>
                        <th>Type</th>
                        <th>Detail</th>
                        <th>Enabled</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={p.triggers}>
                        {(t) => (
                          <tr>
                            <td>{t.name}</td>
                            <td>
                              <Badge tone="neutral">{t.type}</Badge>
                            </td>
                            <td class="mono sub">
                              {t.type === "schedule" ? t.cron ?? "" : t.type === "webhook" ? "secret set" : ""}
                            </td>
                            <td>{t.enabled ? "yes" : "no"}</td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </Table>
                </Card>
              </Show>

              <Show when={p.secrets.length > 0}>
                <Card title="Secrets">
                  <For each={p.secrets}>
                    {(s) => (
                      <div>
                        <span class="mono">{s.name}</span>
                        <Show when={s.description}>
                          <span class="sub"> — {s.description}</span>
                        </Show>
                      </div>
                    )}
                  </For>
                  <div class="sub">Values are write-only (managed via the secrets API).</div>
                </Card>
              </Show>
            </div>

            <Card title="Runs">
              <Show
                when={pipelineRuns().length > 0}
                fallback={<Empty title="No runs yet" />}
              >
                <Table>
                  <thead>
                    <tr>
                      <th>Trigger</th>
                      <th>Started</th>
                      <th>Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={pipelineRuns()}>
                      {(r) => (
                        <tr
                          class="clickable-row"
                          onClick={() =>
                            setView({ kind: "pipelinerun", id: r.id, pipeline: r.pipeline })
                          }
                        >
                          <td class="mono">{r.trigger}</td>
                          <td class="sub">{new Date(r.started_at).toLocaleString()}</td>
                          <td>
                            <Badge tone={runStatusTone(r.status)}>{r.status}</Badge>
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </Table>
              </Show>
            </Card>

            <Show when={triggering()}>
              <TriggerModal
                pipeline={p}
                onDone={(runId) => {
                  setTriggering(false);
                  void refetch();
                  void refetchRuns();
                  if (runId) setView({ kind: "pipelinerun", id: runId, pipeline: p.name });
                }}
              />
            </Show>
          </>
        )}
      </Show>
    </>
  );
}

function TriggerModal(props: { pipeline: PipelineDetail; onDone: (runId?: string) => void }) {
  const [values, setValues] = createStore<Record<string, string>>(
    Object.fromEntries(props.pipeline.properties.map((p) => [p.name, p.default ?? ""])),
  );
  const [busy, setBusy] = createSignal(false);

  const submit = async () => {
    setBusy(true);
    try {
      const { run_id } = await triggerPipeline(props.pipeline.name, { ...values });
      toast(`triggered ${props.pipeline.name}`, { tone: "success" });
      props.onDone(run_id);
    } catch (e: any) {
      toast(e?.message ?? "trigger failed", { tone: "danger" });
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      title={`Trigger ${props.pipeline.name}`}
      onClose={() => props.onDone()}
      footer={
        <>
          <Button variant="ghost" onClick={() => props.onDone()}>
            Cancel
          </Button>
          <Button disabled={busy()} onClick={submit}>
            Run
          </Button>
        </>
      }
    >
      <div class="system-form">
        <Show
          when={props.pipeline.properties.length > 0}
          fallback={<div class="sub">This pipeline takes no properties.</div>}
        >
          <For each={props.pipeline.properties}>
            {(prop) => (
              <Input
                label={`${prop.name}${prop.required ? " *" : ""}`}
                placeholder={prop.type}
                value={values[prop.name] ?? ""}
                onInput={(e) => setValues(prop.name, e.currentTarget.value)}
              />
            )}
          </For>
        </Show>
      </div>
    </Modal>
  );
}
