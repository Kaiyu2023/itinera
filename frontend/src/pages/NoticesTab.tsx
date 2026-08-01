import { Fragment, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/useApi';
import { useMembers } from '../components/hooks';
import type { ChecklistItem, Notice } from '../api/types';
import {
  NOTICE_CATEGORY_META,
  itemDoneForMe,
  itemGroupDone,
  isSubsetAudience,
  noticeAudience,
  noticeStatus,
  personalOpenCount,
  sortedNotices,
} from './noticesShared';
import { NoticeComposer } from './noticeComposer';
import { fillStyle } from '../lib/oklch';
import { useI18n } from '../i18n';
import { useOneShotDeepLink } from '../lib/useOneShotDeepLink';
import { SheetModal } from '../components/SheetModal';

/* ── deep link: ?prep=new opens the composer, one-shot + self-stripping ── */
function readPrepDeepLink(params: URLSearchParams): true | null {
  return params.get('prep') === 'new' ? true : null;
}
function stripPrepDeepLink(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  next.delete('prep');
  return next;
}

/**
 * "Before you go" — the prep overview (§ mockup E/F/G). A "what's still open"
 * roll-up, then the notices with their shared group checklists. The ＋ FAB /
 * header button opens the create-notice composer; the ⋯ kebab edits, pins,
 * resolves or archives a notice.
 */
export function NoticesTab() {
  const { tripId } = useParams();
  const api = useApi();
  const { locale, t: ui, formatDate, formatNumber } = useI18n();
  const queryClient = useQueryClient();
  const members = useMembers(tripId);
  const [params, setParams] = useSearchParams();

  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const trip = useQuery({
    queryKey: ['trip', tripId],
    queryFn: () => api.getTrip(tripId!),
    enabled: !!tripId,
  });
  const notices = useQuery({
    queryKey: ['notices', tripId],
    queryFn: () => api.listNotices(tripId!),
    enabled: !!tripId,
  });

  const [composer, setComposer] = useState<{ mode: 'new' } | { mode: 'edit'; notice: Notice } | null>(null);
  const [openKebab, setOpenKebab] = useState<string | null>(null);
  const [archiveTarget, setArchiveTarget] = useState<Notice | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const [feedback, setFeedback] = useState<{ kind: 'success' | 'error'; message: string } | null>(null);
  useOneShotDeepLink({
    ready: !!notices.data,
    searchParams: params,
    setSearchParams: setParams,
    read: readPrepDeepLink,
    strip: stripPrepDeepLink,
    onMatch: () => setComposer({ mode: 'new' }),
  });

  const toggle = useMutation({
    mutationFn: ({ noticeId, itemId }: { noticeId: string; itemId: string }) =>
      api.toggleChecklistItem(noticeId, itemId),
    // Optimistic — the tick lands instantly, rolls back if the write fails.
    onMutate: async ({ noticeId, itemId }) => {
      await queryClient.cancelQueries({ queryKey: ['notices', tripId] });
      const prev = queryClient.getQueryData<Notice[]>(['notices', tripId]);
      const meId = me.data?.id;
      if (prev && meId) {
        queryClient.setQueryData<Notice[]>(
          ['notices', tripId],
          prev.map((n) =>
            n.id !== noticeId
              ? n
              : {
                  ...n,
                  checklistItems: n.checklistItems.map((i) =>
                    i.id !== itemId
                      ? i
                      : {
                          ...i,
                          doneBy: i.doneBy.includes(meId) ? i.doneBy.filter((u) => u !== meId) : [...i.doneBy, meId],
                        },
                  ),
                },
          ),
        );
      }
      return { prev };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.prev) queryClient.setQueryData(['notices', tripId], ctx.prev);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: ['notices', tripId] }),
  });

  const patch = useMutation({
    mutationFn: ({ noticeId, patch }: { noticeId: string; patch: Parameters<typeof api.updateNotice>[1] }) =>
      api.updateNotice(noticeId, patch),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['notices', tripId] }),
  });

  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(null), 4000);
    return () => window.clearTimeout(timer);
  }, [feedback]);

  if (notices.isLoading || !notices.data || !me.data || !trip.data)
    return <p className="muted">{ui('prep.loading')}</p>;

  const meId = me.data.id;
  const memberIds = members.data?.map((u) => u.id) ?? [];
  const nameOf = (id: string) => members.byId.get(id)?.displayName ?? id;
  const ordered = sortedNotices(notices.data);
  const archivedNotices = notices.data.filter((notice) => noticeStatus(notice) === 'archived');
  const displayNotices = showArchived ? [...ordered, ...archivedNotices] : ordered;
  const isLeader = trip.data.members.some((member) => member.userId === meId && member.role === 'leader');
  const openCount = personalOpenCount(notices.data, meId, memberIds);
  const formatNames = (ids: string[]) =>
    locale === 'en'
      ? ids.map(nameOf).join(' & ')
      : new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(ids.map(nameOf));
  const openItems = buildOpenItems(notices.data, meId, memberIds, formatNames, ui, formatDate, formatNumber).slice(
    0,
    4,
  );

  const copySource = async (notice: Notice) => {
    try {
      if (!notice.sourceUrl || !navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(notice.sourceUrl);
      setFeedback({ kind: 'success', message: ui('prep.copy.success') });
    } catch {
      setFeedback({ kind: 'error', message: ui('prep.copy.failure') });
    }
  };

  const kebabAction = (notice: Notice, action: string) => {
    setOpenKebab(null);
    if (action === 'edit') setComposer({ mode: 'edit', notice });
    else if (action === 'pin') patch.mutate({ noticeId: notice.id, patch: { pinned: !notice.pinned } });
    else if (action === 'copy') void copySource(notice);
    else if (action === 'resolve')
      patch.mutate({
        noticeId: notice.id,
        patch: { status: noticeStatus(notice) === 'resolved' ? 'active' : 'resolved' },
      });
    else if (action === 'archive') setArchiveTarget(notice);
    else if (action === 'restore') patch.mutate({ noticeId: notice.id, patch: { status: 'active' } });
  };

  return (
    <div className="m4-tab notice-tab">
      <div className="m4-tab-head">
        <h2>{ui('prep.title')}</h2>
        <span className="spacer" />
        {/* Hidden under 720px, where the ＋ FAB already offers exactly this —
            two controls for one action, and the header one is the reachable-
            with-a-thumb loser of the pair. See `.notice-new` in index.css. */}
        <button type="button" className="btn accent notice-new" onClick={() => setComposer({ mode: 'new' })}>
          ＋ {ui('prep.newNotice')}
        </button>
      </div>

      {/* What's still open */}
      <div className="open-summary">
        <div className="oh">
          <span aria-hidden>🧳</span>
          <strong>{ui('prep.open.title')}</strong>
          <span className="badge money" style={{ marginLeft: 'auto' }}>
            {ui('prep.open.count', { count: formatNumber(openCount) })}
          </span>
        </div>
        {openItems.length === 0 ? (
          <div className="open-item">
            {/* "All set 🎉" is a claim about work that got done. With no notices
                at all nothing has been done — there is simply nothing to be
                outstanding — so don't congratulate anyone for it. */}
            <span
              className="dot"
              style={{ background: ordered.length ? 'var(--color-ok)' : 'var(--color-border)' }}
              aria-hidden
            />
            <span className="lbl">{ui(ordered.length ? 'prep.open.noneComplete' : 'prep.open.noneYet')}</span>
          </div>
        ) : (
          <div className="open-items">
            {openItems.map((o) => (
              <div key={o.id} className="open-item">
                <span
                  className="dot"
                  style={{ background: o.urgent ? 'var(--color-impossible)' : 'var(--color-tight)' }}
                  aria-hidden
                />
                <span className="lbl">
                  <b>{o.title}</b> — {o.detail}
                </span>
                <span className={`due ${o.urgent ? 'urgent' : 'soon'}`}>{o.pill}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* The annotation that used to live here ("Notices — pinned first, each
          with a group checklist and progress") was a line out of the spec: it
          described the component to a reviewer rather than telling a traveller
          anything. A count is a fact they can use; the 📌 pins already say what
          the ordering is. And with nothing in the list there is nothing to
          annotate, so it goes away entirely. */}
      {(ordered.length > 0 || archivedNotices.length > 0) && (
        <div className="notice-list-tools">
          <div className="sub-anno">
            {ui(ordered.length === 1 ? 'prep.notice.one' : 'prep.notice.many', {
              count: formatNumber(ordered.length),
            })}
          </div>
          {archivedNotices.length > 0 && (
            <button
              type="button"
              className="notice-archive-toggle"
              aria-expanded={showArchived}
              onClick={() => setShowArchived((shown) => !shown)}
            >
              {ui(showArchived ? 'prep.archive.hide' : 'prep.archive.show', {
                count: formatNumber(archivedNotices.length),
              })}
            </button>
          )}
        </div>
      )}

      {ordered.length === 0 && archivedNotices.length === 0 && (
        <div className="notice-empty">
          <span className="em" aria-hidden>
            🧳
          </span>
          <strong>{ui('prep.empty.title')}</strong>
          <p>{ui('prep.empty.description')}</p>
          <button type="button" className="btn accent" onClick={() => setComposer({ mode: 'new' })}>
            ＋ {ui('prep.empty.addFirst')}
          </button>
        </div>
      )}

      {displayNotices.map((notice, index) => {
        const meta = NOTICE_CATEGORY_META[notice.category];
        const aud = noticeAudience(notice, memberIds);
        const audSize = aud.length;
        const iAmIn = aud.includes(meId);
        const subset = isSubsetAudience(notice);
        const total = notice.checklistItems.length;
        const groupDone = notice.checklistItems.filter((i) => itemGroupDone(i, audSize)).length;
        const youDone = notice.checklistItems.filter((i) => itemDoneForMe(i, meId)).length;
        const pct = total ? (groupDone / total) * 100 : 0;
        const amber = notice.checklistItems.some((i) => i.mode === 'group' && i.doneBy.length === 0 && i.dueDate);
        const resolved = noticeStatus(notice) === 'resolved';
        const archived = noticeStatus(notice) === 'archived';
        const canManage = isLeader || notice.createdBy === meId;
        return (
          <Fragment key={notice.id}>
            {archived && index === ordered.length && (
              <div className="notice-archived-head">
                <strong>{ui('prep.archive.section')}</strong>
                <span>{ui('prep.archive.sectionHint')}</span>
              </div>
            )}
            <div className={`card notice${resolved ? ' resolved' : ''}${archived ? ' archived' : ''}`}>
              <div className="notice-top">
                {notice.pinned && (
                  <span className="pin" title={ui('prep.pinned')}>
                    📌
                  </span>
                )}
                <strong>{notice.title}</strong>
                <span className="cat-badge" style={catBadgeStyle(meta.color)}>
                  {meta.emoji} {ui(meta.labelKey)}
                </span>
                {resolved && <span className="notice-status">{ui('prep.resolved')}</span>}
                {archived && <span className="notice-status archived">{ui('prep.archived')}</span>}
                {(canManage || !!notice.sourceUrl) && (
                  <NoticeKebab
                    notice={notice}
                    canManage={canManage}
                    open={openKebab === notice.id}
                    onToggle={() => setOpenKebab(openKebab === notice.id ? null : notice.id)}
                    onAction={(a) => kebabAction(notice, a)}
                    onClose={() => setOpenKebab(null)}
                  />
                )}
              </div>
              <div className="notice-author">{ui('prep.author', { name: nameOf(notice.createdBy) })}</div>
              {subset && (
                <div className="notice-aud">
                  <span className="heads">
                    {aud.map((id) => {
                      const u = members.byId.get(id);
                      if (!u) return null;
                      return (
                        <span
                          key={id}
                          className={`avatar xs${id === meId ? ' me' : ''}`}
                          style={fillStyle(u.avatarColor)}
                          title={u.displayName}
                        >
                          {u.displayName[0]}
                        </span>
                      );
                    })}
                  </span>
                  <span className="lbl">
                    {ui('prep.audience.for')} {formatNames(aud)}
                    {!iAmIn && <> {ui('prep.audience.notYours')}</>}
                  </span>
                </div>
              )}
              <div className="notice-body">
                {renderBody(notice.body)}
                {notice.sourceUrl && (
                  <>
                    {' '}
                    <a href={notice.sourceUrl} target="_blank" rel="noreferrer" className="muted">
                      {ui('prep.source')}
                    </a>
                  </>
                )}
              </div>
              {!archived && total > 0 && (
                <>
                  <div className="prog">
                    <div
                      className="prog-bar"
                      role="progressbar"
                      aria-label={ui('prep.progress.label')}
                      aria-valuemin={0}
                      aria-valuemax={total}
                      aria-valuenow={groupDone}
                      aria-valuetext={ui('prep.progress.done', {
                        done: formatNumber(groupDone),
                        total: formatNumber(total),
                      })}
                    >
                      <div
                        className="prog-fill"
                        style={{ width: `${pct}%`, background: amber ? 'var(--color-tight)' : undefined }}
                      />
                    </div>
                    <span className="lab">
                      {ui(subset ? 'prep.progress.travellers' : 'prep.progress.group')}:{' '}
                      {ui('prep.progress.done', { done: formatNumber(groupDone), total: formatNumber(total) })}
                      {iAmIn && (
                        <> · {ui('prep.progress.you', { done: formatNumber(youDone), total: formatNumber(total) })}</>
                      )}
                    </span>
                  </div>
                  <div className="checklist">
                    {notice.checklistItems.map((item) => (
                      <ChecklistRow
                        key={item.id}
                        item={item}
                        meId={meId}
                        audSize={audSize}
                        iAmIn={iAmIn}
                        membersById={members.byId}
                        onToggle={() => toggle.mutate({ noticeId: notice.id, itemId: item.id })}
                      />
                    ))}
                  </div>
                </>
              )}
            </div>
          </Fragment>
        );
      })}

      <button
        type="button"
        className="m4-fab"
        onClick={() => setComposer({ mode: 'new' })}
        aria-label={ui('prep.newNotice')}
      >
        ＋
      </button>

      {composer && (
        <NoticeComposer
          tripId={tripId!}
          mode={composer.mode}
          notice={composer.mode === 'edit' ? composer.notice : undefined}
          onClose={() => setComposer(null)}
        />
      )}

      {archiveTarget && (
        <NoticeArchiveDialog
          notice={archiveTarget}
          busy={patch.isPending}
          onClose={() => setArchiveTarget(null)}
          onConfirm={() =>
            patch.mutate(
              { noticeId: archiveTarget.id, patch: { status: 'archived' } },
              { onSuccess: () => setArchiveTarget(null) },
            )
          }
        />
      )}

      {feedback && (
        <div
          className={`prep-feedback ${feedback.kind}`}
          role={feedback.kind === 'error' ? 'alert' : 'status'}
          aria-live={feedback.kind === 'error' ? 'assertive' : 'polite'}
        >
          <span aria-hidden>{feedback.kind === 'success' ? '✓' : '!'}</span>
          {feedback.message}
        </div>
      )}
    </div>
  );
}

function NoticeArchiveDialog({
  notice,
  busy,
  onClose,
  onConfirm,
}: {
  notice: Notice;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { t: ui } = useI18n();
  return (
    <SheetModal onClose={onClose}>
      <div
        className="exp-modal cand-reject-modal prep-archive-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="prep-archive-title"
        aria-describedby="prep-archive-copy"
        aria-busy={busy}
      >
        <span className="cand-reject-grip" aria-hidden />
        <header className="cand-reject-head">
          <span className="cand-reject-mark" aria-hidden>
            <svg viewBox="0 0 24 24">
              <path d="M4.5 7.5h15M6.5 7.5v11h11v-11M8.5 4.5h7M9.5 11.5h5" />
            </svg>
          </span>
          <span className="cand-reject-title">
            <span>{ui('prep.archive.eyebrow')}</span>
            <h2 id="prep-archive-title">{ui('prep.archive.title', { title: notice.title })}</h2>
          </span>
          <button type="button" className="cand-reject-close" onClick={onClose} aria-label={ui('common.close')}>
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="m6 6 12 12M18 6 6 18" />
            </svg>
          </button>
        </header>
        <div className="cand-reject-body" id="prep-archive-copy">
          <p>{ui('prep.archive.impact')}</p>
          <p className="cand-reject-reversible">
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="M8 8H4v-4M4.5 8a8 8 0 1 1-.3 7" />
            </svg>
            <span>{ui('prep.archive.reversible')}</span>
          </p>
        </div>
        <footer className="cand-reject-actions">
          <button type="button" className="btn cand-reject-cancel" onClick={onClose} disabled={busy}>
            {ui('prep.archive.keep')}
          </button>
          <button type="button" className="btn danger cand-reject-confirm" onClick={onConfirm} disabled={busy}>
            {ui('prep.archive.confirm')}
          </button>
        </footer>
      </div>
    </SheetModal>
  );
}

/* ── checklist row (whole-row tap target) ── */
function ChecklistRow({
  item,
  meId,
  audSize,
  iAmIn,
  membersById,
  onToggle,
}: {
  item: ChecklistItem;
  meId: string;
  audSize: number;
  /** Whether the current user is in the notice's audience — off-audience rows
      are read-only (no personal checkbox obligation). */
  iAmIn: boolean;
  membersById: Map<string, { displayName: string; avatarColor: string }>;
  onToggle: () => void;
}) {
  const { t: ui, formatDate, formatNumber } = useI18n();
  const group = item.mode === 'group';
  const done = itemDoneForMe(item, meId);
  const dueTxt = item.dueDate
    ? `(${ui(group ? 'prep.check.opens' : 'prep.check.due')} ${formatDate(item.dueDate, {
        month: 'short',
        day: 'numeric',
      })})`
    : '';
  const coverage = (
    <span className="check-cov">
      {item.doneBy.length === 0 ? (
        <span className="none">{ui('prep.check.noOne')}</span>
      ) : (
        <>
          <span className="heads">
            {item.doneBy.map((id) => {
              const u = membersById.get(id);
              if (!u) return null;
              return (
                <span
                  key={id}
                  className={`avatar xs${id === meId ? ' me' : ''}`}
                  style={fillStyle(u.avatarColor)}
                  title={u.displayName}
                >
                  {u.displayName[0]}
                </span>
              );
            })}
          </span>
          <span className="n">
            {group ? ui('prep.check.booked') : `${formatNumber(item.doneBy.length)} / ${formatNumber(audSize)}`}
          </span>
        </>
      )}
    </span>
  );
  const text = (
    <span className="check-text">
      {item.text}
      {dueTxt && (
        <span className="hint" style={{ display: 'inline', marginLeft: 5 }}>
          {dueTxt}
        </span>
      )}
    </span>
  );
  // Off-audience: you can see it, but it isn't your obligation to tick.
  if (!iAmIn) {
    return (
      <div className="check-item not-mine">
        <span className="check-box ghost" aria-hidden />
        {text}
        {coverage}
      </div>
    );
  }
  return (
    <button
      type="button"
      /* The row *is* the checkbox — `.check-box` is a painted span, not an
         <input>, so without these a screen reader read a ticked row and an
         unticked one identically ("button, Sort the Suica cards"). The visible
         ✓ is decorative once the role carries the state. */
      role="checkbox"
      aria-checked={done}
      className={`check-item tappable${done ? ' done' : ''}`}
      onClick={onToggle}
    >
      <span className={`check-box${group ? ' group' : ''}`} aria-hidden>
        {done ? '✓' : ''}
      </span>
      {text}
      {coverage}
    </button>
  );
}

/* ── kebab menu ──
   The trigger and the menu live in one component so the menu can hand focus
   back to the exact ⋯ it came from. Previously the only way out was clicking
   the invisible full-screen catcher: Escape did nothing, so a keyboard user who
   opened the menu had no dismissal at all, and tabbing past the last entry
   walked off into the page behind it with the menu still up. */
function NoticeKebab({
  notice,
  canManage,
  open,
  onToggle,
  onAction,
  onClose,
}: {
  notice: Notice;
  canManage: boolean;
  open: boolean;
  onToggle: () => void;
  onAction: (a: string) => void;
  onClose: () => void;
}) {
  const { t: ui } = useI18n();
  const resolved = noticeStatus(notice) === 'resolved';
  const archived = noticeStatus(notice) === 'archived';
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // Stop it reaching a modal behind us — nothing else is up when a kebab is,
      // but the app's convention is that the topmost surface consumes Escape.
      e.stopPropagation();
      onClose();
      trigger.current?.focus();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  return (
    <span className="kebab-wrap">
      <button
        ref={trigger}
        type="button"
        className="kebab"
        aria-label={ui('prep.actions')}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={onToggle}
      >
        ⋯
      </button>
      {open && (
        <>
          <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 39 }} />
          {/* `role="menu"` with plain buttons inside is a broken contract: the
              menu promises menuitem children and a screen reader counts none. */}
          <div className="kmenu" role="menu" aria-label={ui('prep.actions')}>
            {canManage && !archived && (
              <>
                <button type="button" role="menuitem" onClick={() => onAction('edit')}>
                  ✎ {ui('prep.action.edit')} <span className="k-note">{ui('prep.action.permissions')}</span>
                </button>
                <button type="button" role="menuitem" onClick={() => onAction('pin')}>
                  📌 {ui(notice.pinned ? 'prep.action.unpin' : 'prep.action.pin')}
                </button>
              </>
            )}
            {notice.sourceUrl && (
              <button type="button" role="menuitem" onClick={() => onAction('copy')}>
                🔗 {ui('prep.action.copy')}
              </button>
            )}
            {canManage && (notice.sourceUrl || !archived) && <div className="sep" role="separator" />}
            {canManage &&
              (archived ? (
                <button type="button" role="menuitem" onClick={() => onAction('restore')}>
                  ↩ {ui('prep.action.restore')}
                </button>
              ) : (
                <>
                  <button type="button" role="menuitem" onClick={() => onAction('resolve')}>
                    ✅ {ui(resolved ? 'prep.action.reactivate' : 'prep.action.resolve')}
                  </button>
                  <button type="button" role="menuitem" className="danger" onClick={() => onAction('archive')}>
                    🗄️ {ui('prep.action.archive')}
                  </button>
                </>
              ))}
          </div>
        </>
      )}
    </span>
  );
}

/* ── "what's still open" roll-up items ── */
interface OpenItem {
  id: string;
  title: string;
  detail: string;
  pill: string;
  urgent: boolean;
  sort: number;
}
function buildOpenItems(
  notices: Notice[],
  meId: string,
  memberIds: string[],
  formatNames: (ids: string[]) => string,
  ui: ReturnType<typeof useI18n>['t'],
  formatDate: ReturnType<typeof useI18n>['formatDate'],
  formatNumber: ReturnType<typeof useI18n>['formatNumber'],
): OpenItem[] {
  const items: OpenItem[] = [];
  for (const n of notices) {
    if (noticeStatus(n) !== 'active') continue;
    // Only items on my personal list count — skip notices I'm not an audience of.
    const aud = noticeAudience(n, memberIds);
    if (!aud.includes(meId)) continue;
    const audSize = aud.length;
    for (const item of n.checklistItems) {
      const open = item.mode === 'group' ? item.doneBy.length === 0 : item.doneBy.length < audSize;
      if (!open) continue;
      const group = item.mode === 'group';
      let detail: string;
      if (item.doneBy.length === 0) detail = ui(group ? 'prep.open.nobodyBooked' : 'prep.open.noOneTicked');
      else if (audSize - item.doneBy.length <= 2) {
        detail = `${ui('prep.open.only')} ${formatNames(item.doneBy)} ${ui('prep.open.soFar')} (${formatNumber(item.doneBy.length)} / ${formatNumber(audSize)})`;
      } else
        detail = ui('prep.open.done', {
          done: formatNumber(item.doneBy.length),
          total: formatNumber(audSize),
        });

      let pill: string;
      let urgent = false;
      let sort: number;
      if (group && item.dueDate) {
        pill = ui('prep.open.opens', {
          date: formatDate(item.dueDate, { month: 'short', day: 'numeric' }),
        });
        sort = 1;
      } else if (!group && item.dueDate) {
        pill = ui('prep.open.due', {
          date: formatDate(item.dueDate, { month: 'short', day: 'numeric' }),
        });
        urgent = true;
        sort = 0;
      } else {
        pill = ui('prep.open.pending', { count: formatNumber(audSize - item.doneBy.length) });
        sort = 2;
      }

      items.push({ id: item.id, title: item.text, detail, pill, urgent, sort });
    }
  }
  return items.sort((a, b) => a.sort - b.sort);
}

/**
 * The category badge: a 15% wash of the category hue, with the label written in
 * the *same* hue. That is the bug — a 15% tint of a colour is, by construction,
 * nearly the colour of the page, so the label had almost nothing to sit against:
 * light `money` measured 2.62:1, dark `visa` / `connectivity` 2.14:1, dark
 * `money` 4.10:1. (The category hues are fixed brand values, not scheme-aware
 * tokens — `#4a5d8f` is a light-theme blue printed onto a dark page.)
 *
 * Keep the wash, which is what carries the category at a glance, and pull the
 * *label* toward the page's own ink: 45% of --color-text, which darkens in the
 * light scheme and lightens in the dark one, so one expression fixes both. The
 * hue survives at 55% — still visibly blue / vermilion / green.
 *
 * Measured against each badge's own composited tint (Chromium, both schemes):
 *   light  visa 7.85  safety 10.92  health 7.72  money 5.33  connectivity 7.85  packing 7.63
 *   dark   visa 4.96  safety  6.33  health 6.99  money 6.18  connectivity 4.96  packing 8.13
 * `custom` takes the branch above (muted ink on the sunken surface) and was
 * already passing: 4.75:1 light, 4.77:1 dark. Worst case overall 4.75:1.
 */
function catBadgeStyle(color: string) {
  if (color === 'var(--color-text-muted)')
    return { background: 'var(--color-surface-sunken)', color: 'var(--color-text-muted)' };
  return {
    background: `color-mix(in srgb, ${color} 15%, transparent)`,
    color: `color-mix(in srgb, ${color} 55%, var(--color-text))`,
  };
}

/** Minimal inline markdown for notice bodies: **bold** + line breaks. */
function renderBody(body: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const re = /\*\*([^*]+)\*\*/g;
  let last = 0;
  let key = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(body))) {
    if (m.index > last) nodes.push(body.slice(last, m.index));
    nodes.push(<b key={key++}>{m[1]}</b>);
    last = re.lastIndex;
  }
  if (last < body.length) nodes.push(body.slice(last));
  return nodes;
}
