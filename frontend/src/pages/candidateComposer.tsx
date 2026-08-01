import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent, ReactNode } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/useApi';
import type { CandidatePlaceInput } from '../api/client';
import type { CandidateWithPlace, Place, PlaceKind, PlanDetail } from '../api/types';
import { SheetModal } from '../components/SheetModal';
import { useI18n } from '../i18n';
import { MapView } from '../map/MapView';
import { useStopSearch } from './useStopSearch';
import { PLACE_KINDS } from './governanceDomain';
import { EMBED_PAD, padBounds, searchResultMarkers } from './planMapGeometry';
import { PLACE_KIND_COLOR } from './planShared';

const PLACE_ROWS_VISIBLE = 4;
const PLACE_KIND_MESSAGE = {
  sight: 'ideas.kind.sight',
  food: 'ideas.kind.food',
  lodging: 'ideas.kind.lodging',
  activity: 'ideas.kind.activity',
  transport_hub: 'ideas.kind.transportHub',
} as const;

type ComposerMode = 'find' | 'manual';
type ActivityDraft = { id: number; title: string; details: string };
type PlaceDraft = {
  name: string;
  kind: PlaceKind;
  city: string;
  address: string;
  website: string;
  phone: string;
  openingHours: string;
  photoUrls: string;
  summary: string;
  intro: string;
  activityIdeas: ActivityDraft[];
  practicalTips: string;
};

let nextActivityId = 1;
function activityDraft(title = '', details = ''): ActivityDraft {
  return { id: nextActivityId++, title, details };
}

function lines(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function blankPlaceDraft(name = ''): PlaceDraft {
  return {
    name,
    kind: 'sight',
    city: '',
    address: '',
    website: '',
    phone: '',
    openingHours: '',
    photoUrls: '',
    summary: '',
    intro: '',
    activityIdeas: [activityDraft()],
    practicalTips: '',
  };
}

function placeDraft(place?: Place): PlaceDraft {
  if (!place) return blankPlaceDraft();
  return {
    name: place.name,
    kind: place.kind,
    city: place.city,
    address: place.address,
    website: place.website ?? '',
    phone: place.phone ?? '',
    openingHours: place.openingHours?.weekdayText.join('\n') ?? '',
    photoUrls: place.photoUrls.join('\n'),
    summary: place.guide?.summary ?? '',
    intro: place.guide?.intro ?? '',
    activityIdeas: place.guide?.activityIdeas.length
      ? place.guide.activityIdeas.map((idea) => activityDraft(idea.title, idea.details ?? ''))
      : [activityDraft()],
    practicalTips: place.guide?.practicalTips.join('\n') ?? '',
  };
}

function hasGuideContent(draft: PlaceDraft): boolean {
  return !!(
    draft.summary.trim() ||
    draft.intro.trim() ||
    draft.practicalTips.trim() ||
    draft.activityIdeas.some((idea) => idea.title.trim() || idea.details.trim())
  );
}

function candidatePlaceInput(draft: PlaceDraft): CandidatePlaceInput {
  const guideContent = hasGuideContent(draft);
  return {
    name: draft.name.trim(),
    kind: draft.kind,
    city: draft.city.trim(),
    address: draft.address.trim(),
    website: draft.website.trim() || null,
    phone: draft.phone.trim() || null,
    openingHours: lines(draft.openingHours),
    photoUrls: lines(draft.photoUrls),
    guide: guideContent
      ? {
          summary: draft.summary.trim(),
          intro: draft.intro.trim(),
          activityIdeas: draft.activityIdeas
            .filter((idea) => idea.title.trim() || idea.details.trim())
            .map((idea) => ({
              title: idea.title.trim(),
              ...(idea.details.trim() ? { details: idea.details.trim() } : {}),
            })),
          practicalTips: lines(draft.practicalTips),
        }
      : null,
  };
}

function FormRow({
  label,
  htmlFor,
  children,
  hint,
}: {
  label: string;
  htmlFor?: string;
  children: ReactNode;
  hint?: string;
}) {
  return (
    <div className="frow" style={{ alignItems: 'start' }}>
      {htmlFor ? (
        <label className="fl" htmlFor={htmlFor}>
          {label}
        </label>
      ) : (
        <span className="fl">{label}</span>
      )}
      <span className="fv col" style={{ gap: 6 }}>
        {children}
        {hint && <span className="hint">{hint}</span>}
      </span>
    </div>
  );
}

/** Add or edit a trip idea and its candidate-owned place snapshot. */
export function CandidateComposer({
  tripId,
  detail,
  candidate,
  initialQuery,
  pickFirst,
  onSaved,
  onClose,
}: {
  tripId: string;
  detail: PlanDetail | null;
  candidate?: CandidateWithPlace;
  initialQuery?: string | null;
  pickFirst?: boolean;
  onSaved?: (candidateId: string) => void;
  onClose: () => void;
}) {
  const api = useApi();
  const { locale, t: ui } = useI18n();
  const queryClient = useQueryClient();
  const search = useStopSearch();
  const fieldId = useId();
  const editing = !!candidate;

  const [mode, setMode] = useState<ComposerMode>('find');
  const [draft, setDraft] = useState<PlaceDraft>(() => placeDraft(candidate?.place));
  const [sourcePlaceId, setSourcePlaceId] = useState<string | null>(candidate?.sourcePlaceId ?? null);
  const [pitch, setPitch] = useState(candidate?.pitch ?? '');
  const [tags, setTags] = useState<string[]>(candidate?.tags ?? []);
  const [tagDraft, setTagDraft] = useState('');
  const [pitchAnyway, setPitchAnyway] = useState(false);

  function updateDraft<K extends keyof PlaceDraft>(key: K, value: PlaceDraft[K]) {
    setDraft((current) => ({ ...current, [key]: value }));
  }

  const booted = useRef(false);
  useEffect(() => {
    if (editing || booted.current) return;
    booted.current = true;
    const q = initialQuery?.trim();
    if (q) {
      search.setQuery(q);
      if (pickFirst) search.pickFirstOnNext();
    }
  }, [editing, initialQuery, pickFirst, search]);

  const selected = search.selected;
  useEffect(() => {
    if (editing || mode !== 'find' || !selected) return;
    setSourcePlaceId(selected.id);
    setDraft(placeDraft(selected));
  }, [editing, mode, selected]);

  const selectedInTrip = !editing && !!sourcePlaceId && !!detail?.places.some((place) => place.id === sourcePlaceId);
  useEffect(() => setPitchAnyway(false), [sourcePlaceId]);

  function selectMode(next: ComposerMode) {
    if (next === mode) return;
    if (next === 'manual') {
      setSourcePlaceId(null);
      setDraft(blankPlaceDraft(search.query.trim()));
    } else if (selected) {
      setSourcePlaceId(selected.id);
      setDraft(placeDraft(selected));
    } else {
      setSourcePlaceId(null);
      setDraft(blankPlaceDraft());
    }
    setMode(next);
  }

  function clearSelection() {
    search.select(null);
    setSourcePlaceId(null);
    setDraft(blankPlaceDraft());
  }

  function commitTag() {
    const value = tagDraft.trim().replace(/,+$/, '').trim();
    if (value && !tags.includes(value)) setTags((current) => [...current, value]);
    setTagDraft('');
  }

  function onTagKey(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === 'Enter' || event.key === ',') {
      event.preventDefault();
      commitTag();
    } else if (event.key === 'Backspace' && !tagDraft && tags.length) {
      setTags((current) => current.slice(0, -1));
    }
  }

  function updateActivity(id: number, key: 'title' | 'details', value: string) {
    updateDraft(
      'activityIdeas',
      draft.activityIdeas.map((idea) => (idea.id === id ? { ...idea, [key]: value } : idea)),
    );
  }

  function removeActivity(id: number) {
    const remaining = draft.activityIdeas.filter((idea) => idea.id !== id);
    updateDraft('activityIdeas', remaining.length ? remaining : [activityDraft()]);
  }

  const guidePresent = hasGuideContent(draft);
  const guideIncomplete = guidePresent && (!draft.summary.trim() || !draft.intro.trim());
  const activityIncomplete = draft.activityIdeas.some((idea) => !!idea.details.trim() && !idea.title.trim());
  const placeChosen = editing || mode === 'manual' || !!sourcePlaceId;
  const canSave =
    placeChosen &&
    !!draft.name.trim() &&
    !!draft.city.trim() &&
    !!pitch.trim() &&
    !guideIncomplete &&
    !activityIncomplete &&
    (!selectedInTrip || pitchAnyway);

  const save = useMutation({
    mutationFn: () => {
      const pendingTag = tagDraft.trim().replace(/,+$/, '').trim();
      const allTags = pendingTag && !tags.includes(pendingTag) ? [...tags, pendingTag] : tags;
      const input = { place: candidatePlaceInput(draft), pitch: pitch.trim(), tags: allTags };
      return candidate
        ? api.updateCandidate(candidate.id, input)
        : api.addCandidate(tripId, { ...input, sourcePlaceId });
    },
    onSuccess: (saved) => {
      queryClient.invalidateQueries({ queryKey: ['candidates', tripId] });
      queryClient.invalidateQueries({ queryKey: ['history', tripId] });
      onSaved?.(saved.id);
      onClose();
    },
  });

  const markers = useMemo(
    () =>
      searchResultMarkers(search.results, search.selectedId, locale).map((marker) =>
        marker.selected ? marker : { ...marker, tag: undefined },
      ),
    [search.results, search.selectedId, locale],
  );
  const bounds = useMemo(
    () =>
      padBounds(
        (search.results.length ? search.results : (detail?.places ?? [])).map((place) => ({
          lng: place.lng,
          lat: place.lat,
        })),
        EMBED_PAD,
      ),
    [search.results, detail],
  );
  const [mapShown, setMapShown] = useState(false);
  useEffect(() => {
    if (search.results.length) setMapShown(true);
  }, [search.results.length]);

  const listRef = useRef<HTMLDivElement>(null);
  const [listAtEnd, setListAtEnd] = useState(false);
  function onListScroll() {
    const element = listRef.current;
    if (element) setListAtEnd(element.scrollTop + element.clientHeight >= element.scrollHeight - 2);
  }
  useEffect(() => setListAtEnd(false), [search.results]);

  const title = ui(editing ? 'ideas.composer.editTitle' : 'ideas.composer.title');
  const showPlaceForm = editing || mode === 'manual' || !!sourcePlaceId;

  return (
    <SheetModal onClose={onClose}>
      <div className="exp-modal cand-modal" role="dialog" aria-modal="true" aria-label={title}>
        <div className="mtop">
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
          <strong>{title}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={ui('common.close')}>
            ×
          </button>
        </div>

        <div className="exp-body">
          {!editing && (
            <FormRow label={ui('ideas.composer.addMethod')}>
              <span className="route-seg">
                <button
                  type="button"
                  className={mode === 'find' ? 'active' : ''}
                  aria-pressed={mode === 'find'}
                  onClick={() => selectMode('find')}
                >
                  {ui('ideas.composer.findPlace')}
                </button>
                <button
                  type="button"
                  className={mode === 'manual' ? 'active' : ''}
                  aria-pressed={mode === 'manual'}
                  onClick={() => selectMode('manual')}
                >
                  {ui('ideas.composer.enterManually')}
                </button>
              </span>
            </FormRow>
          )}

          {!editing && mode === 'find' && (
            <>
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
              <FormRow label={ui('ideas.composer.place')} htmlFor={`${fieldId}-search`}>
                <input
                  id={`${fieldId}-search`}
                  className="tinp"
                  placeholder={ui('ideas.composer.searchPlaceholder')}
                  value={search.query}
                  onChange={(event) => search.setQuery(event.target.value)}
                  aria-label={ui('ideas.composer.searchAria')}
                />
                {search.query.trim() && (
                  <div
                    className={`place-results${search.results.length > PLACE_ROWS_VISIBLE && !listAtEnd ? ' more' : ''}`}
                    ref={listRef}
                    onScroll={onListScroll}
                  >
                    {search.loading && <span className="muted pr-status">{ui('common.searching')}</span>}
                    {!search.loading && search.results.length === 0 && (
                      <span className="muted pr-status">{ui('ideas.composer.noMatches')}</span>
                    )}
                    {search.results.map((result) => (
                      <button
                        key={result.id}
                        type="button"
                        className={`place-result${result.id === search.selectedId ? ' sel' : ''}`}
                        style={{ '--kc': PLACE_KIND_COLOR[result.kind] } as CSSProperties}
                        onClick={() => search.select(result.id)}
                      >
                        <span className="pr-dot" />
                        <span className="pr-main">
                          <span className="pr-name">{result.name}</span>
                          <span className="pr-sub">
                            {ui(PLACE_KIND_MESSAGE[result.kind])} · {result.city}
                          </span>
                        </span>
                        {detail?.places.some((place) => place.id === result.id) && (
                          <span className="badge">{ui('ideas.composer.inTrip')}</span>
                        )}
                      </button>
                    ))}
                  </div>
                )}
                {selected && sourcePlaceId && (
                  <span
                    className={`cand-picked${selectedInTrip ? ' dupe' : ''}`}
                    style={{ '--kc': PLACE_KIND_COLOR[selected.kind] } as CSSProperties}
                  >
                    <span className="pr-dot" />
                    <b>{selected.name}</b>
                    <span className="muted">
                      · {ui(PLACE_KIND_MESSAGE[selected.kind])} · {selected.city}
                    </span>
                    <button
                      type="button"
                      className="clear-sel inline"
                      onClick={clearSelection}
                      aria-label={ui('ideas.composer.clearSelection')}
                    >
                      ×
                    </button>
                  </span>
                )}
                {selectedInTrip && (
                  <span className="cand-dupe-warn">
                    <span className="warn-ic" aria-hidden="true">
                      !
                    </span>
                    <span className="warn-txt">
                      {ui('ideas.composer.duplicate', { place: selected!.name })}
                      <label className="cand-anyway">
                        <input
                          type="checkbox"
                          checked={pitchAnyway}
                          onChange={(event) => setPitchAnyway(event.target.checked)}
                        />
                        {ui('ideas.composer.addAnyway')}
                      </label>
                    </span>
                  </span>
                )}
              </FormRow>
            </>
          )}

          {showPlaceForm && (
            <>
              <div style={{ display: 'grid', gap: 2 }}>
                <strong>{ui('ideas.composer.placeDetails')}</strong>
                <span className="hint">{ui('ideas.composer.placeDetailsHint')}</span>
              </div>
              <FormRow label={ui('ideas.composer.name')} htmlFor={`${fieldId}-name`}>
                <input
                  id={`${fieldId}-name`}
                  className="tinp"
                  required
                  value={draft.name}
                  onChange={(event) => updateDraft('name', event.target.value)}
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.kind')} htmlFor={`${fieldId}-kind`}>
                <select
                  id={`${fieldId}-kind`}
                  className="tinp"
                  value={draft.kind}
                  onChange={(event) => updateDraft('kind', event.target.value as PlaceKind)}
                >
                  {PLACE_KINDS.map((kind) => (
                    <option key={kind} value={kind}>
                      {ui(PLACE_KIND_MESSAGE[kind])}
                    </option>
                  ))}
                </select>
              </FormRow>
              <FormRow label={ui('ideas.composer.city')} htmlFor={`${fieldId}-city`}>
                <input
                  id={`${fieldId}-city`}
                  className="tinp"
                  required
                  value={draft.city}
                  onChange={(event) => updateDraft('city', event.target.value)}
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.address')} htmlFor={`${fieldId}-address`}>
                <input
                  id={`${fieldId}-address`}
                  className="tinp"
                  value={draft.address}
                  onChange={(event) => updateDraft('address', event.target.value)}
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.website')} htmlFor={`${fieldId}-website`}>
                <input
                  id={`${fieldId}-website`}
                  className="tinp"
                  type="url"
                  value={draft.website}
                  onChange={(event) => updateDraft('website', event.target.value)}
                  placeholder="https://"
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.phone')} htmlFor={`${fieldId}-phone`}>
                <input
                  id={`${fieldId}-phone`}
                  className="tinp"
                  type="tel"
                  value={draft.phone}
                  onChange={(event) => updateDraft('phone', event.target.value)}
                />
              </FormRow>
              <FormRow
                label={ui('ideas.composer.openingHours')}
                htmlFor={`${fieldId}-hours`}
                hint={ui('ideas.composer.onePerLine')}
              >
                <textarea
                  id={`${fieldId}-hours`}
                  className="tinp"
                  rows={2}
                  value={draft.openingHours}
                  onChange={(event) => updateDraft('openingHours', event.target.value)}
                />
              </FormRow>
              <FormRow
                label={ui('ideas.composer.photos')}
                htmlFor={`${fieldId}-photos`}
                hint={ui('ideas.composer.photosHint')}
              >
                <textarea
                  id={`${fieldId}-photos`}
                  className="tinp"
                  rows={2}
                  value={draft.photoUrls}
                  onChange={(event) => updateDraft('photoUrls', event.target.value)}
                />
              </FormRow>

              <div style={{ display: 'grid', gap: 2, marginTop: 8 }}>
                <strong>{ui('ideas.composer.guide')}</strong>
                <span className="hint">{ui('ideas.composer.guideHint')}</span>
              </div>
              <FormRow label={ui('ideas.composer.summary')} htmlFor={`${fieldId}-summary`}>
                <textarea
                  id={`${fieldId}-summary`}
                  className="tinp"
                  rows={2}
                  value={draft.summary}
                  onChange={(event) => updateDraft('summary', event.target.value)}
                  placeholder={ui('ideas.composer.summaryPlaceholder')}
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.introduction')} htmlFor={`${fieldId}-intro`}>
                <textarea
                  id={`${fieldId}-intro`}
                  className="tinp"
                  rows={4}
                  value={draft.intro}
                  onChange={(event) => updateDraft('intro', event.target.value)}
                  placeholder={ui('ideas.composer.introductionPlaceholder')}
                />
              </FormRow>
              <FormRow label={ui('ideas.composer.activities')} hint={ui('ideas.composer.activitiesHint')}>
                <span className="fv col" style={{ width: '100%', gap: 10 }}>
                  {draft.activityIdeas.map((idea, index) => (
                    <span className="fv col" style={{ width: '100%', gap: 6 }} key={idea.id}>
                      <span style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%' }}>
                        <label className="hint grow" htmlFor={`${fieldId}-activity-${idea.id}`}>
                          {ui('ideas.composer.activityNumber', { number: index + 1 })}
                        </label>
                        <button
                          type="button"
                          className="btn sm"
                          onClick={() => removeActivity(idea.id)}
                          aria-label={ui('ideas.composer.removeActivity', { number: index + 1 })}
                        >
                          {ui('ideas.composer.remove')}
                        </button>
                      </span>
                      <input
                        id={`${fieldId}-activity-${idea.id}`}
                        className="tinp"
                        value={idea.title}
                        onChange={(event) => updateActivity(idea.id, 'title', event.target.value)}
                        placeholder={ui('ideas.composer.activityPlaceholder')}
                      />
                      <textarea
                        className="tinp"
                        rows={2}
                        aria-label={ui('ideas.composer.activityDetails', { number: index + 1 })}
                        value={idea.details}
                        onChange={(event) => updateActivity(idea.id, 'details', event.target.value)}
                        placeholder={ui('ideas.composer.activityDetailsPlaceholder')}
                      />
                    </span>
                  ))}
                  <button
                    type="button"
                    className="btn sm"
                    onClick={() => updateDraft('activityIdeas', [...draft.activityIdeas, activityDraft()])}
                  >
                    {ui('ideas.composer.addActivity')}
                  </button>
                </span>
              </FormRow>
              <FormRow
                label={ui('ideas.composer.tips')}
                htmlFor={`${fieldId}-tips`}
                hint={ui('ideas.composer.onePerLine')}
              >
                <textarea
                  id={`${fieldId}-tips`}
                  className="tinp"
                  rows={3}
                  value={draft.practicalTips}
                  onChange={(event) => updateDraft('practicalTips', event.target.value)}
                />
              </FormRow>
              {guideIncomplete && <span className="warn">{ui('ideas.composer.guideRequired')}</span>}
              {activityIncomplete && <span className="warn">{ui('ideas.composer.activityTitleRequired')}</span>}
            </>
          )}

          <FormRow label={ui('ideas.composer.why')} htmlFor={`${fieldId}-why`}>
            <textarea
              id={`${fieldId}-why`}
              className="tinp"
              rows={3}
              value={pitch}
              onChange={(event) => setPitch(event.target.value)}
              placeholder={ui('ideas.composer.whyPlaceholder')}
              aria-label={ui('ideas.composer.whyAria')}
            />
          </FormRow>

          <FormRow label={ui('ideas.composer.tags')} hint={ui('ideas.composer.tagsHint')}>
            <div className="tag-input">
              {tags.map((tag) => (
                <span key={tag} className="tag-chip">
                  {tag}
                  <button
                    type="button"
                    onClick={() => setTags((current) => current.filter((item) => item !== tag))}
                    aria-label={ui('ideas.composer.removeTag', { tag })}
                  >
                    ×
                  </button>
                </span>
              ))}
              <input
                className="tag-entry"
                value={tagDraft}
                onChange={(event) => setTagDraft(event.target.value)}
                onKeyDown={onTagKey}
                onBlur={commitTag}
                placeholder={tags.length ? '' : ui('ideas.composer.tagsPlaceholder')}
                aria-label={ui('ideas.composer.addTag')}
              />
            </div>
          </FormRow>

          {save.isError && <span className="warn">{ui('ideas.composer.saveError')}</span>}
        </div>

        <div className="exp-foot">
          <span className="hint grow">
            {ui(editing ? 'ideas.composer.editAppliesImmediately' : 'ideas.composer.appliesImmediately')}
          </span>
          <button type="button" className="btn" onClick={onClose} disabled={save.isPending}>
            {ui('common.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || save.isPending}
            onClick={() => save.mutate()}
          >
            {ui(editing ? 'ideas.composer.save' : 'ideas.composer.submit')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
