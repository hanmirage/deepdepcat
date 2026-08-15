/**
 * useConnectors — loads connectors from backend and provides
 * connect/disconnect operations.
 *
 * Components call this hook; they never touch connectorApi directly.
 */

import { useState, useEffect, useCallback } from "react";
import { connectorApi } from "@/lib/tauri";
import type { Connector } from "@/types";

export interface UseConnectorsResult {
  connectors: Connector[];
  loading: boolean;
  connecting: string | null;
  connect: (connectorId: string) => Promise<void>;
}

export function useConnectors(): UseConnectorsResult {
  const [connectors, setConnectors] = useState<Connector[]>([]);
  const [connecting, setConnecting] = useState<string | null>(null);

  useEffect(() => {
    const load = async () => {
      try {
        const data = await connectorApi.getConnectors();
        setConnectors(data);
      } catch {
        // Silently ignore — connectors feature is optional
      }
    };
    load();
  }, []);

  const connect = useCallback(async (connectorId: string) => {
    setConnecting(connectorId);
    try {
      await connectorApi.connect(connectorId);
      setConnectors((prev) =>
        prev.map((c) =>
          c.id === connectorId ? { ...c, status: "connected" as const } : c,
        ),
      );
    } catch {
      // Ignore — UI stays in disconnected state
    } finally {
      setConnecting(null);
    }
  }, []);

  return {
    connectors,
    loading: false,
    connecting,
    connect,
  };
}
