import type { ChecklistItem, Notice, NoticeCategory } from '../api/types';

/**
 * Shared prep vocabulary + checklist/notice status maths, used by the Prep tab
 * and by TripLayout's nav badge so "your open items" is computed one way.
 */

export const NOTICE_CATEGORY_META: Record<NoticeCategory, { label: string; emoji: string; color: string }> = {
  visa: { label: 'visa', emoji: '🛂', color: 'var(--color-primary)' },
  safety: { label: 'safety', emoji: '🛡️', color: 'var(--color-impossible)' },
  health: { label: 'health', emoji: '➕', color: 'var(--color-ok)' },
  money: { label: 'money', emoji: '💴', color: 'var(--color-accent)' },
  connectivity: { label: 'connectivity', emoji: '📶', color: 'var(--color-primary)' },
  packing: { label: 'packing', emoji: '🎒', color: 'var(--color-tight)' },
  custom: { label: 'custom', emoji: '✦', color: 'var(--color-text-muted)' },
};

export const NOTICE_CATEGORY_ORDER: NoticeCategory[] = ['visa', 'safety', 'health', 'money', 'connectivity', 'packing', 'custom'];

export function noticeStatus(n: Notice): 'active' | 'resolved' | 'archived' {
  return n.status ?? 'active';
}

/** Whether an item is still open *for the current user*. Group tasks (booked
    once for everyone) are open only until someone does them; each-mode tasks
    are open until you personally tick them. */
export function itemOpenForMe(item: ChecklistItem, meId: string): boolean {
  if (item.mode === 'group') return item.doneBy.length === 0;
  return !item.doneBy.includes(meId);
}

/** Whether an item is fully cleared by the whole group. */
export function itemGroupDone(item: ChecklistItem, memberCount: number): boolean {
  if (item.mode === 'group') return item.doneBy.length >= 1;
  return item.doneBy.length >= memberCount;
}

/** Whether the current user considers this item done (drives the row's green). */
export function itemDoneForMe(item: ChecklistItem, meId: string): boolean {
  if (item.mode === 'group') return item.doneBy.length >= 1;
  return item.doneBy.includes(meId);
}

/** The nav-badge / roll-up number: your personal outstanding items across the
    active notices. Resolved / archived notices don't count. */
export function personalOpenCount(notices: Notice[], meId: string): number {
  return notices
    .filter((n) => noticeStatus(n) === 'active')
    .reduce((sum, n) => sum + n.checklistItems.filter((i) => itemOpenForMe(i, meId)).length, 0);
}

/** Notices in display order: pinned first, then active before resolved,
    archived dropped. Preserves source order within each band. */
export function sortedNotices(notices: Notice[]): Notice[] {
  const band = (n: Notice) => {
    const s = noticeStatus(n);
    if (s === 'archived') return 3;
    if (s === 'resolved') return 2;
    return n.pinned ? 0 : 1;
  };
  return notices
    .map((n, i) => ({ n, i }))
    .filter(({ n }) => noticeStatus(n) !== 'archived')
    .sort((a, b) => band(a.n) - band(b.n) || a.i - b.i)
    .map(({ n }) => n);
}
