// Hosts seen in traffic and their scope state (#39), polled from spall.
// One poller for the whole app: Traffic marks rows, Engagement lists them.

import { useCallback, useEffect, useState } from "react";
import { apiJson } from "./lib/engagement.ts";
import { asListing, type HostListing } from "./lib/hosts.ts";

export function useHosts(enabled: boolean, nonce: number, intervalMs = 5000): { listing: HostListing | null; refresh(): void } {
  const [listing, setListing] = useState<HostListing | null>(null);
  const [tick, setTick] = useState(0);
  const refresh = useCallback(() => setTick((n) => n + 1), []);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    const load = async () => {
      const body = await apiJson("/api/hosts");
      if (live && typeof body !== "string") setListing(asListing(body));
    };
    void load();
    const id = setInterval(load, intervalMs);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [enabled, intervalMs, nonce, tick]);
  return { listing: enabled ? listing : null, refresh };
}
