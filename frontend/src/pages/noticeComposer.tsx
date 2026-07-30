import { useEffect, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useApi } from '../api/ApiProvider';
import { useMembers } from '../components/hooks';
import { SheetModal } from '../components/SheetModal';
import type { Notice, NoticeCategory } from '../api/types';
import { NOTICE_CATEGORY_META, NOTICE_CATEGORY_ORDER } from './noticesShared';
import { fillStyle } from '../lib/oklch';

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
      <div className="exp-modal" role="dialog" aria-modal="true" aria-label={editing ? 'Edit notice' : 'New notice'}>
        <div className="mtop">
          <span>🧳</span>
          <strong>{editing ? 'Edit notice' : 'New notice'}</strong>
          <button type="button" className="x" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <div className="exp-body">
          <div className="frow">
            <span className="fl">Category</span>
            <span className="fv">
              <span className="cat-pick">
                {NOTICE_CATEGORY_ORDER.map((c) => (
                  <button
                    key={c}
                    type="button"
                    className={`cat-opt${c === category ? ' sel' : ''}`}
                    disabled={!!editing}
                    onClick={() => setCategory(c)}
                    style={{ textTransform: 'capitalize' }}
                  >
                    {NOTICE_CATEGORY_META[c].emoji} {NOTICE_CATEGORY_META[c].label}
                  </button>
                ))}
              </span>
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Who's involved</span>
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
                {everyone
                  ? 'Everyone on the trip — the whole group.'
                  : `Just these ${selectedAudience.length} — others still see it, but it's off their checklist.`}
              </span>
            </span>
          </div>

          <div className="frow">
            <span className="fl">Title</span>
            <span className="fv">
              <input
                className="tinp"
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="Short, plain headline"
              />
            </span>
          </div>

          <div className="frow" style={{ alignItems: 'start' }}>
            <span className="fl">Body</span>
            <span className="fv">
              <textarea
                className="tinp"
                rows={4}
                value={body}
                onChange={(e) => setBody(e.target.value)}
                placeholder="Markdown ok. **bold** for the key facts."
              />
            </span>
          </div>

          <div className="frow">
            <span className="fl">Source URL</span>
            <span className="fv">
              <input
                className="tinp"
                value={sourceUrl}
                onChange={(e) => setSourceUrl(e.target.value)}
                placeholder="https://… (optional)"
              />
            </span>
          </div>

          {!editing && (
            <div className="frow" style={{ alignItems: 'start' }}>
              <span className="fl">Checklist</span>
              <span className="fv col" style={{ gap: 7 }}>
                {items.map((t, i) => (
                  <div key={i} className="add-row">
                    <span className="add-box" />
                    <input
                      className="tinp"
                      value={t}
                      onChange={(e) => setItem(i, e.target.value)}
                      placeholder="A thing the group needs to do"
                    />
                    <button type="button" className="del-x" onClick={() => removeItem(i)} aria-label="Remove item">
                      ✕
                    </button>
                  </div>
                ))}
                <button type="button" className="rowbtn" onClick={() => setItems((prev) => [...prev, ''])}>
                  + Add another item
                </button>
              </span>
            </div>
          )}
          {editing && (
            <p className="hint">
              Editing updates the title, body and source. Checklist ticks are the group's shared state — managed from
              the notice.
            </p>
          )}
        </div>
        <div className="exp-foot">
          <span className="hint grow">
            {editing ? 'Changes are visible to everyone right away.' : 'Posts to everyone. You can pin it after.'}
          </span>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn accent"
            disabled={!canSave || save.isPending}
            onClick={() => save.mutate()}
          >
            {editing ? 'Save changes' : 'Post notice'}
          </button>
        </div>
      </div>
    </SheetModal>
  );
}
