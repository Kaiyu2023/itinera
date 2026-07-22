import type { Expense, Settlement } from '../../types';

/**
 * Pre-trip bookings only (the trip is in November; "today" in fixture-land is
 * late July). Prices are realistic for late-2026 Japan; fx frozen at entry
 * time, ~¥151/USD. Balances and suggested transfers are COMPUTED by the
 * ApiClient (LedgerView), never stored.
 */

export const expenses: Expense[] = [
  {
    // The one on-trip expense — logged during Day 2 (Nov 15), so it sorts to the
    // top of the stream above the "Pre-trip bookings" separator. Linked to the
    // Day-2 Shibuya stop (the dinner "poll winner"). Adding this record mutates
    // the pre-trip balances the mockup froze — see the ledger note in NEXT_PROMPT.
    id: 'e-gyukatsu',
    tripId: 't-japan26',
    paidBy: 'u-kaiyu',
    amount: 14400,
    currency: 'JPY',
    fxRateToBase: 0.0066,
    category: 'food',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'Gyukatsu Motomura · Day 2 dinner — Beef-cutlet dinner in Shibuya after the Scramble, the poll winner.',
    receiptPhotoUrl: null,
    linkedStopId: 's-d2-shibuya',
    createdAt: '2026-11-15T13:32:00Z',
  },
  {
    id: 'e-gracery',
    tripId: 't-japan26',
    paidBy: 'u-ann',
    amount: 226800,
    currency: 'JPY',
    fxRateToBase: 0.0066,
    category: 'lodging',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'Hotel Gracery Shinjuku — 3 twin rooms × 3 nights, prepaid non-refundable rate',
    receiptPhotoUrl: null,
    linkedStopId: 's-d1-hotel',
    createdAt: '2026-07-11T09:12:00Z',
  },
  {
    id: 'e-ryokan',
    tripId: 't-japan26',
    paidBy: 'u-makoto',
    amount: 165000,
    currency: 'JPY',
    fxRateToBase: 0.0066,
    category: 'lodging',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'Ichinoyu Honkan — 3 riverside rooms, kaiseki dinner + breakfast for 6',
    receiptPhotoUrl: null,
    linkedStopId: 's-d4-ryokan',
    createdAt: '2026-07-12T13:00:00Z',
  },
  {
    id: 'e-hostel',
    tripId: 't-japan26',
    paidBy: 'u-yusuke',
    amount: 54000,
    currency: 'JPY',
    fxRateToBase: 0.0066,
    category: 'lodging',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'Piece Hostel Sanjō — 6 dorm beds × 2 nights. The one booking within my means.',
    receiptPhotoUrl: null,
    linkedStopId: 's-d5-hostel',
    createdAt: '2026-07-13T21:40:00Z',
  },
  {
    id: 'e-teamlab',
    tripId: 't-japan26',
    paidBy: 'u-kaiyu',
    amount: 22800,
    currency: 'JPY',
    fxRateToBase: 0.0066,
    category: 'tickets',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'teamLab Planets — 6 × ¥3,800, 16:00 timed entry Nov 16',
    receiptPhotoUrl: null,
    linkedStopId: 's-d3-teamlab',
    createdAt: '2026-07-10T08:30:00Z',
  },
  {
    id: 'e-wifi',
    tripId: 't-japan26',
    paidBy: 'u-futaba',
    amount: 92,
    currency: 'USD',
    fxRateToBase: 1,
    category: 'other',
    split: { kind: 'even', participantIds: ['u-kaiyu', 'u-makoto', 'u-ryuji', 'u-ann', 'u-yusuke', 'u-futaba'] },
    note: 'Pocket wifi — 7 days unlimited + insurance, Haneda counter pickup',
    receiptPhotoUrl: null,
    linkedStopId: null,
    createdAt: '2026-07-14T03:22:00Z',
  },
];

export const settlements: Settlement[] = [
  // Ryuji chipping away at his share early
  {
    id: 'st-1',
    tripId: 't-japan26',
    fromUser: 'u-ryuji',
    toUser: 'u-ann',
    amount: 120,
    settledAt: '2026-07-15T12:00:00Z',
  },
];
