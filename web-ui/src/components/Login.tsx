import { createSignal } from "solid-js";
import { Alert, Button, Card, Input } from "@forge/ui";
import { api } from "../api";
import { setNeedsLogin } from "../store";

export default function Login() {
  const [user, setUser] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.auth.login(user(), password());
      setNeedsLogin(false);
    } catch (err: any) {
      setError(err?.message ?? "login failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="login-wrap">
      <Card title="config-weave">
        <form onSubmit={submit} class="login-form">
          <Input
            label="Username"
            value={user()}
            onInput={(e) => setUser(e.currentTarget.value)}
          />
          <Input
            label="Password"
            type="password"
            value={password()}
            onInput={(e) => setPassword(e.currentTarget.value)}
          />
          {error() && <Alert tone="danger" title={error()} />}
          <Button variant="primary" type="submit" disabled={busy() || !user()}>
            Sign in
          </Button>
        </form>
      </Card>
    </div>
  );
}
