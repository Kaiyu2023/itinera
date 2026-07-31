import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams, useSearchParams } from 'react-router';
import { useApi } from '../api/ApiProvider';
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

/* ── deep link: ?prep=new opens the composer, one-shot + self-stripping ── */
function readPrepDeepLink(params: URLSearchParams): boolean {
  return params.get('prep') === 'new';
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
  const queryClient = useQueryClient();
  const members = useMembers(tripId);
  const [params, setParams] = useSearchParams();

  const me = useQuery({ queryKey: ['me'], queryFn: () => api.getMe() });
  const notices = useQuery({
    queryKey: ['notices', tripId],
    queryFn: () => api.listNotices(tripId!),
    enabled: !!tripId,
  });

  const [composer, setComposer] = useState<{ mode: 'new' } | { mode: 'edit'; notice: Notice } | null>(null);
  const [openKebab, setOpenKebab] = useState<string | null>(null);
  const booted = useRef(false);
  if (!booted.current && notices.data) {
    booted.current = true;
    if (readPrepDeepLink(params)) {
      setComposer({ mode: 'new' });
      setParams(stripPrepDeepLink(params), { replace: true });
    }
  }

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

  if (notices.isLoading || !notices.data || !me.data) return <p className="muted">Loading “Before you go”…</p>;

  const meId = me.data.id;
  const memberIds = members.data?.map((u) => u.id) ?? [];
  const nameOf = (id: string) => members.byId.get(id)?.displayName ?? id;
  const ordered = sortedNotices(notices.data);
  const openCount = personalOpenCount(notices.data, meId, memberIds);
  const openItems = buildOpenItems(notices.data, meId, memberIds, nameOf).slice(0, 4);

  const kebabAction = (notice: Notice, action: string) => {
    setOpenKebab(null);
    if (action === 'edit') setComposer({ mode: 'edit', notice });
    else if (action === 'pin') patch.mutate({ noticeId: notice.id, patch: { pinned: !notice.pinned } });
    else if (action === 'copy') void navigator.clipboard?.writeText(notice.sourceUrl ?? '').catch(() => {});
    else if (action === 'resolve')
      patch.mutate({
        noticeId: notice.id,
        patch: { status: noticeStatus(notice) === 'resolved' ? 'active' : 'resolved' },
      });
    else if (action === 'archive') patch.mutate({ noticeId: notice.id, patch: { status: 'archived' } });
  };

  return (
    <div className="m4-tab notice-tab">
      <div className="m4-tab-head">
        <h1>Before you go</h1>
        <span className="spacer" />
        {/* Hidden under 720px, where the ＋ FAB already offers exactly this —
            two controls for one action, and the header one is the reachable-
            with-a-thumb loser of the pair. See `.notice-new` in index.css. */}
        <button type="button" className="btn accent notice-new" onClick={() => setComposer({ mode: 'new' })}>
          ＋ New notice
        </button>
      </div>

      {/* What's still open */}
      <div className="open-summary">
        <div className="oh">
          <span>🧳</span>
          <strong>What's still open</strong>
          <span className="badge money" style={{ marginLeft: 'auto' }}>
            {openCount} on your list
          </span>
        </div>
        {openItems.length === 0 ? (
          <div className="open-item">
            {/* "All set 🎉" is a claim about work that got done. With no notices
                at all nothing has been done — there is simply nothing to be
                outstanding — so don't congratulate anyone for it. */}
            <span className="dot" style={{ background: ordered.length ? 'var(--color-ok)' : 'var(--color-border)' }} />
            <span className="lbl">
              {ordered.length
                ? "Nothing outstanding — the group's all set. 🎉"
                : 'Nothing on your list yet — anything the group adds below lands here.'}
            </span>
          </div>
        ) : (
          <div className="open-items">
            {openItems.map((o) => (
              <div key={o.id} className="open-item">
                <span
                  className="dot"
                  style={{ background: o.urgent ? 'var(--color-impossible)' : 'var(--color-tight)' }}
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
      {ordered.length > 0 && (
        <div className="sub-anno">{ordered.length === 1 ? '1 notice' : `${ordered.length} notices`}</div>
      )}

      {ordered.length === 0 && (
        <div className="notice-empty">
          <span className="em">🧳</span>
          <strong>No notices yet</strong>
          <p>
            A notice is the group&rsquo;s shared answer to &ldquo;is that sorted?&rdquo; — a visa deadline, the cash
            everyone should carry, an eSIM to buy before you fly. Each one carries a checklist, so you can see who has
            done it and who hasn&rsquo;t.
          </p>
          <button type="button" className="btn accent" onClick={() => setComposer({ mode: 'new' })}>
            ＋ Write the first one
          </button>
        </div>
      )}

      {ordered.map((notice) => {
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
        return (
          <div key={notice.id} className={`card notice${resolved ? ' resolved' : ''}`}>
            <div className="notice-top">
              {notice.pinned && (
                <span className="pin" title="pinned">
                  📌
                </span>
              )}
              <strong>{notice.title}</strong>
              <span className="cat-badge" style={catBadgeStyle(meta.color)}>
                {meta.emoji} {meta.label}
              </span>
              {resolved && <span className="notice-status">resolved</span>}
              <NoticeKebab
                notice={notice}
                open={openKebab === notice.id}
                onToggle={() => setOpenKebab(openKebab === notice.id ? null : notice.id)}
                onAction={(a) => kebabAction(notice, a)}
                onClose={() => setOpenKebab(null)}
              />
            </div>
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
                  For {aud.map(nameOf).join(' & ')}
                  {iAmIn ? '' : ' — not on your list'}
                </span>
              </div>
            )}
            <div className="notice-body">
              {renderBody(notice.body)}
              {notice.sourceUrl && (
                <>
                  {' '}
                  <a href={notice.sourceUrl} target="_blank" rel="noreferrer" className="muted">
                    source ↗
                  </a>
                </>
              )}
            </div>
            {total > 0 && (
              <>
                <div className="prog">
                  <div className="prog-bar">
                    <div
                      className="prog-fill"
                      style={{ width: `${pct}%`, background: amber ? 'var(--color-tight)' : undefined }}
                    />
                  </div>
                  <span className="lab">
                    {subset ? 'these travellers' : 'group'}: {groupDone} / {total} done
                    {iAmIn ? ` · you: ${youDone} / ${total}` : ''}
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
        );
      })}

      <button type="button" className="m4-fab" onClick={() => setComposer({ mode: 'new' })} aria-label="New notice">
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
    </div>
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
  const group = item.mode === 'group';
  const done = itemDoneForMe(item, meId);
  const dueTxt = item.dueDate ? `(${group ? 'opens' : 'due'} ${formatDue(item.dueDate)})` : '';
  const coverage = (
    <span className="check-cov">
      {item.doneBy.length === 0 ? (
        <span className="none">no one yet</span>
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
          <span className="n">{group ? 'booked' : `${item.doneBy.length} / ${audSize}`}</span>
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
  open,
  onToggle,
  onAction,
  onClose,
}: {
  notice: Notice;
  open: boolean;
  onToggle: () => void;
  onAction: (a: string) => void;
  onClose: () => void;
}) {
  const resolved = noticeStatus(notice) === 'resolved';
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
        aria-label="Notice actions"
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
          <div className="kmenu" role="menu">
            <button type="button" role="menuitem" onClick={() => onAction('edit')}>
              ✎ Edit notice <span className="k-note">author + leaders</span>
            </button>
            <button type="button" role="menuitem" onClick={() => onAction('pin')}>
              📌 {notice.pinned ? 'Unpin' : 'Pin to top'}
            </button>
            <button type="button" role="menuitem" onClick={() => onAction('copy')} disabled={!notice.sourceUrl}>
              🔗 Copy source link
            </button>
            <div className="sep" role="separator" />
            <button type="button" role="menuitem" onClick={() => onAction('resolve')}>
              ✅ {resolved ? 'Reactivate' : 'Mark resolved'}
            </button>
            <button type="button" role="menuitem" className="danger" onClick={() => onAction('archive')}>
              🗄️ Archive
            </button>
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
  nameOf: (id: string) => string,
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
      if (item.doneBy.length === 0) detail = group ? "nobody's booked it yet" : "no one's ticked it";
      else if (audSize - item.doneBy.length <= 2) {
        detail = `only ${item.doneBy.map(nameOf).join(' & ')} so far (${item.doneBy.length} / ${audSize})`;
      } else detail = `${item.doneBy.length} / ${audSize} done`;

      let pill: string;
      let urgent = false;
      let sort: number;
      if (group && item.dueDate) {
        pill = `opens ${formatDue(item.dueDate)}`;
        sort = 1;
      } else if (!group && item.dueDate) {
        pill = `due ${formatDue(item.dueDate)}`;
        urgent = true;
        sort = 0;
      } else {
        pill = `${audSize - item.doneBy.length} pending`;
        sort = 2;
      }

      items.push({ id: item.id, title: item.text, detail, pill, urgent, sort });
    }
  }
  return items.sort((a, b) => a.sort - b.sort);
}

function formatDue(iso: string): string {
  return new Date(iso + 'T00:00:00').toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
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
