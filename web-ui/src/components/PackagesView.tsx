// The package repository: local packages (--packages-dir) merged with
// the remote repositories' cached clones, each row tagged with its
// source. Remote repos are managed here too — add/remove/sync — with a
// visible banner when the repo fails validation and a configure hint
// when no repository is set up.

import { For, Show, createResource, createSignal } from "solid-js";
import { Alert, Badge, Button, Card, Empty, Input, PageHead, Table, toast } from "@forge/ui";
import { Plus, RefreshCw, Trash2 } from "lucide-solid";
import type { RepoDef } from "../api";
import { addRepo, listPackages, listRepos, removeRepo, syncAllRepos, syncRepo } from "../api";
import { setView } from "../store";

export default function PackagesView() {
  // null = no --packages-dir configured (the endpoint 404s).
  const [repo, { refetch: refetchPackages }] = createResource(() =>
    listPackages().catch((e: any) => {
      if (e?.status === 404) return null;
      throw e;
    }),
  );

  return (
    <>
      <PageHead title="Packages" sub="Reusable resources, gatherers, and tests in the library" />
      <RepositoriesCard onChanged={() => void refetchPackages()} />
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
      <Show when={(repo()?.shadowed ?? []).length > 0}>
        <Alert tone="warning" title="Shadowed packages">
          <For each={repo()!.shadowed!}>
            {(s) => (
              <div class="sub">
                <span class="mono">{s.name}</span> from <span class="mono">{s.source}</span> is
                hidden by the copy in <span class="mono">{s.by}</span>.
              </div>
            )}
          </For>
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
                  <th>Source</th>
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
                        <Badge tone={p.source && p.source !== "local" ? "info" : "neutral"}>
                          {p.source ?? "local"}
                        </Badge>
                      </td>
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

function RepositoriesCard(props: { onChanged: () => void }) {
  const [repos, { refetch }] = createResource(listRepos);
  const [busy, setBusy] = createSignal(false);
  const [name, setName] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [subdir, setSubdir] = createSignal("");
  const [branch, setBranch] = createSignal("");

  const changed = () => {
    void refetch();
    props.onChanged();
  };

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await action();
      changed();
      return true;
    } catch (e: any) {
      toast(e?.message ?? `${label} failed`, { tone: "danger" });
      return false;
    } finally {
      setBusy(false);
    }
  };

  const add = () =>
    run("add", async () => {
      const res = await addRepo({
        name: name().trim(),
        url: url().trim(),
        subdir: subdir().trim() || undefined,
        branch: branch().trim() || undefined,
      });
      if (res.error) {
        toast(`added ${res.name}, but the clone failed: ${res.error}`, { tone: "warning" });
      } else {
        toast(`added ${res.name} (${res.packages ?? 0} packages)`, { tone: "success" });
      }
      setName("");
      setUrl("");
      setSubdir("");
      setBranch("");
    });

  const sync = (repoName: string) =>
    run("sync", async () => {
      const res = await syncRepo(repoName);
      toast(`synced ${res.name} (${res.packages ?? 0} packages)`, { tone: "success" });
    });

  const syncAll = () =>
    run("sync", async () => {
      const results = await syncAllRepos();
      const failed = results.filter((r) => !r.ok);
      if (failed.length) {
        toast(failed.map((f) => `${f.name}: ${f.error}`).join("\n"), { tone: "danger" });
      } else {
        toast(`synced ${results.length} repositor${results.length === 1 ? "y" : "ies"}`, {
          tone: "success",
        });
      }
    });

  const remove = (repoName: string) => {
    if (
      !confirm(
        `Remove repository "${repoName}"? Its cached packages disappear from the library ` +
          `(copies already added to playbooks are unaffected).`,
      )
    )
      return;
    void run("remove", async () => {
      await removeRepo(repoName);
      toast(`removed ${repoName}`, { tone: "success" });
    });
  };

  return (
    <Card
      title="Remote repositories"
      action={
        <Show when={(repos() ?? []).length > 0}>
          <Button size="sm" variant="ghost" icon={RefreshCw} disabled={busy()} onClick={syncAll}>
            Sync all
          </Button>
        </Show>
      }
    >
      <Show when={(repos() ?? []).length > 0}>
        <Table>
          <thead>
            <tr>
              <th>Repository</th>
              <th>Git URL</th>
              <th>Packages</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <For each={repos() ?? []}>
              {(r: RepoDef) => (
                <tr>
                  <td class="mono">
                    {r.name}
                    <Show when={r.branch}>
                      <span class="sub"> @ {r.branch}</span>
                    </Show>
                  </td>
                  <td class="mono sub">
                    {r.url}
                    <Show when={r.subdir}>
                      <span> ({r.subdir}/)</span>
                    </Show>
                  </td>
                  <td>
                    <Show when={r.cloned} fallback={<Badge tone="warning">not cloned</Badge>}>
                      <Badge tone="info">{r.packages ?? 0}</Badge>
                    </Show>
                  </td>
                  <td>
                    <div class="row-actions">
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={RefreshCw}
                        disabled={busy()}
                        onClick={() => void sync(r.name)}
                      >
                        Sync
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={Trash2}
                        disabled={busy()}
                        onClick={() => remove(r.name)}
                      >
                        Remove
                      </Button>
                    </div>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </Table>
      </Show>
      <div class="repo-add-form">
        <Input
          placeholder="name (e.g. stdlib)"
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <Input
          placeholder="git URL"
          value={url()}
          onInput={(e) => setUrl(e.currentTarget.value)}
        />
        <Input
          placeholder="subdir (optional)"
          value={subdir()}
          onInput={(e) => setSubdir(e.currentTarget.value)}
        />
        <Input
          placeholder="branch (optional)"
          value={branch()}
          onInput={(e) => setBranch(e.currentTarget.value)}
        />
        <Button
          icon={Plus}
          disabled={busy() || !name().trim() || !url().trim()}
          onClick={() => void add()}
        >
          Add
        </Button>
      </div>
      <div class="sub" style={{ "margin-top": "8px" }}>
        Remote packages are read-only in the library; sync pulls the latest, and adding to a
        playbook copies them like local packages.
      </div>
    </Card>
  );
}
