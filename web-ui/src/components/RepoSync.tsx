// Shared write-back UI for remote repositories: a commit-message modal
// (Commit & push) and a status bar the runbook/package editors mount
// when their content comes from a repo — it polls the single-repo
// endpoint and surfaces dirty/unpushed state with the settle actions.

import { Show, createResource, createSignal, onCleanup } from "solid-js";
import { Badge, Button, Modal, Textarea, toast } from "@forge/ui";
import { GitCommitHorizontal, Undo2 } from "lucide-solid";
import type { RepoDef } from "../api";
import { commitRepo, discardRepo, getRepo } from "../api";

export function CommitPushModal(props: {
  repo: string;
  onDone: (pushed: boolean) => void;
}) {
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const commit = async () => {
    setBusy(true);
    try {
      await commitRepo(props.repo, message().trim());
      toast(`pushed to ${props.repo}`, { tone: "success" });
      props.onDone(true);
    } catch (e: any) {
      // 409 = the remote moved; the server message says how to settle.
      toast(e?.message ?? "commit failed", { tone: e?.status === 409 ? "warning" : "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      title={`Commit & push to ${props.repo}`}
      onClose={() => props.onDone(false)}
      footer={
        <>
          <Button variant="ghost" onClick={() => props.onDone(false)}>
            Cancel
          </Button>
          <Button disabled={busy() || !message().trim()} onClick={commit}>
            Commit & push
          </Button>
        </>
      }
    >
      <div class="system-form">
        <Textarea
          label="Commit message"
          rows={3}
          placeholder="what changed and why"
          value={message()}
          onInput={(e) => setMessage(e.currentTarget.value)}
        />
        <div class="sub">
          Commits every local edit in the repository's cache and pushes to origin.
        </div>
      </div>
    </Modal>
  );
}

export function discardConfirm(repo: string): boolean {
  return confirm(
    `Discard ALL local edits and unpushed commits in "${repo}"? ` +
      `The cache resets to the remote's latest state.`,
  );
}

/// Mounted by RunbookView/PackageView for repo-sourced content: shows
/// the repo's dirty/unpushed state and the Commit & push / Discard
/// actions. Polls so edits saved in the workspace below surface here.
export function RepoWriteBar(props: { repo: string; onSettled?: () => void }) {
  const [repo, { refetch }] = createResource(() => props.repo, getRepo);
  const [committing, setCommitting] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const timer = setInterval(() => void refetch(), 2500);
  onCleanup(() => clearInterval(timer));

  const pending = (r: RepoDef | undefined) => !!r && (r.dirty || r.ahead > 0);

  const discard = async () => {
    if (!discardConfirm(props.repo)) return;
    setBusy(true);
    try {
      await discardRepo(props.repo);
      toast(`discarded local changes in ${props.repo}`, { tone: "success" });
      void refetch();
      props.onSettled?.();
    } catch (e: any) {
      toast(e?.message ?? "discard failed", { tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show when={repo()} keyed>
      {(r) => (
        <div class="repo-write-bar">
          <Badge tone="info">from {r.name}</Badge>
          <Show
            when={pending(r)}
            fallback={<span class="sub">in sync with the remote</span>}
          >
            <Badge tone="warning">
              {r.dirty ? "changes not pushed" : `${r.ahead} unpushed commit${r.ahead === 1 ? "" : "s"}`}
            </Badge>
            <Button
              size="sm"
              icon={GitCommitHorizontal}
              disabled={busy()}
              onClick={() => setCommitting(true)}
            >
              Commit & push
            </Button>
            <Button size="sm" variant="ghost" icon={Undo2} disabled={busy()} onClick={discard}>
              Discard
            </Button>
          </Show>
          <Show when={committing()}>
            <CommitPushModal
              repo={props.repo}
              onDone={(pushed) => {
                setCommitting(false);
                if (pushed) {
                  void refetch();
                  props.onSettled?.();
                }
              }}
            />
          </Show>
        </div>
      )}
    </Show>
  );
}
