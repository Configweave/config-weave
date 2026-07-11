// App-wide state: the current view (signal-based routing, no router lib)
// and the session gate. Views: runbook list → runbook (tree + editor +
// tests) → run (live progress + attach).

import { createSignal } from "solid-js";
import { api } from "./api";

export type View =
  | { kind: "runbooks" }
  | { kind: "runbook"; name: string }
  | { kind: "run"; id: string; runbook: string }
  | { kind: "services" }
  | { kind: "service"; name: string; tab?: "overview" | "systems" | "schedules" | "monitoring" | "logs" }
  | { kind: "sysrun"; id: string; service: string; system: string; action: string; playbook: string; play: string }
  | { kind: "packages" }
  | { kind: "package"; name: string; runbook?: string };

export const [view, setView] = createSignal<View>({ kind: "services" });
export const [servicesRevision, setServicesRevision] = createSignal(1);
export const notifyServicesChanged = () => setServicesRevision((n) => n + 1);
export const [ready, setReady] = createSignal(false);
export const [needsLogin, setNeedsLogin] = createSignal(false);

export async function init() {
  try {
    const health = await api.health();
    setNeedsLogin(health.auth_enabled && !api.auth.token());
  } catch {
    // Server unreachable: surface the login-free shell; requests will fail
    // visibly in place.
  }
  api.onUnauthorized(() => setNeedsLogin(true));
  setReady(true);
}
