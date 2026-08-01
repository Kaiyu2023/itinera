export type StopMode = 'candidates' | 'new';

export interface AddStopDeepLink {
  mode: StopMode | null;
  query: string | null;
  pickFirst: boolean;
  candidate: string | null;
}

/** Parse a one-shot add-stop deep link addressed to the given day. */
export function readAddStopDeepLink(params: URLSearchParams, dayId: string): AddStopDeepLink | null {
  if (params.get('gov') !== 'addStop') return null;
  const linkedDayId = params.get('day');
  if (linkedDayId && linkedDayId !== dayId) return null;

  const mode = params.get('mode');
  return {
    mode: mode === 'new' || mode === 'candidates' ? mode : null,
    query: params.get('q'),
    pickFirst: params.get('pick') === 'first',
    candidate: params.get('candidate'),
  };
}

/** Remove consumed add-stop parameters so later manual opens start clean. */
export function stripAddStopDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  for (const key of ['gov', 'mode', 'q', 'pick', 'candidate', 'day']) next.delete(key);
  return next;
}
