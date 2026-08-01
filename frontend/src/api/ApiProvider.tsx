import type { ReactNode } from 'react';
import type { ApiClient } from './client';
import { ApiContext } from './ApiContext';

/**
 * The only place the app learns which ApiClient it talks to.
 * main.tsx wires MockApiClient today; the Phase B cutover swaps in
 * HttpApiClient here and nowhere else.
 */
export function ApiProvider({ client, children }: { client: ApiClient; children: ReactNode }) {
  return <ApiContext.Provider value={client}>{children}</ApiContext.Provider>;
}
