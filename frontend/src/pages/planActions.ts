import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { Day, Stop } from '../api/types';

/** Governance surface currently owned by the Plan tab. */
export type GovAction =
  | { kind: 'discuss'; stop: Stop }
  | { kind: 'change'; stop: Stop }
  | {
      kind: 'addStop';
      day: Day;
      initialSlot?: string;
      initialCandidateId?: string;
      allowDaySelection?: boolean;
    };

export interface PlanActions {
  discuss: (stop: Stop) => void;
  proposeChange: (stop: Stop) => void;
  proposeStop: (day: Day, initialSlot?: string) => void;
}

export const PlanActionsContext = createContext<PlanActions | null>(null);

const NOOP_ACTIONS: PlanActions = {
  discuss: () => {},
  proposeChange: () => {},
  proposeStop: () => {},
};

/** Actions exposed to deeply nested stop cards, popovers, and sheets. */
export function usePlanActions(): PlanActions {
  return useContext(PlanActionsContext) ?? NOOP_ACTIONS;
}

export interface GovState {
  action: GovAction | null;
  actions: PlanActions;
  close: () => void;
}

/** Owns the single active governance surface for a Plan tab. */
export function usePlanActionsState(): GovState {
  const [action, setAction] = useState<GovAction | null>(null);
  const close = useCallback(() => setAction(null), []);
  const actions = useMemo<PlanActions>(
    () => ({
      discuss: (stop) => setAction({ kind: 'discuss', stop }),
      proposeChange: (stop) => setAction({ kind: 'change', stop }),
      proposeStop: (day, initialSlot) => setAction({ kind: 'addStop', day, initialSlot }),
    }),
    [],
  );

  return useMemo(() => ({ action, actions, close }), [action, actions, close]);
}
