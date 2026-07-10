import { Show, onMount } from "solid-js";
import { AppShell, Empty, Toaster } from "@forge/ui";
import type { View } from "./store";
import { init, needsLogin, ready, view } from "./store";

// Keyed <Show> narrows on the view object itself, so navigating between
// two views of the same kind (run → run) recreates the component.
function asKind<K extends View["kind"]>(kind: K) {
  const v = view();
  return v.kind === kind ? (v as Extract<View, { kind: K }>) : undefined;
}
import Login from "./components/Login";
import Topbar from "./components/Topbar";
import Sidebar from "./components/Sidebar";
import RunbooksView from "./components/RunbooksView";
import RunbookView from "./components/RunbookView";
import RunView from "./components/RunView";
import SystemsView from "./components/SystemsView";
import SystemRunView from "./components/SystemRunView";

export default function App() {
  onMount(init);
  return (
    <Show when={ready()} fallback={<Empty title="loading…" />}>
      <Show when={!needsLogin()} fallback={<Login />}>
        <AppShell topbar={<Topbar />} sidebar={<Sidebar />}>
          <Show when={view().kind === "runbooks"}>
            <RunbooksView />
          </Show>
          <Show when={asKind("runbook")} keyed>
            {(v) => <RunbookView name={v.name} />}
          </Show>
          <Show when={asKind("run")} keyed>
            {(v) => <RunView id={v.id} runbook={v.runbook} />}
          </Show>
          <Show when={view().kind === "systems"}>
            <SystemsView />
          </Show>
          <Show when={asKind("sysrun")} keyed>
            {(v) => <SystemRunView id={v.id} system={v.system} action={v.action} />}
          </Show>
        </AppShell>
      </Show>
      <Toaster />
    </Show>
  );
}
