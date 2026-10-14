import { useState } from "react";
import { api, ApiError } from "../api";
import { Wordmark } from "../components/ui";

export function Login({ onSignedIn }: { onSignedIn: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await api.login(password);
      onSignedIn();
    } catch (e) {
      const status = e instanceof ApiError ? e.status : 0;
      setError(status === 401 ? "Wrong password." : status === 429 ? "Too many tries. Wait 10 minutes." : (e as Error).message);
      setPassword("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="center-page">
      <form
        className="login"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Wordmark />
        <p className="login-host t-muted">{location.host}</p>
        <label className="field">
          <span>Password</span>
          <input
            type="password"
            autoFocus
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            aria-invalid={!!error}
          />
        </label>
        <button type="submit" className="btn btn-primary btn-block" disabled={busy || !password}>
          {busy ? "Signing in" : "Sign in"}
        </button>
        {error && (
          <p className="error-line" role="alert">
            {error}
          </p>
        )}
      </form>
    </div>
  );
}
