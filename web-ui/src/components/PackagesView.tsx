// The package repository (--packages-dir): every package dir the server
// found, with a visible banner when the repo fails validation and a
// configure hint when no repository is set up.

import { For, Show, createResource } from "solid-js";
import { Alert, Badge, Card, Empty, PageHead, Table } from "@forge/ui";
import { listPackages } from "../api";
import { setView } from "../store";

export default function PackagesView() {
  // null = no --packages-dir configured (the endpoint 404s).
  const [repo] = createResource(() =>
    listPackages().catch((e: any) => {
      if (e?.status === 404) return null;
      throw e;
    }),
  );

  return (
    <>
      <PageHead title="Packages" sub="The server's package repository" />
      <Show when={repo() === null}>
        <Card>
          <Empty title="No package repository configured">
            <span class="sub">
              Start weave-server with <span class="mono">--packages-dir</span>{" "}
              pointing at a folder of package directories (each containing a{" "}
              <span class="mono">package.wcl</span>).
            </span>
          </Empty>
        </Card>
      </Show>
      <Show when={repo()?.error}>
        <Alert tone="danger" title="Repository failed validation">
          <pre class="log-chunk">{repo()!.error}</pre>
        </Alert>
      </Show>
      <Show when={repo()}>
        <Card>
          <Show
            when={(repo()?.packages ?? []).length > 0}
            fallback={
              <Empty
                title={
                  repo.loading ? "Loading…" : "No packages in the repository"
                }
              />
            }
          >
            <Table>
              <thead>
                <tr>
                  <th>Package</th>
                  <th>Description</th>
                  <th>Resources</th>
                  <th>Tests</th>
                </tr>
              </thead>
              <tbody>
                <For each={repo()?.packages ?? []}>
                  {(p) => (
                    <tr
                      class="clickable-row"
                      onClick={() => setView({ kind: "package", name: p.name })}
                    >
                      <td class="mono">{p.name}</td>
                      <td class="sub">{p.description}</td>
                      <td>
                        <Badge tone="neutral">
                          {(p.resources ?? []).length}
                        </Badge>
                      </td>
                      <td>
                        <Badge tone={p.tests.length ? "info" : "neutral"}>
                          {p.tests.length}
                        </Badge>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </Table>
          </Show>
        </Card>
      </Show>
    </>
  );
}
