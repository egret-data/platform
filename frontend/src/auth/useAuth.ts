import { createContext, useContext } from "react";
import type { Me } from "@/api/hooks";

export interface AuthState { me: Me | null; isLoading: boolean; }
export const AuthContext = createContext<AuthState>({ me: null, isLoading: true });
export const useAuth = () => useContext(AuthContext);
