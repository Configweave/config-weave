import { For, Show, createResource } from "solid-js";
import { Button, Card, Empty, PageHead, Table } from "@forge/ui";
import { listRunbooks } from "../api";
import { setView } from "../store";

export default function RunbooksView() {
  const [runbooks] = createResource(listRunbooks);
  return (
    <>
      <PageHead
        title="Runbooks"
        sub="Every child directory of the server root with a playbook.wcl"
      />
      <Card>
        <Show
          when={(runbooks() ?? []).length > 0}
          fallback={
            <Empty title="No runbooks">
              Point weave-server --dir at a folder of playbook directories.
            </Empty>
          }
        >
          <Table>
            <thead>
              <tr>
                <th>Name</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For each={runbooks() ?? []}>
                {(rb) => (
                  <tr>
                    <td>{rb.name}</td>
                    <td style={{ "text-align": "right" }}>
                      <Button
                        size="sm"
                        onClick={() => setView({ kind: "runbook", name: rb.name })}
                      >
                        Open
                      </Button>
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
