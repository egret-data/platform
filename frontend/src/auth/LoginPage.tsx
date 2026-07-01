import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card } from "@/components/ui/card";
import { useAuthConfig, useLogin } from "@/api/hooks";
import { ApiError } from "@/api/client";

export function LoginPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [showLocal, setShowLocal] = useState(false);
  const login = useLogin();
  const cfg = useAuthConfig();
  const navigate = useNavigate();

  const oidcEnabled = !!cfg.data?.oidc_enabled;
  const localVisible = !oidcEnabled || showLocal;

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await login.mutateAsync({ email, password });
      navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError && err.status === 429) {
        setError("Too many attempts. Please wait a few minutes and try again.");
      } else {
        setError("Invalid email or password");
      }
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm p-6">
        <h1 className="mb-4 text-xl font-semibold">Sign in to Egret</h1>
        {oidcEnabled && (
          <a href="/api/auth/oidc/login" className="block">
            <Button className="w-full">Sign in with SSO</Button>
          </a>
        )}
        {localVisible && (
          <form onSubmit={onSubmit} className={`space-y-3 ${oidcEnabled ? "mt-4" : ""}`}>
            <Input type="email" placeholder="Email" value={email} onChange={(e) => setEmail(e.target.value)} required />
            <Input type="password" placeholder="Password" value={password} onChange={(e) => setPassword(e.target.value)} required />
            {error && <p className="text-sm text-destructive">{error}</p>}
            <Button type="submit" variant={oidcEnabled ? "outline" : "default"} className="w-full" disabled={login.isPending}>Sign in</Button>
          </form>
        )}
        {oidcEnabled && !showLocal && (
          <button
            type="button"
            onClick={() => setShowLocal(true)}
            className="mt-4 block w-full text-center text-xs text-muted-foreground hover:text-foreground"
          >
            Admin sign-in (break-glass)
          </button>
        )}
      </Card>
    </div>
  );
}
