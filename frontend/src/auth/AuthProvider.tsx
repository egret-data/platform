import type { ReactNode } from "react";
import { useMe } from "@/api/hooks";
import { AuthContext } from "./useAuth";

export function AuthProvider({ children }: { children: ReactNode }) {
  const { data, isLoading } = useMe();
  return (
    <AuthContext.Provider value={{ me: data ?? null, isLoading }}>
      {children}
    </AuthContext.Provider>
  );
}
