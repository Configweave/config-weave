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
import ServicesView, { ServiceView } from "./components/ServicesView";
import SystemRunView from "./components/SystemRunView";
import PackagesView from "./components/PackagesView";
import PackageView from "./components/PackageView";

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
          <Show when={view().kind === "services"}>
            <ServicesView />
          </Show>
          <Show when={asKind("service")} keyed>{(v) => <ServiceView name={v.name} tab={v.tab} />}</Show>
          <Show when={asKind("sysrun")} keyed>
            {(v) => <SystemRunView id={v.id} service={v.service} system={v.system} action={v.action} playbook={v.playbook} play={v.play} />}
          </Show>
          <Show when={view().kind === "packages"}>
            <PackagesView />
          </Show>
          <Show when={asKind("package")} keyed>
            {(v) => <PackageView name={v.name} runbook={v.runbook} />}
          </Show>
        </AppShell>
      </Show>
      <Toaster />
    </Show>
  );
}
