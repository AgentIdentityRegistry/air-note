import { createContext, useContext, useEffect, useState, ReactNode } from "react";
import { IdentityMetadata, getIdentity, getTrustScore } from "../api/tauri";

type IdentityState = {
  identity: IdentityMetadata | null;
  trustScore: number | null;
  loading: boolean;
  refresh: () => Promise<void>;
};

const Ctx = createContext<IdentityState | null>(null);

export function IdentityProvider({ children }: { children: ReactNode }) {
  const [identity, setIdentity] = useState<IdentityMetadata | null>(null);
  const [trustScore, setTrustScore] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = async () => {
    setLoading(true);
    const id = await getIdentity();
    setIdentity(id);
    if (id) {
      const score = await getTrustScore();
      setTrustScore(score);
    } else {
      setTrustScore(null);
    }
    setLoading(false);
  };

  useEffect(() => {
    refresh();
  }, []);

  return (
    <Ctx.Provider value={{ identity, trustScore, loading, refresh }}>
      {children}
    </Ctx.Provider>
  );
}

export function useIdentity() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useIdentity must be inside IdentityProvider");
  return c;
}
