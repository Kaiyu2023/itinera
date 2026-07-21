import type { Notice } from '../../types';

export const notices: Notice[] = [
  {
    id: 'n-visa',
    tripId: 't-japan26',
    category: 'visa',
    title: 'Entry: visa-free for most of us — but do the paperwork',
    body:
      'Most passports get **90-day visa-free** entry to Japan — check yours at the MOFA site if unsure. ' +
      'Passport must be valid for the whole stay.\n\n' +
      'Register on **Visit Japan Web** before flying: it generates QR codes for immigration + customs ' +
      'and saves ~30 min of form-filling in the arrivals hall.',
    sourceUrl: 'https://www.mofa.go.jp/j_info/visit/visa/short/novisa.html',
    pinned: true,
    checklistItems: [
      { id: 'chk-vjw', text: 'Fill out Visit Japan Web (flight + hotel info)', doneBy: ['u-kaiyu', 'u-makoto'] },
      { id: 'chk-passport', text: 'Check passport expiry date', doneBy: ['u-kaiyu', 'u-makoto', 'u-ann', 'u-futaba'] },
    ],
  },
  {
    id: 'n-money',
    tripId: 't-japan26',
    category: 'money',
    title: 'Skip the JR Pass — it loses money on this route',
    body:
      'Since the 2023 price hike a 7-day JR Pass costs **¥50,000**. Our actual JR travel ' +
      '(Odawara→Kyoto Hikari ¥12,090 + Kyoto→Shin-Ōsaka ¥1,450 + airport legs) totals ' +
      '**≈ ¥21,000 per person** — point-to-point tickets win by a mile.\n\n' +
      '- Add a **Suica to your phone wallet** (Apple/Google) for everything local.\n' +
      '- Shinkansen seats open **exactly 1 month before** travel.\n' +
      '- Cash still matters at markets and small izakaya: 7-Eleven ATMs take foreign cards.',
    sourceUrl: null,
    pinned: true,
    checklistItems: [
      { id: 'chk-suica', text: 'Add Suica to phone wallet', doneBy: ['u-futaba', 'u-kaiyu', 'u-ann'] },
      { id: 'chk-hikari', text: 'Reserve Hikari seats + oversized luggage (opens Oct 18)', doneBy: [] },
      { id: 'chk-romancecar', text: 'Book Romancecar 09:00 Shinjuku→Hakone-Yumoto (opens Oct 17)', doneBy: [] },
    ],
  },
  {
    id: 'n-connectivity',
    tripId: 't-japan26',
    category: 'connectivity',
    title: 'Internet: one pocket wifi + eSIM backup',
    body:
      'Futaba booked a group **pocket wifi** (Haneda pickup, see ledger). It covers all 6 of us ' +
      'when we\'re together, but it dies by dinner if we stream — so anyone whose phone supports ' +
      'eSIM should install one as backup for the days we split up.',
    sourceUrl: null,
    pinned: false,
    checklistItems: [{ id: 'chk-esim', text: 'Install a Japan eSIM (optional backup)', doneBy: ['u-futaba'] }],
  },
  {
    id: 'n-onsen',
    tripId: 't-japan26',
    category: 'custom',
    title: 'Ryokan & onsen crash course',
    body:
      'For the Ichinoyu night (Day 4):\n\n' +
      '- **Wash thoroughly before** entering the bath; the small towel never touches the water.\n' +
      '- Tattoos: fine in the **private riverside bath** (book a slot at check-in), cover in shared baths.\n' +
      '- Kaiseki dinner is at **18:30 sharp** — courses are timed, lateness is genuinely rude.\n' +
      '- Yukata provided; left side over right (the reverse is for funerals).',
    sourceUrl: null,
    pinned: false,
    checklistItems: [
      { id: 'chk-kaiseki', text: 'Submit kaiseki dietary requirements (form due Nov 1)', doneBy: ['u-makoto'] },
    ],
  },
  {
    id: 'n-luggage',
    tripId: 't-japan26',
    category: 'packing',
    title: 'Luggage forwarding — don\'t drag suitcases through Hakone',
    body:
      'On Day 4 morning we **takkyūbin the big bags** from Hotel Gracery straight to Piece Hostel ' +
      'Kyoto (~¥2,200/bag, arrives next day — i.e. when we do). Everyone carries a small ' +
      'overnight bag for the ryokan.\n\n' +
      'Ask the Gracery front desk before 10:00; they handle the pickup.',
    sourceUrl: null,
    pinned: false,
    checklistItems: [{ id: 'chk-overnight', text: 'Pack a separate overnight bag for Hakone', doneBy: [] }],
  },
];
