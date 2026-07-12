import { For, Show, createResource, createSignal } from "solid-js";
import { Alert, Badge, Button, Card, Empty, PageHead, Table, toast } from "@forge/ui";
import { FileUp } from "lucide-solid";
import { listRunbooks, uploadRunbookZip } from "../api";
import { setView } from "../store";

export default function RunbooksView() {
  const [listing, { refetch }] = createResource(listRunbooks);
  const runbooks = () => listing()?.runbooks;
  const shadowed = () => listing()?.shadowed ?? [];
  const [uploading, setUploading] = createSignal(false);
  let fileInput!: HTMLInputElement;

  const upload = async (file: File, name?: string) => {
    setUploading(true);
    try {
      const res = await uploadRunbookZip(file, name);
      toast(`uploaded ${res.name}`, { tone: "success" });
      void refetch();
      setView({ kind: "runbook", name: res.name });
    } catch (e: any) {
      // A name conflict (409) or a root-level archive with no folder to
      // name it after (400) both resolve with an explicit name.
      if (e?.status === 409 || (e?.status === 400 && e?.message?.includes("?name="))) {
        const suggestion = file.name.replace(/\.zip$/i, "");
        const picked = prompt(`${e.message}\n\nUpload as:`, name ?? suggestion);
        if (picked) {
          setUploading(false);
          return upload(file, picked.trim());
        }
      } else {
        toast(e?.message ?? "upload failed", { tone: "danger" });
      }
    } finally {
      setUploading(false);
    }
  };

  const pick = (e: Event) => {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (file) void upload(file);
  };

  return (
    <>
      <PageHead
        title="Playbooks"
        sub="Reusable configuration plans available to services"
        actions={
          <>
            <input
              ref={fileInput}
              type="file"
              accept=".zip,application/zip"
              style={{ display: "none" }}
              onChange={pick}
            />
            <Button
              size="sm"
              icon={FileUp}
              disabled={uploading()}
              title="Create a playbook from a zip (as produced by Download)"
              onClick={() => fileInput.click()}
            >
              {uploading() ? "Uploading…" : "Upload zip"}
            </Button>
          </>
        }
      />
      <Show when={shadowed().length > 0}>
        <Alert tone="warning" title="Shadowed playbooks">
          <For each={shadowed()}>
            {(s) => (
              <div class="sub">
                <span class="mono">{s.name}</span> from <span class="mono">{s.source}</span> is
                hidden by the copy in <span class="mono">{s.by}</span>.
              </div>
            )}
          </For>
        </Alert>
      </Show>
      <Card>
        <Show
          when={(runbooks() ?? []).length > 0}
          fallback={
            <Empty title="No playbooks">
              Point weave-server --dir at a folder of playbook directories, or add a remote
              repository with a playbooks subdir.
            </Empty>
          }
        >
          <Table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Source</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For each={runbooks() ?? []}>
                {(rb) => (
                  <tr>
                    <td>{rb.name}</td>
                    <td>
                      <Badge tone={rb.source !== "local" ? "info" : "neutral"}>{rb.source}</Badge>
                    </td>
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
