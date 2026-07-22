import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';

/**
 * Standalone poll composer (§4.2). A plain `decision` poll — a question, 2–6
 * free-text options, an optional note, a closing date and whether more than one
 * choice may be picked. Maps straight to `CreatePollInput`; the mock opens the
 * poll on create (status → `open`), so the Polls tab shows it live with voting
 * immediately. Quorum isn't part of the input shape — the mock derives it from
 * the member count — so there's no quorum field here.
 */

const MAX_OPTIONS = 6;

/** A date-input value (YYYY-MM-DD) → an end-of-that-day ISO instant. */
function dayEndIso(dateStr: string): string {
  return new Date(`${dateStr}T23:59:00`).toISOString();
}

/** YYYY-MM-DD `n` days from today, for the default closing date. */
function dateInDays(n: number): string {
  const d = new Date();
  d.setDate(d.getDate() + n);
  return d.toISOString().slice(0, 10);
}

export function PollComposer({ tripId, onClose }: { tripId: string; onClose: () => void }) {
  const api = useApi();
  const queryClient = useQueryClient();

  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [options, setOptions] = useState<string[]>(['', '']);
  const [closesDate, setClosesDate] = useState(dateInDays(3));
  const [allowMulti, setAllowMulti] = useState(false);

  const filledOptions = options.map((o) => o.trim()).filter(Boolean);
  const canSave = title.trim().length > 0 && filledOptions.length >= 2 && !!closesDate;

  const setOption = (i: number, v: string) => setOptions((prev) => prev.map((o, j) => (j === i ? v : o)));
  const addOption = () => setOptions((prev) => (prev.length < MAX_OPTIONS ? [...prev, ''] : prev));
  const removeOption = (i: number) => setOptions((prev) => (prev.length > 2 ? prev.filter((_, j) => j !== i) : prev));

  const create = useMutation({
    mutationFn: () =>
      api.createPoll(tripId, {
        kind: 'decision',
        title: title.trim(),
        description: description.trim(),
        options: filledOptions.map((label) => ({ label })),
        closesAt: dayEndIso(closesDate),
        allowMulti,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['polls', tripId] });
      onClose();
    },
  });

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label="New poll">
        <div className="mtop">
          <span className="mtop-ic" style={{ background: 'var(--color-primary)' }}>🗳️</span>
          <strong>New poll</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="exp-body">
          <div className="frow" style={{ alignItems: 'start' }}>
            <label className="fl" htmlFor="poll-q">Question</label>
            <span className="fv">
              <textarea id="poll-q" className="tinp" rows={2} value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What should the group decide?" />
            </span>
          </div>

          <div className="frow">
            <label className="fl" htmlFor="poll-desc">Note</label>
            <span className="fv">
              <input id="poll-desc" className="tinp" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Optional context for voters" />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Options</span>
            <span className="fv col" style={{ gap: 7 }}>
              {options.map((o, i) => (
                <div key={i} className="add-row">
                  <span className="add-box" style={{ borderRadius: '50%' }} />
                  <input
                    className="tinp"
                    value={o}
                    onChange={(e) => setOption(i, e.target.value)}
                    placeholder={`Option ${i + 1}`}
                    aria-label={`Option ${i + 1}`}
                  />
                  <button
                    type="button"
                    className="del-x"
                    onClick={() => removeOption(i)}
                    disabled={options.length <= 2}
                    aria-label={`Remove option ${i + 1}`}
                  >
                    ✕
                  </button>
                </div>
              ))}
              {options.length < MAX_OPTIONS && (
                <button type="button" className="rowbtn" onClick={addOption}>+ Add another option</button>
              )}
              <span className="hint">Two options minimum, up to {MAX_OPTIONS}.</span>
            </span>
          </div>

          <div className="frow">
            <label className="fl" htmlFor="poll-closes">Closes</label>
            <span className="fv">
              <input
                id="poll-closes"
                type="date"
                className="tinp date"
                value={closesDate}
                min={dateInDays(0)}
                onChange={(e) => setClosesDate(e.target.value)}
              />
              <span className="hint">Voting stays open until the end of this day.</span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">Choices</span>
            <span className="fv">
              <label className="poll-multi">
                <input type="checkbox" checked={allowMulti} onChange={(e) => setAllowMulti(e.target.checked)} />
                Let voters pick more than one option
              </label>
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">Opens immediately for the whole group. Quorum is half the members.</span>
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="button" className="btn accent" disabled={!canSave || create.isPending} onClick={() => create.mutate()}>Open poll</button>
        </div>
      </div>
    </SheetModal>
  );
}
