import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/useApi';
import { SheetModal } from '../components/SheetModal';
import { useI18n } from '../i18n';
import { formatPlanDuration } from '../i18n/messages.plan';
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
  const { t } = useI18n();
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
      <div
        className="exp-modal"
        role="dialog"
        aria-modal="true"
        aria-label={t('plan.editor.stopLabel', { place: placeName })}
      >
        <div className="mtop">
          <span className="mtop-ic" style={{ background: KIND_COLOR[stop.stopKind] }}>
            ✎
          </span>
          <strong>{t('plan.editor.stopTitle', { place: placeName })}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={t('plan.editor.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="stop-arr">
              {t('plan.editor.arrival')}
            </label>
            <span className="fv">
              <input
                id="stop-arr"
                type="time"
                className="tinp time"
                value={arrival}
                onChange={(e) => setArrival(e.target.value)}
              />
            </span>
          </div>
          <div className="frow">
            <label className="fl" htmlFor="stop-dur">
              {t('plan.editor.duration')}
            </label>
            <span className="fv">
              <input
                id="stop-dur"
                type="number"
                min={0}
                step={5}
                className="tinp num"
                value={durationStr}
                onChange={(e) => setDurationStr(e.target.value)}
              />
              <span className="hint">
                {t('plan.editor.minutesHint', { duration: formatPlanDuration(durationMin, t) })}
              </span>
            </span>
          </div>
          <div className="frow" style={{ alignItems: 'start' }}>
            <label className="fl" htmlFor="stop-notes">
              {t('plan.editor.tripNote')}
            </label>
            <span className="fv">
              <textarea
                id="stop-notes"
                className="tinp"
                rows={3}
                value={notes}
                onChange={(e) => setNotes(e.target.value)}
                placeholder={t('plan.editor.tripNotePlaceholder')}
              />
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">{t('plan.editor.stopImmediate')}</span>
          <button type="button" className="btn" onClick={onClose}>
            {t('plan.editor.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || save.isPending}
            onClick={() => save.mutate()}
          >
            {t('plan.editor.save')}
          </button>
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
  const { t } = useI18n();
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
      <div
        className="exp-modal"
        role="dialog"
        aria-modal="true"
        aria-label={t('plan.editor.dayLabel', { day: dayIndex + 1 })}
      >
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--accent)' }}>
            ✎
          </span>
          <strong>{t('plan.editor.dayTitle', { day: dayIndex + 1 })}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={t('plan.editor.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="day-city">
              {t('plan.editor.city')}
            </label>
            <span className="fv">
              <input
                id="day-city"
                className="tinp"
                value={cityHint}
                onChange={(e) => setCityHint(e.target.value)}
                placeholder={t('plan.editor.cityPlaceholder')}
              />
            </span>
          </div>
          <div className="frow">
            <label className="fl" htmlFor="day-start">
              {t('plan.editor.window')}
            </label>
            <span className="fv">
              <input
                id="day-start"
                type="time"
                className="tinp time"
                value={windowStart}
                onChange={(e) => setWindowStart(e.target.value)}
                aria-label={t('plan.editor.windowStart')}
              />
              <span className="muted">→</span>
              <input
                type="time"
                className="tinp time"
                value={windowEnd}
                onChange={(e) => setWindowEnd(e.target.value)}
                aria-label={t('plan.editor.windowEnd')}
              />
            </span>
          </div>
          {windowStart && windowEnd && windowEnd <= windowStart && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="hint bad">⚠ {t('plan.editor.windowError')}</span>
              </span>
            </div>
          )}
        </div>
        <div className="exp-foot">
          <span className="hint grow">{t('plan.editor.dayImmediate')}</span>
          <button type="button" className="btn" onClick={onClose}>
            {t('plan.editor.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || save.isPending}
            onClick={() => save.mutate()}
          >
            {t('plan.editor.save')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
