import { useContext } from 'react';
import { ApiContext } from './ApiContext';
import type { ApiClient } from './client';

export function useApi(): ApiClient {
  const client = useContext(ApiContext);
  if (!client) throw new Error('useApi must be used inside <ApiProvider>');
  return client;
}
