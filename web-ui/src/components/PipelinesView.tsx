// The Pipelines section: lists pipelines served by a config-weave-pipeline
// daemon (proxied through weave-server) plus recent runs. When no daemon is
// configured (--pipeline-url unset), a hint is shown instead.

import { For, Show, createResource } from "solid-js";
import { Badge, Button, Card, Empty, PageHead, Table } from "@forge/ui";
import { Play, Workflow } from "lucide-solid";
import { getPipelineConfig, listPipelineRuns, listPipelines } from "../api";
import { setView } from "../store";
import { runStatusTone } from "./PipelineRunView";

export default function PipelinesView() {
  const [config] = createResource(getPipelineConfig);
  const [pipelines] = createResource(
    () => config()?.configured ?? false,
    (configured) => (configured ? listPipelines() : Promise.resolve({ pipelines: [] })),
  );
  const [runs] = createResource(
    () => config()?.configured ?? false,
    (configured) => (configured ? listPipelineRuns() : Promise.resolve({ runs: [] })),
  );

  return (
    <>
      <PageHead title="Pipelines" sub="CI/CD primitives — triggered scripts and plays" />
      <Show
        when={config()?.configured}
        fallback={
          <Card>
            <Empty title="No pipeline daemon configured">
              <span class="sub">
                Point weave-server at a config-weave-pipeline daemon with{" "}
                <span class="mono">--pipeline-url</span> (and a forge-auth machine token) to manage
                pipelines here.
              </span>
            </Empty>
          </Card>
        }
      >
        <div class="service-grid">
          <For
            each={pipelines()?.pipelines ?? []}
            fallback={
              <Card>
                <Empty title="No pipelines yet">
                  <span class="sub">Add pipelines to the daemon's pipelines.wcl.</span>
                </Empty>
              </Card>
            }
          >
            {(p) => (
              <Card>
                <button
                  class="service-card-main"
                  onClick={() => setView({ kind: "pipeline", name: p.name })}
                >
                  <span class="service-mark">
                    <Workflow size={18} />
                  </span>
                  <span class="service-card-copy">
                    <strong>{p.name}</strong>
                    <span class="sub">{p.description || "No description"}</span>
                  </span>
                  <span class="service-metric">
                    <strong>{p.steps}</strong>
                    <small>steps</small>
                  </span>
                  <span class="service-metric">
                    <strong>{p.triggers.length}</strong>
                    <small>triggers</small>
                  </span>
                </button>
                <div class="service-card-actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Play}
                    onClick={() => setView({ kind: "pipeline", name: p.name })}
                  >
                    Open
                  </Button>
                </div>
              </Card>
            )}
          </For>
        </div>

        <Show when={(runs()?.runs ?? []).length > 0}>
          <Card title="Recent runs">
            <Table>
              <thead>
                <tr>
                  <th>Pipeline</th>
                  <th>Trigger</th>
                  <th>Started</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                <For each={runs()?.runs ?? []}>
                  {(r) => (
                    <tr
                      class="clickable-row"
                      onClick={() =>
                        setView({ kind: "pipelinerun", id: r.id, pipeline: r.pipeline })
                      }
                    >
                      <td>{r.pipeline}</td>
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
          </Card>
        </Show>
      </Show>
    </>
  );
}
