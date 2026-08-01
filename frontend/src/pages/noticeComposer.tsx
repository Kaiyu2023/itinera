import { useEffect, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { SheetModal } from '../components/SheetModal';
import type { Notice, NoticeCategory } from '../api/types';
import { NOTICE_CATEGORY_META, NOTICE_CATEGORY_ORDER } from './noticesShared';
import { fillStyle } from '../lib/oklch';
import { useI18n } from '../i18n';

/**
 * Create / edit a notice (§ mockup F). Category, title, body, optional source
 * URL and repeatable checklist rows — the shape of `createNotice`. On edit only
 * the content the contract's `NoticePatch` carries (title / body / sourceUrl)
 * persists; checklist ticks are the group's shared state, not set here.
 */
export function NoticeComposer({
  tripId,
  mode,
  notice,
  onClose,
}: {
  tripId: string;
  mode: 'new' | 'edit';
  notice?: Notice;
  onClose: () => void;
}) {
  const api = useApi();
  const { t: ui, formatNumber } = useI18n();
  const queryClient = useQueryClient();
  const members = useMembers(tripId);
  const editing = mode === 'edit' && notice;

  const [category, setCategory] = useState<NoticeCategory>(notice?.category ?? 'money');
  const [title, setTitle] = useState(notice?.title ?? '');
  const [body, setBody] = useState(notice?.body ?? '');
  const [sourceUrl, setSourceUrl] = useState(notice?.sourceUrl ?? '');
  const [items, setItems] = useState<string[]>(editing ? [] : ['']);

  // "Who's involved" — the checklist audience. null until seeded from members;
  // seeded to everyone (or the notice's existing audience when editing).
  const memberList = members.data ?? [];
  const [audience, setAudience] = useState<string[] | null>(null);
  useEffect(() => {
    if (audience === null && memberList.length) {
      setAudience(notice?.audience && notice.audience.length ? notice.audience : memberList.map((u) => u.id));
    }
  }, [memberList, audience, notice]);
  const selectedAudience = audience ?? memberList.map((u) => u.id);
  const everyone = memberList.length > 0 && selectedAudience.length === memberList.length;
  const toggleMember = (id: string) =>
    setAudience((prev) => {
      const base = prev ?? memberList.map((u) => u.id);
      return base.includes(id) ? base.filter((x) => x !== id) : [...base, id];
    });

  const save = useMutation({
    mutationFn: () => {
      if (editing) {
        return api.updateNotice(notice!.id, {
          title: title.trim(),
          body: body.trim(),
          sourceUrl: sourceUrl.trim() || null,
          audience: everyone ? null : selectedAudience,
        });
      }
      return api.createNotice(tripId, {
        category,
        title: title.trim(),
        body: body.trim(),
        sourceUrl: sourceUrl.trim() || undefined,
        checklistItems: items.map((t) => t.trim()).filter(Boolean),
        audience: everyone ? undefined : selectedAudience,
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['notices', tripId] });
      onClose();
    },
  });

  const canSave = title.trim().length > 0 && body.trim().length > 0 && selectedAudience.length > 0;
  const setItem = (i: number, v: string) => setItems((prev) => prev.map((t, j) => (j === i ? v : t)));
  const removeItem = (i: number) => setItems((prev) => prev.filter((_, j) => j !== i));

  return (
    <SheetModal onClose={onClose}>
      <div
        className="exp-modal"
        role="dialog"
        aria-modal="true"
        aria-label={ui(editing ? 'prep.composer.editTitle' : 'prep.composer.newTitle')}
      >
        <div className="mtop">
          <span>🧳</span>
          <strong>{ui(editing ? 'prep.composer.editTitle' : 'prep.composer.newTitle')}</strong>
          <button type="button" className="x" onClick={onClose} aria-label={ui('prep.composer.close')}>
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <span className="fl">{ui('prep.composer.category')}</span>
            <span className="fv">
              <span className="cat-pick">
                {NOTICE_CATEGORY_ORDER.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={`cat-opt${c === category ? ' sel' : ''}`}
                    /* Same toggle-state gap `.aud-chip` below already closes:
                       `.sel` is a border colour, and nothing else said which
                       category was picked. */
                    aria-pressed={c === category}
                    disabled={!!editing}
                    onClick={() => setCategory(c)}
                    style={{ textTransform: 'capitalize' }}
                  >
                    {NOTICE_CATEGORY_META[c].emoji} {ui(NOTICE_CATEGORY_META[c].labelKey)}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">{ui('prep.composer.audience')}</span>
            <span className="fv col" style={{ gap: 6 }}>
              <span className="aud-pick">
                {memberList.map((u) => {
                  const on = selectedAudience.includes(u.id);
                  return (
                    <button
                      key={u.id}
                      type="button"
                      className={`aud-chip${on ? ' on' : ''}`}
                      aria-pressed={on}
                      onClick={() => toggleMember(u.id)}
                    >
                      <span className="avatar xs" style={fillStyle(u.avatarColor)}>
                        {u.displayName[0]}
                      </span>
                      {u.displayName}
                    </button>
                  );
                })}
              </span>
              <span className="hint">
                {everyone ? (
                  ui('prep.composer.everyoneHint')
                ) : (
                  <>
                    {ui('prep.composer.subsetHintPrefix', { count: formatNumber(selectedAudience.length) })}{' '}
                    {ui('prep.composer.subsetHintSuffix')}
                  </>
                )}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('prep.composer.title')}</span>
            <span className="fv">
              <input
                className="tinp"
                value={title}
                aria-label={ui('prep.composer.title')}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={ui('prep.composer.titlePlaceholder')}
              />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">{ui('prep.composer.body')}</span>
            <span className="fv">
              <textarea
                className="tinp"
                rows={4}
                value={body}
                aria-label={ui('prep.composer.body')}
                onChange={(e) => setBody(e.target.value)}
                placeholder={ui('prep.composer.bodyPlaceholder')}
              />
            </span>
          </div>

          <div className="frow">
            <span className="fl">{ui('prep.composer.sourceUrl')}</span>
            <span className="fv">
              <input
                className="tinp"
                value={sourceUrl}
                aria-label={ui('prep.composer.sourceUrl')}
                onChange={(e) => setSourceUrl(e.target.value)}
                placeholder={ui('prep.composer.sourcePlaceholder')}
              />
            </span>
          </div>

          {!editing && (
            <div className="frow" style={{ alignItems: 'start' }}>
              <span className="fl">{ui('prep.composer.checklist')}</span>
              <span className="fv col" style={{ gap: 7 }}>
                {items.map((t, i) => (
                  <div key={i} className="add-row">
                    <span className="add-box" />
                    <input
                      className="tinp"
                      value={t}
                      aria-label={ui('prep.composer.itemLabel', { number: formatNumber(i + 1) })}
                      onChange={(e) => setItem(i, e.target.value)}
                      placeholder={ui('prep.composer.itemPlaceholder')}
                    />
                    <button
                      type="button"
                      className="del-x"
                      onClick={() => removeItem(i)}
                      aria-label={ui('prep.composer.removeItem')}
                    >
                      ✕
                    </button>
                  </div>
                ))}
                <button type="button" className="rowbtn" onClick={() => setItems((prev) => [...prev, ''])}>
                  + {ui('prep.composer.addItem')}
                </button>
              </span>
            </div>
          )}
          {editing && <p className="hint">{ui('prep.composer.editingHint')}</p>}
        </div>
        <div className="exp-foot">
          {/* This line used to be hardcoded "Posts to everyone. You can pin it
              after." — sitting directly under an audience picker that could be
              reading "Just these 5". The `everyone` flag two screens up already
              knows the truth; say that instead of contradicting the control
              immediately above it. */}
          <span className="hint grow">
            {editing ? (
              ui('prep.composer.editVisible')
            ) : everyone ? (
              ui('prep.composer.postEveryone')
            ) : (
              <>
                {ui('prep.composer.postSubsetPrefix', { count: formatNumber(selectedAudience.length) })}{' '}
                {ui('prep.composer.postSubsetSuffix')}
              </>
            )}
          </span>
          <button type="button" className="btn" onClick={onClose}>
            {ui('prep.composer.cancel')}
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || save.isPending}
            onClick={() => save.mutate()}
          >
            {ui(editing ? 'prep.composer.save' : 'prep.composer.post')}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
