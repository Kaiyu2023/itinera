import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import { useI18n } from '../i18n';

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

export function PollComposer({
  tripId,
  onCreated,
  onClose,
}: {
  tripId: string;
  /** Hands the new poll's id back so the tab can reveal + flash the card. */
  onCreated?: (pollId: string) => void;
  onClose: () => void;
}) {
  const api = useApi();
  const { locale, t: ui } = useI18n();
  const queryClient = useQueryClient();
  const formatNumber = (value: number) => new Intl.NumberFormat(locale).format(value);

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
    onSuccess: (poll) => {
      queryClient.invalidateQueries({ queryKey: ['polls', tripId] });
      onCreated?.(poll.id);
      onClose();
    },
  });

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={ui('polls.composer.title')}>
        <div className="mtop">
          {/* Monochrome mark on the accent tile, inheriting --color-ink-on-fill:
              a full-colour emoji sat on a coloured tile fought it at every
              theme, and it was the only icon in the app that wasn't drawn. */}
          <span className="mtop-ic" style={{ background: 'var(--accent)' }} aria-hidden="true">
            <svg
              width="15"
              height="15"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
            >
              <path d="M4 12.5V8.5" />
              <path d="M8 12.5V3.5" />
              <path d="M12 12.5V6.5" />
            </svg>
          </span>
          <strong>{ui('polls.composer.title')}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={ui('common.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow" style={{ alignItems: 'start' }}>
            <label className="fl" htmlFor="poll-q">
              {ui('polls.composer.question')}
            </label>
            <span className="fv">
              <textarea
                id="poll-q"
                className="tinp"
                rows={2}
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={ui('polls.composer.questionPlaceholder')}
              />
            </span>
          </div>

          <div className="frow">
            <label className="fl" htmlFor="poll-desc">
              {ui('polls.composer.note')}
            </label>
            <span className="fv">
              <input
                id="poll-desc"
                className="tinp"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder={ui('polls.composer.notePlaceholder')}
              />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">{ui('polls.composer.options')}</span>
            <span className="fv col" style={{ gap: 7 }}>
              {options.map((o, i) => (
                <div key={i} className="add-row">
                  {/* Was a hollow circle — a radio button, to every eye that has
                      ever seen one. It promised a selection the composer does
                      not have and never should: picking is the voter's job, not
                      the author's. A number just says which option this is. */}
                  <span className="opt-num" aria-hidden="true">
                    {formatNumber(i + 1)}
                  </span>
                  <input
                    className="tinp"
                    value={o}
                    onChange={(e) => setOption(i, e.target.value)}
                    placeholder={ui('polls.composer.option', { number: formatNumber(i + 1) })}
                    aria-label={ui('polls.composer.option', { number: formatNumber(i + 1) })}
                  />
                  <button
                    type="button"
                    className="del-x"
                    onClick={() => removeOption(i)}
                    disabled={options.length <= 2}
                    aria-label={ui('polls.composer.removeOption', { number: formatNumber(i + 1) })}
                  >
                    ✕
                  </button>
                </div>
              ))}
              {options.length < MAX_OPTIONS && (
                <button type="button" className="rowbtn" onClick={addOption}>
                  {ui('polls.composer.addOption')}
                </button>
              )}
              <span className="hint">{ui('polls.composer.optionsHint', { max: formatNumber(MAX_OPTIONS) })}</span>
            </span>
          </div>

          <div className="frow">
            <label className="fl" htmlFor="poll-closes">
              {ui('polls.composer.closes')}
            </label>
            <span className="fv">
              <input
                id="poll-closes"
                type="date"
                className="tinp date"
                value={closesDate}
                min={dateInDays(0)}
                onChange={(e) => setClosesDate(e.target.value)}
              />
              <span className="hint">{ui('polls.composer.closesHint')}</span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('polls.composer.choices')}</span>
            <span className="fv">
              <label className="poll-multi">
                <input type="checkbox" checked={allowMulti} onChange={(e) => setAllowMulti(e.target.checked)} />
                {ui('polls.composer.allowMulti')}
              </label>
            </span>
          </div>
        </div>
        <div className="exp-foot">
          <span className="hint grow">{ui('polls.composer.opensHint')}</span>
          <button type="button" className="btn" onClick={onClose}>
            {ui('common.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || create.isPending}
            onClick={() => create.mutate()}
          >
            {ui('polls.openPoll')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
