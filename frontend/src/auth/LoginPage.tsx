import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card } from "@/components/ui/card";
import { useAuthConfig, useLogin } from "@/api/hooks";

export function LoginPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const login = useLogin();
  const cfg = useAuthConfig();

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try { await login.mutateAsync({ email, password }); }
    catch { setError("Invalid email or password"); }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm p-6">
        <h1 className="mb-4 text-xl font-semibold">Sign in to Egret</h1>
        <form onSubmit={onSubmit} className="space-y-3">
          <Input type="email" placeholder="Email" value={email} onChange={(e) => setEmail(e.target.value)} required />
          <Input type="password" placeholder="Password" value={password} onChange={(e) => setPassword(e.target.value)} required />
          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button type="submit" className="w-full" disabled={login.isPending}>Sign in</Button>
        </form>
        {cfg.data?.oidc_enabled && (
          <a href="/api/auth/oidc/login" className="mt-3 block">
            <Button variant="outline" className="w-full">Sign in with SSO</Button>
          </a>
        )}
      </Card>
    </div>
  );
}
