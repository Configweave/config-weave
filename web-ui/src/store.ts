// App-wide state: the current view (signal-based routing, no router lib)
// and the session gate. Views: runbook list → runbook (tree + editor +
// tests) → run (live progress + attach).

import { createSignal } from "solid-js";
import { api } from "./api";

export type View =
  | { kind: "runbooks" }
  | { kind: "runbook"; name: string }
  | { kind: "run"; id: string; runbook: string }
  | { kind: "systems" }
  | { kind: "sysrun"; id: string; system: string; action: string }
  | { kind: "packages" }
  | { kind: "package"; name: string; runbook?: string };

export const [view, setView] = createSignal<View>({ kind: "runbooks" });
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
