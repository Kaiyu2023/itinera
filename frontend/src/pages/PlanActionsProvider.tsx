import type { ReactNode } from 'react';
import { PlanActionsContext } from './planActions';
import type { PlanActions } from './planActions';

/** Provides Plan governance actions to nested stop controls. */
export function PlanActionsProvider({ actions, children }: { actions: PlanActions; children: ReactNode }) {
  return <PlanActionsContext.Provider value={actions}>{children}</PlanActionsContext.Provider>;
}
