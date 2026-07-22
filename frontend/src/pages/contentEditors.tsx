import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import { KIND_COLOR } from './planShared';
import type { Day, Stop } from '../api/types';

/**
 * Inline content editors for the timeline (§3.3). Content edits — a stop's
 * notes / arrival / duration, a day's city label / window — apply immediately
 * with no approval; only structural moves go through proposals. These edit
 * exactly the fields `StopPatch` / `DayPatch` carry (never the stop's day or
 * order, which stay governance-gated) and write straight through
 * `updateStop` / `updateDay`, invalidating the plan query on success.
 */

/** Minutes → a compact "1h 30m" for the duration hint. */
function durationHint(min: number): string {
  if (!min || min < 0) return '—';
  const h = Math.floor(min / 60);
  const m = min % 60;
  return h ? `${h}h${m ? ` ${m}m` : ''}` : `${m}m`;
}

export function StopEditor({
  stop,
  placeName,
  tripId,
  onClose,
}: {
  stop: Stop;
  placeName: string;
  tripId: string;
  onClose: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();

  const [arrival, setArrival] = useState(stop.plannedArrival);
  const [durationStr, setDurationStr] = useState(String(stop.durationMin));
  const [notes, setNotes] = useState(stop.notes);

  const durationMin = durationStr.trim() === '' ? 0 : Math.max(0, Math.round(Number(durationStr) || 0));
  const canSave = !!arrival && durationMin > 0;

  const save = useMutation({
    mutationFn: () =>
      api.updateStop(stop.id, {
        plannedArrival: arrival,
        durationMin,
        notes: notes.trim(),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['plan', tripId] });
      onClose();
    },
  });

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={`Edit ${placeName}`}>
        <div className="mtop">
          <span className="mtop-ic" style={{ background: KIND_COLOR[stop.stopKind] }}>✎</span>
          <strong>Edit details · {placeName}</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="stop-arr">Arrival</label>
            <span className="fv">
              <input id="stop-arr" type="time" className="tinp time" value={arrival} onChange={(e) => setArrival(e.target.value)} />
            </span>
          </div>
          <div className="frow">
            <label className="fl" htmlFor="stop-dur">Duration</label>
            <span className="fv">
              <input id="stop-dur" type="number" min={0} step={5} className="tinp num" value={durationStr} onChange={(e) => setDurationStr(e.target.value)} />
              <span className="hint">minutes · {durationHint(durationMin)}</span>
            </span>
          </div>
          <div className="frow" style={{ alignItems: 'start' }}>
            <label className="fl" htmlFor="stop-notes">Notes</label>
            <span className="fv">
              <textarea id="stop-notes" className="tinp" rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} placeholder="Anything the group should know about this stop" />
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">Content edits apply immediately — no approval. Moving or removing the stop is a proposal.</span>
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="button" className="btn accent" disabled={!canSave || save.isPending} onClick={() => save.mutate()}>Save changes</button>
        </div>
      </div>
    </SheetModal>
  );
}

export function DayEditor({
  day,
  dayIndex,
  tripId,
  onClose,
}: {
  day: Day;
  dayIndex: number;
  tripId: string;
  onClose: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();

  const [cityHint, setCityHint] = useState(day.cityHint);
  const [windowStart, setWindowStart] = useState(day.windowStart);
  const [windowEnd, setWindowEnd] = useState(day.windowEnd);

  const windowOk = !!windowStart && !!windowEnd && windowEnd > windowStart;
  const canSave = cityHint.trim().length > 0 && windowOk;

  const save = useMutation({
    mutationFn: () =>
      api.updateDay(day.id, {
        cityHint: cityHint.trim(),
        windowStart,
        windowEnd,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['plan', tripId] });
      onClose();
    },
  });

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={`Edit Day ${dayIndex + 1}`}>
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--accent)' }}>✎</span>
          <strong>Edit Day {dayIndex + 1}</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="day-city">City</label>
            <span className="fv">
              <input id="day-city" className="tinp" value={cityHint} onChange={(e) => setCityHint(e.target.value)} placeholder="Where the day is based" />
            </span>
          </div>
          <div className="frow">
            <label className="fl" htmlFor="day-start">Window</label>
            <span className="fv">
              <input id="day-start" type="time" className="tinp time" value={windowStart} onChange={(e) => setWindowStart(e.target.value)} aria-label="Window start" />
              <span className="muted">→</span>
              <input type="time" className="tinp time" value={windowEnd} onChange={(e) => setWindowEnd(e.target.value)} aria-label="Window end" />
            </span>
          </div>
          {windowStart && windowEnd && windowEnd <= windowStart && (
            <div className="frow">
              <span className="fl" />
              <span className="fv"><span className="hint bad">⚠ The window's end must be after its start.</span></span>
            </div>
          )}
        </div>
        <div className="exp-foot">
          <span className="hint grow">The day's window is the feasibility budget. Content edits apply immediately.</span>
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="button" className="btn accent" disabled={!canSave || save.isPending} onClick={() => save.mutate()}>Save changes</button>
        </div>
      </div>
    </SheetModal>
  );
}
