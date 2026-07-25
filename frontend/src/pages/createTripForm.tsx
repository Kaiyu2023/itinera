import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';

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
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [name, setName] = useState('');
  const [startDate, setStartDate] = useState('');
  const [endDate, setEndDate] = useState('');
  const [baseCurrency, setBaseCurrency] = useState('USD');

  const datesOk = !!startDate && !!endDate && endDate >= startDate;
  const canSave = name.trim().length > 0 && datesOk;

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
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label="New trip">
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--accent)' }}>
            🧭
          </span>
          <strong>New trip</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <label className="fl" htmlFor="trip-name">
              Name
            </label>
            <span className="fv">
              <input
                id="trip-name"
                className="tinp"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Spring in Kyushu"
              />
            </span>
          </div>

          <div className="frow">
            <label className="fl" htmlFor="trip-start">
              Dates
            </label>
            <span className="fv">
              <input
                id="trip-start"
                type="date"
                className="tinp date"
                value={startDate}
                onChange={(e) => setStartDate(e.target.value)}
                aria-label="Start date"
              />
              <span className="muted">→</span>
              <input
                type="date"
                className="tinp date"
                value={endDate}
                min={startDate || undefined}
                onChange={(e) => setEndDate(e.target.value)}
                aria-label="End date"
              />
            </span>
          </div>
          {startDate && endDate && endDate < startDate && (
            <div className="frow">
              <span className="fl" />
              <span className="fv">
                <span className="hint bad">⚠ The end date can't be before the start date.</span>
              </span>
            </div>
          )}

          <div className="frow">
            <label className="fl" htmlFor="trip-cur">
              Currency
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
              <span className="hint">The trip's base currency for the ledger.</span>
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">You'll be the trip's leader. Invite the group once it's created.</span>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || create.isPending}
            onClick={() => create.mutate()}
          >
            Create trip
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
