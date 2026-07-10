// The package repository (--packages-dir): every package dir the server
// found, with a visible banner when the repo fails validation.

import { For, Show, createResource } from "solid-js";
import { Alert, Badge, Card, Empty, PageHead, Table } from "@forge/ui";
import { listPackages } from "../api";
import { setView } from "../store";

export default function PackagesView() {
  const [repo] = createResource(listPackages);

  return (
    <>
      <PageHead title="Packages" sub="The server's package repository" />
      <Show when={repo()?.error}>
        <Alert tone="danger" title="Repository failed validation">
          <pre class="log-chunk">{repo()!.error}</pre>
        </Alert>
      </Show>
      <Card>
        <Show
          when={(repo()?.packages ?? []).length > 0}
          fallback={<Empty title={repo.loading ? "Loading…" : "No packages in the repository"} />}
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
                      <Badge tone="neutral">{(p.resources ?? []).length}</Badge>
                    </td>
                    <td>
                      <Badge tone={p.tests.length ? "info" : "neutral"}>{p.tests.length}</Badge>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </Table>
        </Show>
      </Card>
    </>
  );
}
