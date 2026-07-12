// The package repository: local packages (--packages-dir) merged with
// the remote repositories' cached clones, each row tagged with its
// source. Remote repos are managed here too — add/edit/remove/sync,
// scheduled + webhook sync config, and Commit & push / Discard for
// repos with local edits — with a visible banner when the repo fails
// validation and a configure hint when no repository is set up.

import { For, Show, createResource, createSignal } from "solid-js";
import { Alert, Badge, Button, Card, Empty, Input, Modal, PageHead, Select, Table, toast } from "@forge/ui";
import { Copy, GitCommitHorizontal, Pencil, Plus, RefreshCw, Trash2, Undo2 } from "lucide-solid";
import type { RepoDef, RepoInput } from "../api";
import { addRepo, listPackages, listRepos, removeRepo, syncAllRepos, syncRepo, updateRepo } from "../api";
import { setView } from "../store";
import { CommitPushModal, discardConfirm } from "./RepoSync";
import { discardRepo } from "../api";

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
  // undefined = closed, null = add, RepoDef = edit.
  const [editing, setEditing] = createSignal<RepoDef | null | undefined>(undefined);
  const [committing, setCommitting] = createSignal<string | null>(null);

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
      toast(e?.message ?? `${label} failed`, {
        // 409 = sync skipped over local changes: guidance, not failure.
        tone: e?.status === 409 ? "warning" : "danger",
      });
      return false;
    } finally {
      setBusy(false);
    }
  };

  const sync = (repoName: string) =>
    run("sync", async () => {
      const res = await syncRepo(repoName);
      toast(
        `synced ${res.name} (${res.packages ?? 0} packages, ${res.runbooks ?? 0} playbooks)`,
        { tone: "success" },
      );
    });

  const syncAll = () =>
    run("sync", async () => {
      const results = await syncAllRepos();
      const failed = results.filter((r) => !r.ok);
      if (failed.length) {
        toast(
          failed.map((f) => `${f.name}: ${f.error ?? f.skipped}`).join("\n"),
          { tone: failed.every((f) => f.skipped) ? "warning" : "danger" },
        );
      } else {
        toast(`synced ${results.length} repositor${results.length === 1 ? "y" : "ies"}`, {
          tone: "success",
        });
      }
    });

  const discard = (repoName: string) => {
    if (!discardConfirm(repoName)) return;
    void run("discard", async () => {
      await discardRepo(repoName);
      toast(`discarded local changes in ${repoName}`, { tone: "success" });
    });
  };

  const remove = (repoName: string) => {
    if (
      !confirm(
        `Remove repository "${repoName}"? Its cached packages and playbooks disappear from ` +
          `the library (copies already added to playbooks are unaffected).`,
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
        <div class="row-actions">
          <Show when={(repos() ?? []).length > 0}>
            <Button size="sm" variant="ghost" icon={RefreshCw} disabled={busy()} onClick={syncAll}>
              Sync all
            </Button>
          </Show>
          <Button size="sm" icon={Plus} disabled={busy()} onClick={() => setEditing(null)}>
            Add repository
          </Button>
        </div>
      }
    >
      <Show when={(repos() ?? []).length > 0}>
        <Table>
          <thead>
            <tr>
              <th>Repository</th>
              <th>Git URL</th>
              <th>Packages</th>
              <th>Playbooks</th>
              <th>Schedule</th>
              <th>State</th>
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
                    <Show when={r.runbooks !== null} fallback={<span class="sub">—</span>}>
                      <Badge tone="info">{r.runbooks}</Badge>
                    </Show>
                  </td>
                  <td class="mono sub">{r.sync_cron ?? ""}</td>
                  <td>
                    <Show
                      when={r.dirty || r.ahead > 0}
                      fallback={<span class="sub">clean</span>}
                    >
                      <Badge tone="warning">
                        {r.dirty
                          ? "local changes"
                          : `${r.ahead} unpushed`}
                      </Badge>
                    </Show>
                  </td>
                  <td>
                    <div class="row-actions">
                      <Show when={r.dirty || r.ahead > 0}>
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={GitCommitHorizontal}
                          disabled={busy()}
                          onClick={() => setCommitting(r.name)}
                        >
                          Commit & push
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          icon={Undo2}
                          disabled={busy()}
                          onClick={() => discard(r.name)}
                        >
                          Discard
                        </Button>
                      </Show>
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
                        icon={Pencil}
                        disabled={busy()}
                        onClick={() => setEditing(r)}
                      >
                        Edit
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={Trash2}
                        disabled={busy()}
                        onClick={() => remove(r.name)}
                      />
                    </div>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </Table>
      </Show>
      <div class="sub" style={{ "margin-top": "8px" }}>
        Repositories can provide packages and playbooks. Their content is editable — edits stay
        local until Commit & push; Sync fast-forwards clean repositories only. Trigger a sync
        manually, on the cron schedule, or from a git-host webhook.
      </div>
      <Show when={editing() !== undefined}>
        <RepoForm
          initial={editing() ?? null}
          onDone={(saved) => {
            setEditing(undefined);
            if (saved) changed();
          }}
        />
      </Show>
      <Show when={committing()} keyed>
        {(repoName) => (
          <CommitPushModal
            repo={repoName}
            onDone={(pushed) => {
              setCommitting(null);
              if (pushed) changed();
            }}
          />
        )}
      </Show>
    </Card>
  );
}

const CRON_PRESETS = [
  { value: "", label: "Manual / webhook only" },
  { value: "0 */15 * * * *", label: "Every 15 minutes" },
  { value: "0 0 * * * *", label: "Every hour" },
  { value: "0 0 2 * * *", label: "Daily at 02:00 UTC" },
  { value: "0 0 2 * * Sun", label: "Weekly Sunday at 02:00 UTC" },
];

function RepoForm(props: { initial: RepoDef | null; onDone: (saved: boolean) => void }) {
  const [def, setDef] = createSignal<RepoInput>(
    props.initial
      ? { ...props.initial }
      : { name: "", url: "", subdir: null, runbooks_subdir: null, branch: null, sync_cron: null, webhook_secret: null },
  );
  const [saving, setSaving] = createSignal(false);
  const set = (patch: Partial<RepoInput>) => setDef({ ...def(), ...patch });
  const opt = (v: string) => v.trim() || null;

  const webhookUrl = () =>
    `${location.origin}/api/webhooks/repos/${encodeURIComponent(def().name || "<name>")}`;

  const save = async () => {
    setSaving(true);
    try {
      const payload = def();
      const res = props.initial
        ? await updateRepo(props.initial.name, payload)
        : await addRepo(payload);
      if (res.error) {
        toast(`saved ${res.name}, but the clone failed: ${res.error}`, { tone: "warning" });
      } else {
        toast(`saved ${res.name}`, { tone: "success" });
      }
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
      size="lg"
      title={props.initial ? `Edit ${props.initial.name}` : "Add repository"}
      onClose={() => props.onDone(false)}
      footer={
        <>
          <Button variant="ghost" onClick={() => props.onDone(false)}>
            Cancel
          </Button>
          <Button disabled={saving() || !def().name.trim() || !def().url.trim()} onClick={save}>
            {props.initial ? "Save" : "Add & clone"}
          </Button>
        </>
      }
    >
      <div class="system-form">
        <div class="form-grid">
          <Input
            label="Name"
            placeholder="e.g. stdlib"
            value={def().name}
            disabled={!!props.initial}
            onInput={(e) => set({ name: e.currentTarget.value })}
          />
          <Input
            label="Branch (optional)"
            value={def().branch ?? ""}
            onInput={(e) => set({ branch: opt(e.currentTarget.value) })}
          />
        </div>
        <Input
          label="Git URL"
          placeholder="https://… or git@…"
          value={def().url}
          onInput={(e) => set({ url: e.currentTarget.value })}
        />
        <div class="form-grid">
          <Input
            label="Packages subdir (optional)"
            placeholder="e.g. pkgs"
            value={def().subdir ?? ""}
            onInput={(e) => set({ subdir: opt(e.currentTarget.value) })}
          />
          <Input
            label="Playbooks subdir (optional)"
            placeholder={'e.g. books, or "." for the repo root'}
            value={def().runbooks_subdir ?? ""}
            onInput={(e) => set({ runbooks_subdir: opt(e.currentTarget.value) })}
          />
        </div>
        <div class="form-grid">
          <Select
            label="Sync schedule"
            options={CRON_PRESETS}
            value={def().sync_cron ?? ""}
            onChange={(cron) => set({ sync_cron: cron || null })}
          />
          <Input
            label="Cron expression (UTC, seconds first)"
            placeholder="unset = manual / webhook only"
            value={def().sync_cron ?? ""}
            onInput={(e) => set({ sync_cron: opt(e.currentTarget.value) })}
          />
        </div>
        <div class="form-grid">
          <Input
            label="Webhook secret (optional, min 8 chars)"
            type="password"
            value={def().webhook_secret ?? ""}
            onInput={(e) => set({ webhook_secret: opt(e.currentTarget.value) })}
          />
          <div style={{ "align-self": "end" }}>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => set({ webhook_secret: crypto.randomUUID() })}
            >
              Generate secret
            </Button>
          </div>
        </div>
        <Show when={def().webhook_secret}>
          <div class="webhook-url">
            <span class="mono sub">{webhookUrl()}</span>
            <Button
              size="sm"
              variant="ghost"
              icon={Copy}
              title="Copy the webhook URL"
              onClick={() =>
                navigator.clipboard
                  .writeText(webhookUrl())
                  .then(() => toast("webhook URL copied", { tone: "success" }))
              }
            />
          </div>
          <div class="sub">
            Point a push webhook here. GitHub/Gitea: secret field (HMAC{" "}
            <span class="mono">X-Hub-Signature-256</span>). GitLab: secret token (
            <span class="mono">X-Gitlab-Token</span>). Manual:{" "}
            <span class="mono">X-Weave-Token</span> header.
          </div>
        </Show>
      </div>
    </Modal>
  );
}
