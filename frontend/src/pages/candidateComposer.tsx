import { useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { SheetModal } from '../components/SheetModal';
import { MapView } from '../map/MapView';
import { useStopSearch } from './PlanGovernance';
import { PLACE_KIND_COLOR } from './planShared';
import { PLACE_KIND_LABEL } from './governanceShared';
import { EMBED_PAD, padBounds, searchResultMarkers } from './planMapGeometry';
import type { Place, PlanDetail } from '../api/types';

/**
 * "Pitch an idea" — the add-candidate composer (§3.2). Reuses the plan's
 * propose-a-stop search UX: a debounced place search over the catalog + trip
 * places, a results list, and an embedded map whose pins mirror the list and
 * are click-to-select. Candidates aren't governance-gated — the pitch applies
 * immediately and lands under "Competing for a slot".
 *
 * Fields map straight to `AddCandidateInput`: a place (required), a pitch
 * (required), and free-entry tags (optional).
 */

/** How many result rows `.place-results` is sized to show — kept in step with
    the `max-height` there, which is written as a multiple of the row height. */
const PLACE_ROWS_VISIBLE = 4;
export function CandidateComposer({
  tripId,
  detail,
  initialQuery,
  pickFirst,
  onAdded,
  onClose,
}: {
  tripId: string;
  detail: PlanDetail | null;
  /** Deep-link seed: pre-run this search on mount. */
  initialQuery?: string | null;
  /** Deep-link seed: auto-select the first hit of the seeded search. */
  pickFirst?: boolean;
  /** Hands the new candidate's id back so the tab can reveal + flash the card. */
  onAdded?: (candidateId: string) => void;
  onClose: () => void;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const search = useStopSearch();

  const [pitch, setPitch] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [tagDraft, setTagDraft] = useState('');
  /** Deliberate override of the "already in the trip" guard (see `canSave`). */
  const [pitchAnyway, setPitchAnyway] = useState(false);

  // One-shot deep-link seed (?q=&pick=first). Guarded so it fires once.
  const booted = useRef(false);
  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    const q = initialQuery?.trim();
    if (q) {
      search.setQuery(q);
      if (pickFirst) search.pickFirstOnNext();
    }
  }, [initialQuery, pickFirst, search]);

  const selected = search.selected;
  const selectedInTrip = !!selected && !!detail?.places.some((p) => p.id === selected.id);
  // Picking a different place is a fresh decision — never carry the override.
  useEffect(() => {
    setPitchAnyway(false);
  }, [search.selectedId]);

  const commitTag = () => {
    const t = tagDraft.trim().replace(/,+$/, '').trim();
    if (t && !tags.includes(t)) setTags((prev) => [...prev, t]);
    setTagDraft('');
  };
  const onTagKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      commitTag();
    } else if (e.key === 'Backspace' && !tagDraft && tags.length) {
      setTags((prev) => prev.slice(0, -1));
    }
  };
  const removeTag = (t: string) => setTags((prev) => prev.filter((x) => x !== t));

  /**
   * The "already in the trip" case used to be a five-word suffix on the picked
   * chip that nothing acted on — you could pitch the group a place they were
   * already going to, and the shortlist would happily carry the duplicate. It
   * is not forbidden (a second visit is a real thing to want), so it is a
   * confirm rather than a block.
   */
  const canSave = !!selected && pitch.trim().length > 0 && (!selectedInTrip || pitchAnyway);

  const add = useMutation({
    mutationFn: () => {
      // Fold any half-typed tag in so nothing the user wrote is dropped.
      const draft = tagDraft.trim().replace(/,+$/, '').trim();
      const allTags = draft && !tags.includes(draft) ? [...tags, draft] : tags;
      return api.addCandidate(tripId, { placeId: selected!.id, pitch: pitch.trim(), tags: allTags });
    },
    onSuccess: (candidate) => {
      queryClient.invalidateQueries({ queryKey: ['candidates', tripId] });
      onAdded?.(candidate.id);
      onClose();
    },
  });

  /**
   * Embedded map: search-result pins, two-way selectable with the list.
   *
   * Name tags are drawn only for the selected pin. `searchResultMarkers` tags
   * every hit, and a 7-hit search in a 220px pane stacked seven name plates on
   * top of each other and on top of the attribution — an unreadable pile. The
   * results list directly below already names every hit in full, so the map's
   * job here is *where*, not *what*.
   */
  const markers = useMemo(
    () =>
      searchResultMarkers(search.results, search.selectedId).map((m) => (m.selected ? m : { ...m, tag: undefined })),
    [search.results, search.selectedId],
  );
  const bounds = useMemo(
    () =>
      padBounds(
        // With no hits to frame, fall back to the trip's own places. It used to
        // fall through to `padBounds`' hardcoded Tokyo frame, so opening this
        // composer on the Aegean trip showed you Shinjuku.
        (search.results.length ? search.results : (detail?.places ?? [])).map((r) => ({ lng: r.lng, lat: r.lat })),
        EMBED_PAD,
      ),
    [search.results, detail],
  );

  /**
   * The map only earns its space once there is something to put on it. Before
   * the first search it was 180–220px of empty grid sitting between the sheet's
   * title and the field you opened it to type in — on a phone that is most of
   * the first screen. It stays up once shown, so clearing the query doesn't
   * make the sheet jump.
   */
  const [mapShown, setMapShown] = useState(false);
  useEffect(() => {
    if (search.results.length > 0) setMapShown(true);
  }, [search.results.length]);

  // Whether the results list still has rows below the fold — see `.place-results`.
  const listRef = useRef<HTMLDivElement>(null);
  const [listAtEnd, setListAtEnd] = useState(false);
  const onListScroll = () => {
    const el = listRef.current;
    if (el) setListAtEnd(el.scrollTop + el.clientHeight >= el.scrollHeight - 2);
  };
  useEffect(() => {
    setListAtEnd(false);
  }, [search.results]);

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal cand-modal" role="dialog" aria-modal="true" aria-label="Pitch an idea">
        <div className="mtop">
          {/* --color-kind-food is a *ledger expense category* hue (see the token
              rule in theme/tokens.css: "kind hues are ledger expense categories,
              and nothing in the plan") — a candidate is not an expense, and the
              full-colour emoji on top of it fought the tile at every theme.
              Monochrome mark on the one accent tile, inheriting the tile's ink. */}
          <span className="mtop-ic" style={{ background: 'var(--accent)' }} aria-hidden="true">
            <svg
              width="15"
              height="15"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
            >
              <circle cx="8" cy="6.4" r="4" />
              <path d="M6.1 11.9h3.8" />
              <path d="M6.9 13.9h2.2" />
            </svg>
          </span>
          <strong>Pitch an idea</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="exp-body">
          {mapShown && (
            <div className="compose-mappane cand-map">
              <MapView
                markers={markers}
                routes={[]}
                bounds={bounds}
                padding={18}
                onMarkerClick={(id) => id.startsWith('sr:') && search.select(id.slice(3))}
              />
            </div>
          )}

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Place</span>
            <span className="fv col" style={{ gap: 6 }}>
              <input
                id="cand-search"
                className="tinp"
                placeholder="Search places to pitch…"
                value={search.query}
                onChange={(e) => search.setQuery(e.target.value)}
                aria-label="Search places"
              />
              {search.query.trim() && (
                /* The list is sized to whole rows now, which removes the sliced
                   half-row that used to be the only hint there was more — so it
                   needs a deliberate one. The fade is drawn while more results
                   remain below and retracts once you reach the end. */
                <div
                  className={`place-results${search.results.length > PLACE_ROWS_VISIBLE && !listAtEnd ? ' more' : ''}`}
                  ref={listRef}
                  onScroll={onListScroll}
                >
                  {search.loading && <span className="muted pr-status">Searching…</span>}
                  {!search.loading && search.results.length === 0 && (
                    <span className="muted pr-status">No matches — try another name.</span>
                  )}
                  {search.results.map((r: Place) => (
                    <button
                      key={r.id}
                      type="button"
                      className={`place-result${r.id === search.selectedId ? ' sel' : ''}`}
                      style={{ '--kc': PLACE_KIND_COLOR[r.kind] } as CSSProperties}
                      onClick={() => search.select(r.id)}
                    >
                      <span className="pr-dot" />
                      <span className="pr-main">
                        <span className="pr-name">{r.name}</span>
                        <span className="pr-sub">
                          {PLACE_KIND_LABEL[r.kind]} · {r.city}
                        </span>
                      </span>
                      {detail?.places.some((p) => p.id === r.id) && <span className="badge">in trip</span>}
                    </button>
                  ))}
                </div>
              )}
              {selected && (
                <span
                  className={`cand-picked${selectedInTrip ? ' dupe' : ''}`}
                  style={{ '--kc': PLACE_KIND_COLOR[selected.kind] } as CSSProperties}
                >
                  <span className="pr-dot" />
                  <b>{selected.name}</b>
                  <span className="muted">
                    · {PLACE_KIND_LABEL[selected.kind]} · {selected.city}
                  </span>
                  <button
                    type="button"
                    className="clear-sel inline"
                    onClick={() => search.select(null)}
                    aria-label="Clear selection"
                  >
                    ✕
                  </button>
                </span>
              )}
              {selectedInTrip && (
                <span className="cand-dupe-warn">
                  <span className="warn-ic" aria-hidden="true">
                    !
                  </span>
                  <span className="warn-txt">
                    <b>{selected!.name}</b> is already on the itinerary. Pitching it again puts a duplicate on the
                    shortlist — worth it only if you're arguing for a second visit.
                    <label className="cand-anyway">
                      <input type="checkbox" checked={pitchAnyway} onChange={(e) => setPitchAnyway(e.target.checked)} />
                      Pitch it anyway
                    </label>
                  </span>
                </span>
              )}
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Pitch</span>
            <span className="fv">
              <textarea
                className="tinp"
                rows={3}
                value={pitch}
                onChange={(e) => setPitch(e.target.value)}
                placeholder="Why should the group go?"
                aria-label="Pitch"
              />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Tags</span>
            <span className="fv col" style={{ gap: 6 }}>
              <div className="tag-input">
                {tags.map((t) => (
                  <span key={t} className="tag-chip">
                    {t}
                    <button type="button" onClick={() => removeTag(t)} aria-label={`Remove ${t}`}>
                      ✕
                    </button>
                  </span>
                ))}
                <input
                  className="tag-entry"
                  value={tagDraft}
                  onChange={(e) => setTagDraft(e.target.value)}
                  onKeyDown={onTagKey}
                  onBlur={commitTag}
                  placeholder={tags.length ? '' : 'e.g. rainy-day, foodie (Enter or comma)'}
                  aria-label="Add a tag"
                />
              </div>
              <span className="hint">Optional — press Enter or comma to add each tag.</span>
            </span>
          </div>
        </div>
        <div className="exp-foot">
          {/* Short on purpose: in the narrow column this sits in on a phone, the
              old two-sentence version wrapped to five lines of standing footer.
              What a shortlist *is* now belongs to the tab's zero state. */}
          <span className="hint grow">Applies immediately — no approval needed.</span>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || add.isPending}
            onClick={() => add.mutate()}
          >
            Add to shortlist
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
