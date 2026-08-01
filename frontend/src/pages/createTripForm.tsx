import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import { useI18n } from '../i18n';

/**
 * Create-trip composer. Maps to `CreateTripInput` exactly: name, start / end
 * dates (native pickers, end ≥ start enforced) and base currency. Trip accent
 * theming is deliberately absent — the frozen input shape doesn't carry it, and
 * a picker whose choice the create call would discard is worse than none.
 *
 * The creator becomes the trip's leader (the mock stamps the role on create).
 * On success we invalidate the trip list and jump to the new trip.
 */

const CURRENCIES = ['USD', 'JPY', 'EUR', 'GBP'];

export function CreateTripForm({ onClose }: { onClose: () => void }) {
  const api = useApi();
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [name, setName] = useState('');
  const [startDate, setStartDate] = useState('');
  const [endDate, setEndDate] = useState('');
  const [baseCurrency, setBaseCurrency] = useState('USD');

  const datesOk = !!startDate && !!endDate && endDate >= startDate;
  const datesInvalid = !!startDate && !!endDate && endDate < startDate;
  const canSave = name.trim().length > 0 && datesOk;
  // Typed, but nothing survives the trim — the case that disabled the button
  // with no explanation anywhere on the form.
  const nameBlank = name.length > 0 && name.trim().length === 0;

  const create = useMutation({
    mutationFn: () => api.createTrip({ name: name.trim(), startDate, endDate, baseCurrency }),
    onSuccess: (trip) => {
      queryClient.invalidateQueries({ queryKey: ['trips'] });
      onClose();
      navigate(`/trips/${trip.id}`);
    },
  });

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={t('createTrip.title')}>
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--accent)' }}>
            🧭
          </span>
          <strong>{t('createTrip.title')}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={t('common.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="trip-name">
              {t('createTrip.name')}
            </label>
            <span className="fv">
              <input
                id="trip-name"
                className="tinp"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t('createTrip.namePlaceholder')}
                aria-describedby={nameBlank ? 'trip-name-why' : undefined}
                aria-invalid={nameBlank || undefined}
              />
            </span>
          </div>
          {/* The date rule two rows down explains itself the moment you break
              it. A whitespace-only name silently disabled "Create trip" and
              said nothing — same class of problem, opposite treatment, which
              is what made it jarring. Only shown once something has been
              typed: an untouched empty field isn't an error yet. */}
          {nameBlank && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="hint bad" id="trip-name-why" role="status">
                  ⚠ {t('createTrip.nameError')}
                </span>
              </span>
            </div>
          )}

          <div className="frow">
            <label className="fl" htmlFor="trip-start">
              {t('createTrip.dates')}
            </label>
            <span className="fv">
              <input
                id="trip-start"
                type="date"
                className="tinp date"
                value={startDate}
                onChange={(e) => setStartDate(e.target.value)}
                aria-label={t('createTrip.startDate')}
                aria-describedby={datesInvalid ? 'trip-date-why' : undefined}
                aria-invalid={datesInvalid || undefined}
              />
              <span className="muted">→</span>
              <input
                type="date"
                className="tinp date"
                value={endDate}
                min={startDate || undefined}
                onChange={(e) => setEndDate(e.target.value)}
                aria-label={t('createTrip.endDate')}
                aria-describedby={datesInvalid ? 'trip-date-why' : undefined}
                aria-invalid={datesInvalid || undefined}
              />
            </span>
          </div>
          {datesInvalid && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="hint bad" id="trip-date-why" role="status">
                  ⚠ {t('createTrip.dateError')}
                </span>
              </span>
            </div>
          )}

          {create.isError && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="hint bad" role="alert">
                  ⚠ {t('createTrip.error')}
                </span>
              </span>
            </div>
          )}

          <div className="frow">
            <label className="fl" htmlFor="trip-cur">
              {t('createTrip.currency')}
            </label>
            <span className="fv">
              <select
                id="trip-cur"
                className="tinp"
                value={baseCurrency}
                onChange={(e) => setBaseCurrency(e.target.value)}
              >
                {CURRENCIES.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <span className="hint">{t('createTrip.currencyHint')}</span>
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">{t('createTrip.leaderHint')}</span>
          <button type="button" className="btn" onClick={onClose}>
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || create.isPending}
            onClick={() => create.mutate()}
          >
            {t('createTrip.create')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
