# Itinera — Visual Design

Status: **v3, built and reviewed** · 2026-07-30 · author: Kaiyu Huang + Claude

Companion to `DESIGN.md`, which covers architecture. This one covers
**presentation**: how a trip plan is depicted, and how the colour system
should carry meaning.

Every number in §3 and §4 is produced by the script in Appendix A, and the
severity ramp by the search in Appendix B. Nothing here is asserted from taste
alone where it could be measured.

v1 was a proposal. v2 is what survived building it — §6 answers the questions
v1 left open, §7 records what shipped, and the blockquotes through §3–§4 mark
where the measurements contradicted the proposal. The most useful of those:
§4.5's `C ≥ 0.19` alarm floor is unreachable in sRGB.

---

## 1. The diagnosis

The current UI works, is well-structured, and feels generic. The cause is
specific and worth stating precisely, because it points at the fix:

> **The plan is described rather than depicted.**

The domain model is unusually rich. Every leg has a mode, a duration, a
distance and a feasibility verdict. Every day has a window, a timezone, a
sunrise and a sunset. Every place has a rating, a price level, opening
hours and photos. Almost all of it renders as *text inside a chip the same
size as every other chip*.

In the milestone-2 mockup, `2h30`, `1h40` and `1h` are three visually
identical rows. A four-hour temple visit and a five-minute walk occupy the
same vertical space. The day is "87% full" because a badge says so, not
because it looks full. Sunset is a number in a strip, not a horizon.

Chips-in-cards-in-tabs is the universal SaaS idiom precisely because it is
the safe way to display *anything*. It flattens every dimension to equal
visual weight. That is the boredom — not a colour problem, a **depiction**
problem.

A corollary worth noticing: the Plan tab has a Timeline/Map toggle because
*neither view is complete*. A view you have to leave is a view that is
missing something.

---

## 2. Four directions

These are not mutually exclusive. The recommendation in §2.5 combines them.

### A. The ribbon — a trip as a transit diagram

Straighten the route into one continuous line. Stops are stations, legs are
the track between them, coloured by mode and *sized* by duration. Days are
segments of the line.

```
DAY 06 ─────────────────────────────────── Kyoto
  ①━━━━━━━━━━━━②━━━━━━━━③━━━④
  Fushimi       Kiyomizu  Yosh Bamboo
  ▓▓▓▓▓ 2h30    ▓▓▓ 1h40  ▓1h ▓▓▓▓ 2h30
       ══train 35══  ══bus 50══  ‥walk 5‥
```

The lineage is old and literal: the Roman *itineraria* — the app's
namesake — were exactly this. So was the Peutinger Table, and so were AAA
TripTiks in the 1920s. Sequence preserved, geography sacrificed. It is the
correct diagram for "a route through time," and nobody in this product
category uses it.

Strongest at **whole-trip scale**, where the current UI has nothing at all:
seven days as one unbroken line taken in at a glance.

Data needed: `Leg.mode`, `Leg.durationMin`, `Stop.seq`, `Day.date`. All
present.

### B. Time as physical space

> Revised in §9.8 — the proportional scale moved off the cards and onto the
> axis. Everything below about the *sky* behind the column still stands; the
> claim that a block's height is its duration does not.

Vertical extent proportional to actual duration. Then move the daylight
gradient from a strip above the timeline to *behind the column itself*.

```
07:00  ┌──────────────┐  ░ dawn
       │ Fushimi Inari│  ▒
       │              │  ▒
       │        2h30  │  ▓
09:45  └──────────────┘  ▓
       ≈ 35 min  train   █
10:45  ┌──────────────┐  █
       │ Kiyomizu-dera│  █  full daylight
       │        1h40  │  █
       └──────────────┘  █
              ⋮
16:45  ─── sunset ─────  ▓
       ┌──────────────┐  ▒
       │ Bamboo Grove │  ░  ← in the dark
```

Two things fall out for free:

1. The fixture note *"the 14:45 Arashiyama arrival leaves little daylight
   for the grove"* stops being prose and becomes something you **see** —
   the block sits below the sunset line.
2. A tight day is a column with no gaps. An impossible day visibly
   **overflows** its window.

At which point `TIGHT · 87%` and the feasibility notes list can be deleted,
because the shape already said it. Deleting UI is the strongest evidence a
depiction is working.

Data needed: `DaylightStrip` and `lib/sun.ts` already compute all of it.

### C. Map-first

The map becomes the page — full-bleed, no card, no border — with the plan
riding over it as a draggable sheet. Selecting a day flies the camera and
dims the rest of the route.

Honest assessment: **highest confidence, lowest distinctiveness.** The map
is currently a card, inside a tab, inside a shell — three layers of chrome
away from the content, which is why it reads as a widget rather than a
place. Fixing that is real and worth doing. But the result looks like
Airbnb, and Airbnb is part of the 99%.

Best treated as a correction to fold into A and B rather than a direction
of its own: unbox the map, kill the toggle, let the ribbon be drawn on it.

### D. The journal — an interface that ages with the trip

Drop the dashboard idiom. Each day is a spread: a large date numeral, one
photo bleeding off the edge, stops as a numbered marginal list.

What makes this more than a skin is **already in the type system**.
`TripStatus` runs `dreaming → planning → booked → ongoing → done`. Let the
interface change character as the trip matures:

| status | character |
|---|---|
| `dreaming` | moodboard — loose, photo-heavy, no times, candidates scattered |
| `planning` | working surface — structure appears, feasibility starts to matter |
| `booked` | printed itinerary — tight, typographic, confident, times in bold |
| `ongoing` | today only — phone-sized, glanceable, everything else collapsed |
| `done` | scrapbook — photos win, times recede, the ledger settles |

One app that is five different things depending on where the trip is in its
life. Nothing in the category does this, and it is built on a field already
stored.

### 2.5 Recommendation

**B for the day, A for the trip, D's status idea layered over both, C
folded in as a correction.**

- **B is the core.** Making the plan legible is the app's actual job, and B
  converts the richest data from text into perception.
- **A supplies the whole-trip view** that currently does not exist, and is
  the one that will make people ask what this app is.
- **D costs little conceptually** — mostly restraint applied differently per
  status — and is the most differentiating item on the list.
- **C is a fix, not a direction.** Unbox the map; delete the toggle.

None of this touches the API contract. `planMapGeometry.ts` and `sun.ts`
already do the mathematics. This is a rendering change.

---

## 3. The colour system as it stands

Verdict: **well chosen, badly assigned.** The problems are in the mapping
from colour to meaning, not in the colours.

### 3.1 What is right

- The palette has a point of view — warm off-white rather than grey, dusk
  blue, torii vermilion "used sparingly." Better than a default ramp.
- **Text contrast passes everywhere**, in both themes:

  | | | ratio |
  |---|---|---|
  | light text | `#26251f` on `#ffffff` | 15.36 |
  | light text-muted | `#6f6c63` on `#ffffff` | 5.25 |
  | dark text | `#e8e6e1` on `#22242a` | 12.44 |
  | dark text-muted | `#9a978e` on `#22242a` | 5.31 |

- The `--accent` / `.accent-scope` derivation is genuinely subtle and
  correct. The comment in `tokens.css` — that `var()` inside a custom
  property resolves where it is *defined*, not where it is *used*, so a
  derivation made only on `:root` freezes to the brand colour — describes a
  bug most systems ship.

### 3.2 Three hexes carry nine meanings

```
#d97b4f  =  --color-accent  =  --color-unreasonable  =  --color-kind-food
#2f9e6e  =  --color-ok      =  --color-kind-activity
#4a5d8f  =  --color-primary =  --color-kind-sight
```

An orange element means *"interactive"* or *"this leg is unreasonable"* or
*"this is a restaurant."* In the milestone-2 mockup, the `MEAL` chip on
Arashiyama Yoshimura and the `50 min — lunch-hour bus; optimistic` warning
directly below it are the same family of orange. One is a category, one is
an alarm.

**Root cause:** two orthogonal axes — *what kind of place* and *how
feasible* — compete for a single channel. This is a defect, not a
preference.

### 3.3 The severity scale is not a ramp

| step | hex | luminance |
|---|---|---|
| ok | `#2f9e6e` | 0.262 |
| tight | `#d9a13b` | **0.406** ← brightest |
| unreasonable | `#d97b4f` | 0.295 |
| impossible | `#c4453b` | 0.163 |

Severity climbs; lightness goes up and back down. `tight` is brighter than
both its neighbours, so at a glance it shouts louder than `impossible`.

A severity scale must be monotonic in *something* — lightness, chroma,
weight — so that "worse" reads as "more" pre-attentively.

> **Resolved (v2).** The shipped ramp is monotonic in **chroma**, and `ok` was
> removed from it entirely — a day that fits now renders no verdict badge at
> all. Silence is the signal, which frees the whole lightness range for the
> three steps that are genuinely alarms and is what makes the dichromat
> separation below achievable. Appendix B has the search and the numbers.
>
> | step | light | dark |
> |---|---|---|
> | tight | `#865900` | `#ffe1a2` |
> | unreasonable | `#852b00` | `#ffae81` |
> | impossible | `#6e000c` | `#ff6e80` |

### 3.4 It collapses for roughly 8% of men

Viénot–Brettel–Mollon simulation of the feasibility scale:

| step | normal | deuteranopia |
|---|---|---|
| ok | `#2f9e6e` | `#898970` |
| tight | `#d9a13b` | `#b4b435` |
| unreasonable | `#d97b4f` | `#9e9e49` |
| impossible | `#c4453b` | `#7d7d33` |

**`ok` vs `impossible`: 1.21:1.** The two ends of the severity scale are
effectively the same colour. Green → amber → orange → red is the canonical
red-green trap, and it is sitting on the alarm channel — the one place
where failure is expensive.

Stop kinds are worse: **`activity` vs `transit` is 1.03:1.** Identical.

### 3.5 Dark mode was done for surfaces, never for hues

The `@media (prefers-color-scheme: dark)` block redefines bg, surface,
border, text and shadows — but **not one brand, semantic or kind hue**. The
same hex serves both themes. As non-text UI (map dots, timeline nodes,
chips) these need 3:1:

| token(s) | hex | on `#fbfaf8` | on `#22242a` |
|---|---|---|---|
| primary = sight | `#4a5d8f` | 6.19 ok | **2.40 FAIL** |
| primary-strong | `#37476e` | 8.80 ok | **1.69 FAIL** |
| accent = unreasonable = food | `#d97b4f` | **2.92 FAIL** | 5.09 ok |
| ok = activity | `#2f9e6e` | 3.23 ok | 4.61 ok |
| tight | `#d9a13b` | **2.21 FAIL** | 6.73 ok |
| impossible | `#c4453b` | 4.72 ok | 3.15 ok |
| lodging | `#7b5bd2` | 4.72 ok | 3.15 ok |
| transit | `#8a8577` | 3.53 ok | 4.21 ok |

`kind-sight` is the most common stop type; in dark mode its dots and nodes
are close to invisible. `primary-strong` at 1.69:1 is the worst in the
system. And the accent itself fails on the light page — note this is
measured against `--color-bg` `#fbfaf8`, the actual painted background, not
against pure white, which flatters it to 3.05.

> **Fixed (v2).** Every hue now has a dark-mode variant, generated rather than
> picked: each keeps the angle the palette originally chose and has its
> lightness rebuilt per theme, capped at its own chroma so the character
> survives. (Capping everything at the ceiling instead would have pushed
> `--color-primary` from C=0.075 to C=0.130 and turned a muted dusk blue into a
> saturated one — a redesign nobody asked for.) `e2e/palette.spec.ts` measures
> all eleven tokens against the real painted background in both themes, so this
> table cannot silently come back.
>
> One class of failure the original audit missed entirely: tokens painted as a
> **solid fill with a label on top** — `.btn.approve`, `.verb.*`, `.check-box`.
> That ink was hardcoded `#fff`, which on the dark theme's `--color-ok` reads
> **2.04:1**. It is now `--color-ink-on-fill`, which flips with the theme.

### 3.6 The avatar palette is not in the system

```
avatar  #6b5bd2   vs   --color-kind-lodging  #7b5bd2     one digit apart
avatar  #4fb06d   vs   --color-ok            #2f9e6e
avatar  #3b6fd4   vs   --color-primary       #4a5d8f
```

Near-misses read worse than either matching or clearly differing: the eye
registers the mismatch without being able to name it.

It also now lives in **three** places — the TS fixtures, the neighbourhood
of `tokens.css`, and `AVATAR_PALETTE` in `crates/api/src/routes/me.rs`. A
palette change currently requires a backend deploy, which is a sign the
backend holds a presentation concern it should not.

### 3.7 The strategic problem

Warm neutral background, near-black text, one accent used sparingly. That
is the *correct* formula, and it is the formula behind Stripe, Linear,
Notion and every template. It is tasteful, which is exactly why it is
invisible. It commits to nothing.

---

## 4. Photo-derived accent

Proposal: derive the accent from the trip's cover photo rather than fixing
it in the palette. The plumbing already exists — `--accent` is a single
knob with `.accent-scope` re-derivation, and `Trip.accentColor` is already
nullable and per-trip. The change is *who supplies the value*.

### 4.1 The trap: photos do not contain one colour

Extraction yields a *distribution*, and both obvious summaries are wrong:

- **Dominant colour** (k-means, most pixels) — for a landscape photo this is
  sky, foliage and stone. The dominant colour of a beautiful photo is
  usually mud, not the red maple.
- **Most saturated colour** — a blown highlight, a tourist's jacket, a sign.
  Unstable: recrop the photo and the theme changes.

But the deeper problem is not *which* colour. It is that **an extracted
colour carries no lightness guarantee**, and the system already assumes
one:

```css
--accent-contrast: #fff;  /* text/glyphs on solid accent (fixtures are mid-tone) */
--accent-strong: color-mix(in srgb, var(--accent) 82%, #1c2030);
```

The comment says it — *"fixtures are mid-tone."* Feed that a pale sand
`#e8dcc0` from a beach photo and white glyphs vanish. Feed it near-black
from a night shot and `--accent-strong` is a no-op. Every derivation
downstream inherits the failure.

### 4.2 Extract a hue, synthesise the colour

Take only the **angle** from the photo. Rebuild lightness and chroma in
OKLCH, where `L` is perceptual — unlike HSL, in which `hsl(60 100% 50%)`
and `hsl(240 100% 50%)` are both "50% lightness" and look nothing alike.

Sweeping the full hue circle at fixed lightness (Appendix A §4):

- light accent `L=0.52` — **worst case 4.97:1** at hue 150
- dark accent `L=0.72` — **worst case 5.88:1** at hue 345
- every hue clears the 3:1 floor, in both themes, with no exceptions

Contrast stops being something audited per trip and becomes a property of
the construction.

Chroma must be **clamped, not fixed**. At `L=0.52`, cyan holds only
`C=0.094` before leaving sRGB while purple holds `C=0.268` — a 2.8×
difference. A constant chroma silently produces out-of-gamut colours for
the cyan trips.

And `L` has a ceiling, because `--accent-contrast` is hardcoded to white
(Appendix A §5):

| accent L | worst ratio vs `#fff` | verdict |
|---|---|---|
| 0.50 | 5.65 | ok |
| 0.52 | 5.17 | ok |
| 0.55 | 4.56 | ok — but exactly at the 4.5 floor |
| 0.58 | 4.02 | large text only |

`L=0.52` is the recommended value: comfortably safe, still vivid.

### 4.3 The recipe

```
photo → extract dominant hue h, with a chroma confidence
      → if chroma below floor (fog, snow, night, B&W): fall back to brand
      → store h

light theme:  --accent = oklch(0.52  min(0.13, Cmax(0.52, h))  h)
dark theme:   --accent = oklch(0.72  min(0.13, Cmax(0.72, h))  h)
```

`--accent-strong` and `--accent-soft` keep working unchanged, and
`--accent-contrast: #fff` becomes *provably* safe rather than a comment
hoping the fixtures stay mid-tone.

Prior art: Android's Material You does exactly this — extract a source hue,
synthesise a tonal palette at controlled lightness. A trodden path, not a
gamble.

### 4.4 Contract change: store the hue, not the hex

Replace `accentColor: string | null` with `accentHue: number | null`.

A hex is a **decision** — it bakes in lightness, chroma and one theme. A hue
is a **fact about the photo**. Storing the fact means:

- the backend never owns a colour decision;
- the frontend synthesises per theme, and dark mode comes free;
- a redesign is a CSS change, not a deploy plus a data migration.

This is `DESIGN.md` §1's ports principle applied to colour, and it retires
the `AVATAR_PALETTE`-in-Rust problem by the same argument. Worth doing
before the frontend freezes and these types become `openapi.yaml`.

### 4.5 What stays genuinely hard

**Colour design gets harder in exactly one way, and easier in several.**

The hard part: no more hand-tuning. Every decision must be a *rule that
holds for all 360 hues*. This is where photo-theming attempts usually die —
tuned against three demo trips, broken by the fourth.

Then three specific problems:

**1. Collision with the alarm channel.** If the accent can be any hue, some
trip gets a red-orange accent and every button reads as a warning. Two ways
out:

- *Reserve an arc* — exclude the accent from red-through-amber. Simple, but
  it punishes exactly the autumn-foliage trip this app is being built
  against.
- *Separate by chroma* — accent capped at `C ≤ 0.13`, alarms floored at
  `C ≥ 0.19`. "Louder means wrong" becomes the rule, hue stays free, and
  the Kyoto trip keeps its maple red.

**Recommended: separate by chroma.** It keeps the whole circle available and
stacks with the monotonic lightness ramp (§3.3) and a shape change at
`impossible` — three redundant signals instead of one.

> **Correction (v2, measured).** The `C ≥ 0.19` floor is **not reachable in
> sRGB**. Building the ramp forced the question and Appendix B answers it: in
> the amber→red arc the gamut allows at most `C ≈ 0.14` at the lightnesses that
> also satisfy contrast, peaking around L=0.55–0.65 and collapsing above L=0.80.
> The shipped ramp runs `0.106 → 0.132 → 0.138` (light) and `0.086 → 0.112 →
> 0.177` (dark).
>
> So chroma separates the *top* of the ramp from the accent and not the bottom —
> `tight` is genuinely quieter than a button, which is arguably right, since
> `tight` is not an emergency. What actually carries the separation is the
> redundancy the recommendation already called for: alarms are the only tokens
> rendered as a filled badge with a glyph, and they are the only ones on the
> monotonic severity ramp. Chroma is one of three signals, not the load-bearing
> one. The recommendation survives; the number in it does not.

**2. Which photo, and how often it changes.** Trip-level only. A per-*place*
accent makes the chrome shimmer while scrolling the timeline. Let a place's
colour live inside its own photo card and never leak outward. Compute once
at upload and store it — deriving at render time costs work on every paint
and flashes brand colour before the real theme lands.

**3. Photos with no hue to give.** Snow, fog, night, black-and-white: the
extracted chroma is near zero and the hue is numerical noise. Needs a
confidence floor and a clean fallback. Never theme from a hue you cannot
trust.

### 4.6 Why it composes with §2

Direction B wants **daylight** to drive the page. This wants the **photo** to
drive the accent. Those are different channels — sun gives lightness and
temperature to the substrate, photo gives hue to the interactive layer — so
they stack rather than fight.

Both derive from something physically true about the trip. That is the
actual cure for §3.7: a palette that cannot be accused of arbitrary taste,
because nobody chose it.

---

## 5. Consolidated recommendations

1. **Give colour one job: alarm.** Feasibility owns the loud channel; stop
   kind gets a glyph. The timeline already prints `VISIT` and `MEAL` as
   text, so kind colour is redundant there; on the map a fork/bed/camera
   glyph separates kinds better than five dots, two of which are
   indistinguishable to a deuteranope (§3.4).
2. **Make severity a ramp, not a wheel.** One hue family, steps of increasing
   chroma, plus a shape change at `impossible`. Never hue alone.
   *(v2: "and decreasing lightness" held in the light theme but not the dark
   one — see Appendix B. Chroma is the channel that survives both.)*
3. **Give the hues dark-mode variants.** Four tokens currently fail the 3:1
   floor in one theme or the other (§3.5).
4. **Let the environment own the page.** Daylight from `lib/sun.ts` drives
   background lightness and warmth.
5. **Let the photo own the accent**, as a hue synthesised in OKLCH (§4).
6. **Let status own the treatment** (§2, direction D).
7. **One source for avatars.** Fold them into `tokens.css` as real palette
   members; the backend stores a chosen colour, never a derivation table.

Order of work: 1–3 are corrections and independent of any direction in §2.
4–7 are the redesign.

---

## 6. Decisions

These were open in draft v1. All three are now settled and built.

### 6.1 Kind surrenders colour — and gets a glyph

**Yes**, but narrower than §5.1 first proposed: kind gives up colour *where it
competes with feasibility*, which is the plan. It keeps it in the ledger, where
expense categories have no alarm axis to collide with.

What settled it is that the trade §5.1 worried about — losing at-a-glance
category scanning — was already lost. At five categories the kind hues were
never scannable for a dichromat: `activity` vs `transit` measured **1.03:1**
under simulated deuteranopia, i.e. the same colour. A glyph works for everyone,
so this is not a sacrifice of legibility for hygiene; it is a straight gain.

### 6.2 The environment is a dial, not a switch

The honest answer to "is an environment-driven page too much for a `booked`
itinerary read on a train" is **yes** — so amplitude became a number keyed to
`TripStatus`, not a feature that is either on or off:

| status | amplitude | reading |
|---|---|---|
| `dreaming` | 1.0 | a mood, not a plan — let the sky do what it likes |
| `planning` | 0.75 | |
| `booked` | 0.3 | legible in bad light; the shape still reads |
| `ongoing` | 0.45 | enough to tell you dusk is coming while you are out in it |
| `done` | 0.6 | a memory, warmer than a plan |

This is direction D reduced to its load-bearing part. The daylight still drives
the page; how loudly is the trip's business. One CSS custom property
(`--env-amplitude`), one lookup, no forked rendering.

### 6.3 Hue extraction runs in the frontend, from the stored colour

Not Rust-at-upload, and not yet. The frontend already receives
`Trip.accentColor`, so it derives the hue from that and re-synthesises. That
buys the entire guarantee with **no backend work and no contract change**.

The sequencing matters: the frontend consumes *a hue* either way. When there is
a real upload pipeline and the backend starts storing `accentHue` (§4.4), the
render path is unchanged — only the source of the number moves. Adding an image
-decoding dependency to the backend before there is anything to decode would be
paying for a port with no adapter behind it.

---

## 7. What was built

Branch `feat/visual-design-plan-depiction`. Directions **B** and **A** in full,
the colour corrections 1–3, the photo-derived accent 5, and the load-bearing
part of D.

| | where |
|---|---|
| **B** — day as physical space | `src/components/DayCanvas.tsx` |
| **A** — the trip ribbon | `src/components/TripRibbon.tsx` |
| kind as a shape | `src/components/KindGlyph.tsx` |
| OKLCH synthesis | `src/lib/oklch.ts` |
| accent + env amplitude | `src/theme/useTripTheme.ts` |
| shared sky model | `src/lib/daylight.ts` |
| sun / moon / weather glyphs | `src/components/SkyGlyph.tsx` |
| forecast vs climatology | `src/lib/weather.ts` |
| regenerated tokens | `src/theme/tokens.css` |

Three things the build changed about the design:

1. **Blocks do not grow to fit their content.** If the scale bends, the picture
   lies. A 20-minute stop is 32 pixels tall and shows only its name; detail
   degrades through `sz-full` / `sz-med` / `sz-min` tiers instead. *(Reversed in
   §9.8: the tiers were the bill for the scale, and it came due. Blocks still do
   not grow to fit their content — they are all one height now.)*
2. **A long stop earns its photo.** Strict proportionality left a 2h30 visit as
   a large empty rectangle. Filling it with the place's photo turns duration
   into something worth looking at — and is direction D's "photo bleeding off
   the edge" arriving through the back door. *(Now every stop earns one, since
   every stop has the room. §9.8.)*
3. **"After dark" is judged on when a stop *ends*.** The first cut tested the
   start time, which missed the exact case the design was built to show: the
   Arashiyama grove begins in daylight at 14:45 and runs out of it. Caught by
   `e2e/depiction.spec.ts`, not by looking.

Deliberately **not** done, and still open:

- ~~**The map still colours by stop kind**~~ — done in the sweep pass. A pin's
  *fill* is now the worst verdict of the legs touching it, its centre is the
  `KindGlyph` path (one set of paths, two renderers), and the route is drawn per
  leg in that leg's verdict. `DAY_COLORS` is deleted.
- **Direction C** — unboxing the map and deleting the Timeline/Map toggle. The
  toggle is load-bearing for the desktop map default that the add-stop preview
  flow depends on.
- **Canvas basemap labels can still collide with DOM tags.** The declutter pass
  measures real rects, but only for things in the DOM; making canvas type
  participate means the basemap returning its label boxes.
- **`accentColor` → `accentHue`** (§4.4) is still the frontend deriving the hue
  from the stored hex. See §6.3 for why that ordering is deliberate.

---

## 8. The sky, and what kind of claim the weather is

v1 shipped the day canvas with a sky behind it and a sunset marker on top of
it. Both were wrong in ways that only showed up once real fixtures were on the
screen, and the review that found them is worth recording because the failures
are not the ones the design predicted.

### 8.1 One opacity cannot serve both ends of a ramp

The sky was a five-stop gradient of opaque hexes under a blanket
`opacity: 0.62`. That single number has to be right for full daylight *and*
for midnight, and there is no value that is:

| | at 0.62 |
|---|---|
| daylight `#dce8f2` over the cream page | washes the page out; text loses ~15% of its contrast for no information |
| night `#353c66` over the same page | composites to `rgb(109 113 131)` — a mid slate. `56 min spare` in `--color-text-muted` measured **1.9:1** on it |

So the band was simultaneously too strong where it meant nothing and too weak
where it meant something, and it arrived on screen as one flat lavender slab
that was neither day nor night. It is now alpha per stop — night `0.82`,
full daylight `0.30` — with the layer opacity reserved for the environment
dial. Three consequences fell out of that:

1. **The dark theme needs the ramp inverted, not re-tinted.** On a cream page
   night is the strong band; on a `#191a1e` page it is *daylight* that has to
   lift off the substrate, and painting the light theme's colours over a dark
   page reads as the sun going *down* at dawn. `lib/daylight.ts` therefore emits
   `var(--sky-*)` into the gradient string and owns only the geometry;
   `tokens.css` owns what the light looks like, and declares two different
   ramps. A module that compiles hexes into a gradient can only ever serve one
   theme.
2. **The amplitude dial (§6.2) may not reach zero here.** "This stop happens
   after dark" is a fact about the plan, not atmosphere. `.dc-sky` is
   `calc(0.55 + 0.45 * var(--env-amplitude))` — the dial modulates the sky, it
   cannot delete a signal.
3. **Anything printed onto the night band carries its own surface.** Not a
   lighter ink: an ink chosen for a 0.82 wash is wrong the moment the amplitude
   dial moves. Chips and pills, so the contrast is independent of the dial.

### 8.2 The horizon is three things, and only one of them belongs on the plan

v1 drew sunset as a 2px solid rule at `z-index: 3` with an opaque
`SUNSET 16:35` pill pinned to its right end — over the top of the itinerary.
On Day 3 it struck straight through the middle of teamLab Planets' note. There
was no sunrise marker at all, so the day had an end and no beginning.

Thinning the line to a 1px dash and keeping it above the column does not fix
this; it is the same bug in a finer pen. The reframing that does: **a horizon
belongs to the sky, not to the itinerary**, and the three jobs it was doing at
once should be done by three separate things.

| job | where it went |
|---|---|
| *when* | a drawn token in the gutter — a sun, a crescent with a star — carrying the time. Nothing can collide with it out there. |
| *the line across the day* | 1px dashed, `z-index: 1`, **behind** the column. Visible in the gaps, hidden by the cards. |
| *which part of this stop is in the light* | the card itself. A block that straddles sunset gets `--dusk-at` — the fraction of its own height where the sun goes down — and darkens from exactly there. |

The third is the one that matters, and it is strictly better than a line. The
Arashiyama note — "the 14:45 arrival leaves little daylight for the grove" — is
a claim about *proportion*, and a rule across the card can only say "somewhere
in here". The gradient says 84%.

A sun and a crescent-with-a-star also replace `sunset 16:35 ☀ ↓`, which was
three symbols doing one job, one of them the sun announcing the end of the sun.

### 8.3 A legend is not a timestamp

The trip ribbon got the same tokens, and immediately exposed a second design
error: v1 laid each day out on *elapsed engaged time*, packing stop against leg
with the gaps squeezed out. Every day therefore started at the same instant,
which makes the one thing the ribbon exists for — comparing days — impossible,
and leaves no x-axis to hang a sky on. Days are now on their own clock.

The celestial tokens then needed a different rule from the canvas's. Pinned to
the horizons, they almost never appear: every day in the fixture opens after
sunrise, so no day ever got a sun. On the ribbon they mark the **middle of the
lit stretch and the middle of the dark one** — a legend for the band rather
than a timestamp. Precision is the day canvas's job; "is this a long evening or
a long morning" is the ribbon's.

### 8.4 Weather: the honest version is the useful one

> **Mock visual reference, not an approved production data flow.** These states
> explain the existing UI prototype. Real mode disables direct weather requests
> and purges its browser cache until the privacy, provider, network, response,
> rate, and cache controls in [`SECURITY.md`](SECURITY.md) receive separate
> review.

Weather is environment, and the prototype treats it separately from the trip
backend — `lib/sun.ts` computes sunrise from a coordinate and a date for the
same reason. Its current Open-Meteo experiment requires no project key, but that
does not remove third-party privacy, availability, terms, or rate constraints.

The interesting constraint is that **a forecast is exactly what this app cannot
show**. A forecast reaches about two weeks; every trip in the fixtures is
months out. The available answer is climate — what the same week actually did
in each of the last four years — and it is a genuinely useful thing to pack
against. It is also a *different claim*, and printing a four-year median in a
forecast's ink is a lie told to someone packing a bag. So `source` is part of
the model, `typical` renders differently from `forecast` (dotted rule, the word
itself, a tooltip naming the years), and the code path that straddles the
horizon runs both and lets the forecast win only where it exists.

Three engineering notes, all learned the hard way:

- **Sixteen headless contexts hitting the archive at once earns a 429.** Four
  parallel requests per mounted plan is impolite to a service that asks for
  nothing in return. They now run sequentially, and a year that fails just
  coarsens the median.
- **The prototype currently persists results.** It uses `localStorage` for seven
  days for `typical` and three hours for a forecast. That behavior is explicitly
  forbidden in real mode: frontend cutover removes it and purges the legacy key.
  A future opt-in offline design must partition by verified identity and define
  expiry, logout, and device-loss behavior before persistence returns.
- **It may never block, retry hard, or throw.** `e2e/weather.spec.ts` asserts
  the plan renders identically with Open-Meteo unreachable, because that is the
  state it will be in on the train the plan was written for.

### 8.5 What the screenshots caught that the tests did not

Worth listing, because every one of them is a class of bug that no assertion in
the suite was ever going to find:

- The photo scrim ramped to **fully transparent** across the body's own height,
  which put the note — the longest and lowest line — printed directly onto a
  photograph of a building.
- The action row was styled as glass, with `backdrop-filter`, *inside* the
  element that already carried the scrim. It was blurring an opaque surface, so
  it rendered as a plain white bar. Moving it out to be a sibling is what made
  it glass.
- The ribbon's warning legs used the class `warn`, which the app already uses
  for advisory boxes — an 8px-padded tinted brick drawn across a 3px line.
- `.dc-tail` is a `<button>`, and setting `background-image` without
  `background-color: transparent` left the UA's `buttonface` painting an opaque
  light slab over the night sky.
- `visit` and `lodging` were both a pitched roof.

---

## 9. States you could read and not set, and one material

### 9.1 A state the product does not really have

Two things the type system declared and the UI would only ever describe.
`TripStatus` had five values and rendered one of them as a static pill;
dark mode existed solely as `@media (prefers-color-scheme: dark)`, so on a
laptop pinned to light there was no way to see it. A state you can read and
cannot set is a state the product does not really have.

Both are now controls, and both are the *same* object that used to be the
readout — the hero pill became the phase picker, because it is already where
you look to find out what phase you are in. The phase ladder is drawn as a
ladder rather than a dropdown, since the phases are ordered and the order is the
information, and every rung stays live: `booked → planning` is a real thing that
happens when a booking falls through, and that is the moment you least want the
app arguing with you.

The payoff for putting the picker there is immediate, which is the argument for
putting it there. `--env-amplitude` is keyed to status (§6.2), so choosing a
phase changes how loud the whole page is allowed to be — dreaming 1.0, booked
0.3.

The theme switch has one structural cost worth recording. Every dark rule now
keys off `<html data-theme>` instead of the media query, which is what lets an
in-app switch beat the OS; the price is that the media query needs resolving in
script, and that is paid by a blocking inline script in `<head>` before any CSS
applies, so choosing dark does not buy you a white flash on the way in.
`useColorScheme` had to move onto the same signal — it read the media query
directly, so a page darkened by hand still got an accent synthesised at L=0.52
for a cream substrate.

### 9.2 One opacity cannot serve both ends of a ramp — the second time

§8.1 fixed this in the token values and left the bug in place one layer up.
The wash still carried a blanket `opacity: calc(0.55 + 0.45 * amplitude)`, which
means every alpha in `tokens.css` was a claim about a layer nobody ever saw at
full strength: a `booked` trip painted the whole ramp at 0.685. And a ramp at
0.685 is not a quieter day and night, it is a day and a night that have moved
toward each other until they are the same mid grey.

Measured, that put light-mode night at **L\*≈38** — "late afternoon in bad
weather" — and dark-mode daylight at **L\*≈21**, which is the same evening the
night band was already showing. Both complaints in one cause.

The fix is a division of labour, not a number:

- **The wash is the fact.** Day and night is a property of the plan, and status
  is allowed to make facts quieter, not to erase the difference between them. It
  now ranges 0.86–1.0.
- **The scene is the atmosphere.** The sun, the clouds and the star field are
  the part that is genuinely decoration, so they are the part that dims —
  0.3–1.0.

With the dial off it, the tokens can finally be honest: night composes to
**L\*≈18** and day to **L\*≈87** on cream, inverting to **L\*≈4** and
**L\*≈40** on a dark page. `e2e/glass.spec.ts` asserts the *composed* values
rather than the token values, because composed is the only form of the colour
anyone has ever complained about.

### 9.3 Glass, and where it is not allowed to go

`backdrop-filter` is the load-bearing property, not the tint: an 18px blur
averages a wide enough neighbourhood that even a busy photograph arrives behind
the words as a flat field, which is what lets the pane be transparent at all.
Two rules keep it a material instead of an effect:

1. **It only goes over something worth seeing through** — a photo, the sky, the
   page scrolling under a bar. Glass over a flat surface is a slightly wrong
   surface.
2. **It is never the only thing separating text from a picture.** Every tint is
   picked against the worst substrate in the app — the night band — not the
   average one. That is where the ribbon's stop chips get 68%: the lowest tint
   at which `--color-text` still clears 4.5:1 over night in either theme.

The consumers: the top bar and the mobile chrome (sticky over a scrolling page),
the ribbon's day plate and stop chips, the stop card's action pill, the day
scrubber, and the unplanned-time button.

That last one is the interesting deletion. Unplanned time was a 45° barber pole,
which read as "disabled" — the opposite of what free time in a plan means — then
dotted graph paper, which was friendlier but still *something*. Something is the
wrong answer. This is the part of the day where nothing is happening, so the
honest picture of it is the sky, showing through. It is now the only place on
the canvas where you can see the weather uninterrupted, which is a better
argument for filling it than any texture was.

### 9.4 The ribbon takes the whole screen

> Narrowed in §9.7: this is now a phone treatment. The mechanism below is
> unchanged; what changed is when it is allowed to fire.

The ribbon is the one view whose job is "how long is this, and how does it
compare to that", so the amount of it on screen at once *is* how much of that
job it does — and a 928px column was cutting a week in half for no reason. It is
now full-bleed: `width: 100vw` with `margin-inline: calc(50% - 50vw)`, which
lands it at x=0 regardless of the column's width. Days are the same width they
always were; there are simply more of them visible (7 at 1920px, against 3.5).

Two things this needed:

- **A clip.** 100vw includes the vertical scrollbar, so it overshoots the
  viewport by 15px and adds a horizontal scrollbar. `overflow-x: clip` on the
  shell — not on `<main>`, whose box is the thing full-bleed is escaping; and
  `clip` rather than `hidden`, since `hidden` would make it a scroll container
  and break the sticky day scrubber inside it.
- **A gutter.** `--bleed-pad` puts the first day back under the page's own left
  margin, so the ribbon reads as running *out* to the edge rather than as a
  detached band, and a mask fades both ends so a guillotined day says "this
  continues" rather than "this is the end of it".

The label moved with it. Day number, city, clock and weather used to be two rows
of bare text stacked above and below the band — twice the height, and a caption
rather than a label. They are now one glass plate on the day's own sky. When the
band is too short to hold everything the **clock** is what goes first, and not
because it is less useful: the band's extent *is* the clock, drawn to scale, so
it is the one fact on the plate already being said twice. The weather is said
nowhere else.

### 9.5 Deleting the scrollbar

The trough was the **third** control on this axis. The day chips below index the
same seven days; clicking a day is the same navigation again. A 6px groove under
a picture of a week is chrome describing something the picture already says.

What replaced it is three parts, each doing one job:

- **The mask fade** says *there is more*. It was already there for the
  guillotine problem; it turns out to be the whole of the "you can scroll this"
  signal.
- **Two chevrons**, on glass, sitting over the fade — the thing that says there
  is more and the thing that fetches it in the same place. Each is `disabled`
  rather than hidden when there is nothing that way, so the row never changes
  width and a keyboard user is never offered a direction that does not exist.
  Hidden entirely under `@media (hover: none)`, where they would only be two
  objects sitting on top of the first and last day.
- **Drag-to-pan**, which is what everyone tries first. Pointer travel past 4px
  promotes the press to a pan, and the `click` that follows is swallowed —
  otherwise a drag that happens to end over Thursday selects Thursday.

The **wheel is deliberately left alone.** Mapping `deltaY` to horizontal scroll
is the obvious next move and it is wrong here: this is a full-width band you
must scroll past to reach the day below it, so hijacking the wheel trades a rare
need for a constant annoyance. Shift+wheel and trackpad gestures already work.

### 9.6 The same argument, one view down

> Narrowed in §9.7, on the same terms as §9.4. The clock-on-a-rail below is
> unaffected and is the part that mattered.

The day canvas got the ribbon's treatment: **the sky is the width of the room,
not the width of the itinerary.** The column of cards stays in the 960px reading
measure — that number is about how far the eye travels across a line of prose,
which a wider sky does not change — and only the weather behind it goes wall to
wall. Reaching back past both the page margin and the canvas's own 52px gutter
is what `--page-inset` and `--dc-gutter` are for; the former is defined once,
because two surfaces bleed now and a sky that starts somewhere other than the
day it belongs to is worse than no sky.

That move broke the clock, which is the interesting part. The hour labels lived
in the gutter as `--color-text-muted` on the page, and that worked *only*
because the sky stopped at the column's edge. Once it ran underneath them the
labels were sitting on a surface that is a different colour at every hour of the
day. So they got a rail — glass, full height, the same answer as the ribbon's
`.rb-rail` and for the same reason: everything printed along an axis that
changes colour needs one substrate.

Two contrast findings from that rail, both worth keeping:

- **Muted ink cannot survive glass over night.** `--color-text-muted` is
  calibrated against the page (5.25:1 on white); on a 62% pane over the night
  band it composes to **2.4:1**, and no tint that still counts as glass gets it
  back — 93% surface would, and 93% is a surface. The hours are full-strength
  ink now, at 0.66rem and unbolded, which is where the quietness comes from
  instead.
- **The horizon tokens kept their own opaque chip.** `--sky-rise-line` and
  `--sky-set-line` were picked against the page at 4.35:1 and 7.77:1, and a
  translucent pane over a sky that changes hue by the hour cannot promise either
  number. The one place in the plan where a hue means something other than
  feasibility is not the place to start guessing.

The day scrubber went full-bleed in the same pass. Its fade-to-page gradient was
drawn at the column's width, and against a wall-to-wall sky that band read as an
opaque rectangle patched over the middle of the weather.

### 9.7 The bleed is a phone treatment

§9.4 and §9.6 argued for width and got it, and on a desktop the answer was
wrong. The argument was about the *ribbon* — more days on screen is more of what
the ribbon is for — and it was then applied to the sky, the scrubber and the
canvas because they all sit on the same axis. But those three do not gain
anything from width. They gain a **second left edge.**

The 960px measure exists so that everything on the page starts in the same
place: the hero, the tabs, the day heading, the first stop. A band running the
full width of a 1280px room no longer starts there. It is not part of the page,
it is a stripe behind it with the reading measure floating on top, and the eye
has to re-find the itinerary's left edge every time it crosses one. Three of
them stacked — ribbon, scrubber, sky — is three re-finds per screen.

On a 390px phone none of that is true, because the column *is* the viewport.
There, 16px of page margin either side of a picture of the weather is two
stripes of nothing, and taking it wall to wall costs the layout nothing and buys
the only width there is. So the bleed survives, at the size where it was always
the real answer:

```css
:root {                       /* off */
  --bleed-w: 100%;
  --bleed-mx: 0px;
  --page-inset: 0px;
}
@media (max-width: 719px) {   /* on */
  --bleed-w: 100vw;
  --bleed-mx: calc(50% - 50vw);
  --page-inset: var(--space-4);
}
```

Three tokens, one media query, and the three surfaces stop knowing which mode
they are in. `--page-inset` — how far a bled surface pushes its *contents* back
in so the first day still lines up with the heading — falls out to `0` when
nothing went out, so every formula that consumed it collapses correctly instead
of needing a second branch. The canvas needs one extra token (`--dc-bleed-w`)
only because its width is `100% + gutter` rather than `100%`.

Two things fell out of narrowing it:

- **The fade had to become conditional.** Inside the column the first day starts
  at the track's own left edge, so an unconditional mask dissolved the front of
  Monday — announcing days before the start of the trip. The fade means *there is
  more this way*, which is exactly the two booleans `usePan` already computes for
  the chevrons; they now draw the mask too (`.more-back` / `.more-fwd`). Written
  to the DOM in the scroll handler rather than rendered, because `dragging` is
  set imperatively on the same element and a rendered `className` would wipe it
  mid-drag.
- **The sky got corners.** A band that stops inside the page needs to look like
  it meant to. `border-radius: var(--radius-md)` with `overflow: hidden`, and the
  radius is switched off with the bleed — an edge-to-edge band has no corners to
  round.

#### The clock was not centred in its rail

Separately, and visible in the same screenshots: the hour labels were hung off a
`right` offset with shrink-to-fit width, so their *right* edge lined up with the
rail and the left edge landed wherever the digits happened to end — 3px of glass
on one side of `07:00` and 13px on the other. A column of numbers leaning against
the itinerary.

The fix is to stop positioning the text and start positioning **the rail's own
box**: `width: var(--dc-rail-w)` with `text-align: center`, offset by
`--dc-rail-gap` (the gutter minus the rail, derived rather than typed — that
third hand-written number was where the drift came from). The horizon tokens
join the same axis, with 3px of rail showing either side. `line-height: 1` and
`top: -0.5em` centre it on the hour rule vertically at the same time.

---

## 9.8 The scale moved off the card and onto the axis

§B built the day as a linear scale: one minute is a fixed number of pixels, so
a long visit is a long block. That is Google Calendar's model, and it does buy
the duration read — at a price paid entirely by the card.

At 1.9 px/min a 25-minute stop gets 47 pixels. There is no arrangement of a
name, a time, a note and a photograph that fits in 47 pixels, so the code grew
three size tiers (`sz-full` / `sz-med` / `sz-min`) whose job was to decide which
parts of a stop to delete. Half the column was a picture of how long things
took, and the other half was a strip with a word in it. Meanwhile the *long*
stops had two hundred pixels of empty card, because a 2h30 temple visit does
not have five times as much to say as a half-hour one.

So the day is now **rows**: every stop the same height, every space between two
stops the same height, and the clock in the gutter absorbs the difference.

```
07:00 ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄    every row spends the same height on
07:15  ▓▓▓ Fushimi Inari  2h30      however many minutes it holds, so the
08:00  ▓▓▓      ↕ 61px per hour     scale changes from row to row and the
09:00  ▓▓▓                          hours bunch and spread accordingly
09:45  ┄┄┄┄ 35 min · 4.5 km ┄┄┄┄
10:45  ▓▓▓ Kiyomizu-dera  1h40      same four stops, same four heights;
11:00  ▓▓▓      ↕ 91px per hour     the gutter is where the day's shape
12:00  ▓▓▓                          lives now
12:25  ┄┄┄┄ 50 min · 11 km ┄┄┄┄┄
13:30  ▓▓▓ Yoshimura  1h
14:00  ▓▓▓
```

Which makes the axis a picture of **how fast the plan is moving**: where the
hours bunch up, one thing is taking a long time; where they spread out, the day
is turning over quickly. That is the read the card heights used to carry, and
it is the one thing on the canvas that still has to be looked at rather than
read.

`DayCanvas` builds the column as a list of rows contiguous in both dimensions —
in time, so a minute can be interpolated across them, and in pixels, so nothing
can overlap — and everything else on the canvas is positioned by asking `yOf()`
where a given minute landed. A row's `from` is clamped forward past the previous
row's `to`, which keeps the axis monotonic through overlapping stops rather than
letting it fold back and print 11:00 above 10:00.

**What this cost, and what paid for it.** The duration read is genuinely weaker:
you can no longer see that Fushimi is two and a half times Yoshimura by
squinting at two rectangles. What you get instead is that every stop has room
for its picture and its note — the tiers are deleted, not degraded — and the
duration is still on the screen, in the gutter, one step further from the eye.
That is a slower read than a tall box, and it is the read that survives a
25-minute stop.

The height itself is set by the worst case, because a card that has to be one
height has to be the height of the fullest one: a name, a time, two lines of
note, and — when the stop is open — its actions. On a phone those actions wrap
onto a second row, since three pills do not fit across 300px, which is why the
phone's card is *taller* than the desktop's (200 against 152) rather than
shorter.

Three consequences had to be handled rather than hoped about:

- **An even hourly grid is no longer even.** Where the map compresses, three
  labels land inside one line of type. The axis drops any label closer than
  `HOUR_MIN_PX` to the last one it kept, so it thins out exactly where the day
  is dense. (The label nearest a sunrise or sunset still yields its slot to the
  horizon token, as before.)
- **The sky has to bend the same way.** `skyGradient()` took a linear
  minutes→percent map baked into its signature. It now takes the map as an
  argument, defaulting to the linear one for the ribbon and the map strip; the
  canvas passes its own. Any monotonic map works, because the ramp is sorted by
  position after it is built. Without this, sunset would be painted where sunset
  is not.
- **The first and last hour labels hang off the ends.** `top: -0.5em` centres a
  label on its rule, which at `y = 0` puts half of `14:00` above the canvas and
  out onto the page. `.at-top` / `.at-end` pull it fully inside — and `.at-end`
  has to subtract the rule's own width as well, because `top` is measured from
  the containing block's *padding* box, which starts under the border.

### The picture is the card

Three defects in one screenshot of a selected stop, all downstream of the same
thing.

The card's frame was a `border`: 1px around, 3px of `--accent` down the left.
A border sits outside the padding box, and `.dc-blk-photo` was `inset: 0` — so
the photograph stopped 1px short on three sides and 3px short on the fourth, and
what showed in that margin was the accent. A **brown frame drawn around the
picture**, at its loudest on exactly the card you had selected, since selection
turned all four sides accent-coloured.

The fix is to make the frame an inset `box-shadow` instead. An inset shadow
paints with the element's own background, which puts it *under* the photograph:
a card with a picture is the picture, corner to corner, and a card without one
still gets its edge and its accent stripe. One rule, and the image wins wherever
there is an image. Selection is then a lift (`--shadow-pop`) plus the actions
pill opening — which was always the louder signal anyway.

Two more from the same screenshot:

- **The glass pane was the size of the card, not the size of the words.** It was
  a full-bleed gradient across the top of the block, which on a 900px-wide
  desktop card is a pale slab over half a photograph worth showing. It is now
  `width: fit-content` with the measure capped at `52ch`, so the pane ends where
  the text ends.
- **The column had a margin on one side only.** `left: 12px; right: 0` ran every
  card flush into the end of the weather panel. Both sides now come off one
  token, `--dc-inset`.

And one that the constant height *revealed* rather than caused: the after-dark
wash was a background, so on every card with a photograph — which is to say on
most of the cards it describes — it was painted underneath the picture and
never seen. It is an overlay now.

### The row between two cards

*"`🚃 45 min · 21.0 km`  `15 min spare` — it's not quite readable."* Measured
in the browser, on the composited pixels rather than on the tokens, both themes,
three days:

| label | light | dark |
|---|---|---|
| `.leg-chip.tight` — the feasibility warning | **3.52** | **3.43** |
| `.dc-slack` — `15 min spare` | **4.36** | 5.45 |
| `.leg-chip` — an ordinary leg | 4.50 | 5.09 |
| `.dc-tail em` — `＋ propose something here` | **2.35** | **4.42** |

One cause with three faces, and the codebase had already written the rule down
twice — in `.dc-hour i` ("full-strength ink, not `--color-text-muted`; muted is
calibrated against the page") and in `.dc-slack`'s own comment ("carries an
opaque surface for the same reason every other label out here does"). The gap
row never got either treatment.

- **The feasibility chips were the only translucent thing out there.**
  `background: color-mix(var(--color-tight) 18%, transparent)` lets the sky
  through, so the one chip on this canvas whose entire job is to raise an alarm
  was the least legible thing on it — and its contrast depended on what time of
  day the gap happened to fall in. The hue moves to the ink and to a ring; the
  substrate is `--color-surface`, the same one every other label out here stands
  on. 3.52 → **6.10**.
- **`--color-text-muted` is a page colour.** It measures 5.25:1 on white and
  nothing at all against a wash that is navy at 18:00. Every label in the row is
  full-strength now and stays quiet by being 0.66rem — including the tail pill's
  caption, which is on *glass* by design and so has no surface of its own to be
  measured against. 2.35 → **6.83**.
- **The long legs were being sliced mid-word.** `text-overflow` does not
  ellipsise the anonymous flex items a flex container wraps its text in, and the
  chip inherited `display: inline-flex`. As a block it truncates with a mark.

Worst case across both themes and all three days is now 6.10:1. Pinned by
`glass.spec.ts`, which asserts the *rule* rather than the numbers: anything with
a surface in that row has an opaque one and clears AA against it, and the glass
pill's two lines are the same ink.

---

## Appendix A — the measurement script

Stdlib only, no dependencies. Save and run with `python3`. It regenerates
every number in §3 and §4, in order:

1. WCAG contrast for every token in `frontend/src/theme/tokens.css`
2. whether the feasibility scale is monotonic in lightness
3. Viénot–Brettel–Mollon dichromat simulation
4. whether one OKLCH lightness holds contrast across all 360 hues
5. the accent lightness ceiling for white glyphs on a solid fill

Two conventions matter for reproducing the numbers:

- Hues are measured against `--color-bg` `#fbfaf8` (the painted page), not
  pure white. Against `#ffffff` the accent reads 3.05 and appears to pass.
- Dark-mode *text* tokens are overridden in the `@media` block, so each is
  measured on its own surface. Dark-mode *hues* are not overridden, which
  is the finding in §3.5.

```python
#!/usr/bin/env python3
"""Measurements behind docs/VISUAL-DESIGN.md.

Stdlib only, no dependencies:

    python3 palette_audit.py
"""

import math

# --------------------------------------------------------------------------
# sRGB <-> linear, relative luminance, WCAG 2.1 contrast

def to_linear(c8: float) -> float:
    """8-bit sRGB channel -> linear-light [0,1]."""
    c = c8 / 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def encode(c: float) -> float:
    """Linear-light [0,1] -> sRGB-encoded [0,1]."""
    c = max(0.0, min(1.0, c))
    return 12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def parse(hexs: str) -> tuple:
    h = hexs.lstrip("#")
    return tuple(to_linear(int(h[i:i + 2], 16)) for i in (0, 2, 4))


def to_hex(rgb) -> str:
    return "#%02x%02x%02x" % tuple(round(255 * encode(c)) for c in rgb)


def luminance(rgb) -> float:
    r, g, b = (max(0.0, min(1.0, c)) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b) -> float:
    """WCAG 2.1 contrast ratio. Args are linear-light triples."""
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


# --------------------------------------------------------------------------
# OKLCH -> linear sRGB (Bjorn Ottosson, https://bottosson.github.io/posts/oklab/)

def oklch_to_linear(L: float, C: float, h_deg: float) -> tuple:
    a = C * math.cos(math.radians(h_deg))
    b = C * math.sin(math.radians(h_deg))
    l_ = L + 0.3963377774 * a + 0.2158037573 * b
    m_ = L - 0.1055613458 * a - 0.0638541728 * b
    s_ = L - 0.0894841775 * a - 1.2914855480 * b
    l, m, s = l_ ** 3, m_ ** 3, s_ ** 3
    return (
        +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )


def in_gamut(rgb, eps: float = 1e-4) -> bool:
    return all(-eps <= c <= 1 + eps for c in rgb)


def max_chroma(L: float, h_deg: float, cap: float = 0.37) -> float:
    """Largest in-sRGB-gamut chroma at this lightness and hue (bisection)."""
    lo, hi = 0.0, cap
    for _ in range(40):
        mid = (lo + hi) / 2
        if in_gamut(oklch_to_linear(L, mid, h_deg)):
            lo = mid
        else:
            hi = mid
    return lo


# --------------------------------------------------------------------------
# Dichromat simulation (Vienot, Brettel & Mollon 1999)

RGB_TO_LMS = [[17.8824, 43.5161, 4.11935],
              [3.45565, 27.1554, 3.86714],
              [0.0299566, 0.184309, 1.46709]]
LMS_TO_RGB = [[0.080944, -0.130504, 0.116721],
              [-0.0102485, 0.0540194, -0.113615],
              [-0.000365294, -0.00412163, 0.693513]]
CONFUSION = {
    "protan": [[0, 2.02344, -2.52581], [0, 1, 0], [0, 0, 1]],
    "deutan": [[1, 0, 0], [0.494207, 0, 1.24827], [0, 0, 1]],
    "tritan": [[1, 0, 0], [0, 1, 0], [-0.395913, 0.801109, 0]],
}


def _mul(M, v):
    return [sum(M[i][j] * v[j] for j in range(3)) for i in range(3)]


def simulate(hexs: str, kind: str) -> str:
    lms = _mul(RGB_TO_LMS, list(parse(hexs)))
    return to_hex(_mul(LMS_TO_RGB, _mul(CONFUSION[kind], lms)))


# --------------------------------------------------------------------------
# Tokens under test — frontend/src/theme/tokens.css

SURFACE_LIGHT = "#ffffff"
PAGE_LIGHT = "#fbfaf8"
SURFACE_DARK = "#22242a"

FEASIBILITY = {"ok": "#2f9e6e", "tight": "#d9a13b",
               "unreasonable": "#d97b4f", "impossible": "#c4453b"}
KINDS = {"sight": "#4a5d8f", "food": "#d97b4f", "lodging": "#7b5bd2",
         "activity": "#2f9e6e", "transit": "#8a8577"}
BRAND = {"primary": "#4a5d8f", "primary-strong": "#37476e", "accent": "#d97b4f"}
TEXT = {"light text": ("#26251f", SURFACE_LIGHT),
        "light text-muted": ("#6f6c63", SURFACE_LIGHT),
        "dark text": ("#e8e6e1", SURFACE_DARK),
        "dark text-muted": ("#9a978e", SURFACE_DARK)}

RULE = "-" * 74


def section(n, title):
    print(f"\n{RULE}\n{n}. {title}\n{RULE}")


def verdict(r, floor):
    return "ok  " if r >= floor else "FAIL"


# --------------------------------------------------------------------------

def report_contrast():
    section(1, "TOKEN CONTRAST")
    print("Text needs 4.5:1 (body) / 3:1 (large). Dark-mode text tokens ARE")
    print("overridden in the @media block, so each is measured on its own surface.\n")
    for label, (fg, bg) in TEXT.items():
        r = contrast(parse(fg), parse(bg))
        print(f"  {label:20s} {fg} on {bg}  {r:6.2f}  {verdict(r, 4.5)}")

    print("\nHues are NOT overridden in the dark block - one hex serves both themes.")
    print("As non-text UI (map dots, timeline nodes, chips) they need 3:1.\n")
    print(f"  {'token':34s} {'hex':9s} {'light':>7} {'':4}  {'dark':>6} {'':4}")
    merged = {}
    for name, hexs in {**BRAND, **FEASIBILITY, **KINDS}.items():
        merged.setdefault(hexs, []).append(name)
    for hexs, names in merged.items():
        rl = contrast(parse(hexs), parse(PAGE_LIGHT))
        rd = contrast(parse(hexs), parse(SURFACE_DARK))
        print(f"  {' = '.join(names):34s} {hexs:9s} {rl:7.2f} {verdict(rl, 3):4s}  "
              f"{rd:6.2f} {verdict(rd, 3):4s}")


def report_ramp():
    section(2, "IS THE SEVERITY SCALE MONOTONIC?")
    print("A severity scale should climb in some channel, so 'worse' reads as")
    print("'more' without decoding hue.\n")
    lums = [(k, v, luminance(parse(v))) for k, v in FEASIBILITY.items()]
    for k, v, l in lums:
        bar = "#" * round(l * 50)
        print(f"  {k:14s} {v}  L={l:.3f}  {bar}")
    seq = [l for _, _, l in lums]
    mono = all(x > y for x, y in zip(seq, seq[1:])) or all(x < y for x, y in zip(seq, seq[1:]))
    print(f"\n  monotonic in luminance: {mono}")
    if not mono:
        peak = max(lums, key=lambda t: t[2])
        print(f"  -> '{peak[0]}' is the BRIGHTEST step, brighter than both its neighbours")


def report_cvd():
    section(3, "DICHROMAT SIMULATION")
    print("Vienot-Brettel-Mollon. Contrast between two simulated colours is a")
    print("proxy for 'can these still be told apart' - near 1.0 means identical.\n")
    for title, group in (("feasibility scale", FEASIBILITY), ("stop-kind hues", KINDS)):
        print(f"  {title}")
        print(f"    {'token':14s} {'normal':9s} {'deutan':9s} {'protan':9s}")
        for k, v in group.items():
            print(f"    {k:14s} {v:9s} {simulate(v, 'deutan'):9s} {simulate(v, 'protan'):9s}")
        pairs = []
        keys = list(group)
        for i in range(len(keys)):
            for j in range(i + 1, len(keys)):
                a, b = simulate(group[keys[i]], "deutan"), simulate(group[keys[j]], "deutan")
                pairs.append((contrast(parse(a), parse(b)), keys[i], keys[j]))
        print("    worst confusions under deuteranopia:")
        for r, x, y in sorted(pairs)[:3]:
            print(f"      {x:14s} vs {y:14s} {r:.2f}:1")
        print()


def report_hue_sweep():
    section(4, "DOES ONE OKLCH LIGHTNESS HOLD ACROSS ALL HUES?")
    print("The question a photo-derived accent rests on: if the hue is arbitrary,")
    print("can contrast still be guaranteed by construction?\n")
    print("  light accent  L=0.52, dark accent  L=0.72, chroma clamped to gamut\n")
    print(f"  {'hue':>4} {'light':9s} {'vs page':>8} {'Cmax':>6}   "
          f"{'dark':9s} {'vs surf':>8} {'Cmax':>6}")
    worst_l = worst_d = (99.0, None)
    for h in range(0, 360, 15):
        cl, cd = max_chroma(0.52, h), max_chroma(0.72, h)
        lrgb = oklch_to_linear(0.52, min(0.13, cl), h)
        drgb = oklch_to_linear(0.72, min(0.13, cd), h)
        rl = contrast(lrgb, parse(PAGE_LIGHT))
        rd = contrast(drgb, parse(SURFACE_DARK))
        worst_l = min(worst_l, (rl, h))
        worst_d = min(worst_d, (rd, h))
        print(f"  {h:>4} {to_hex(lrgb):9s} {rl:8.2f} {cl:6.3f}   "
              f"{to_hex(drgb):9s} {rd:8.2f} {cd:6.3f}")
    print(f"\n  worst light: {worst_l[0]:.2f}:1 at hue {worst_l[1]}")
    print(f"  worst dark:  {worst_d[0]:.2f}:1 at hue {worst_d[1]}")
    print(f"  every hue clears the 3:1 floor: {worst_l[0] >= 3 and worst_d[0] >= 3}")
    print("\n  Chroma must be CLAMPED, not fixed - cyan and purple differ by ~2.5x:")
    for h in (180, 270):
        print(f"    hue {h:>3}  max chroma at L=0.52  {max_chroma(0.52, h):.3f}")


def report_fill_ceiling():
    section(5, "ACCENT LIGHTNESS CEILING FOR WHITE GLYPHS")
    print("tokens.css hardcodes --accent-contrast: #fff. For that to be safe for")
    print("an arbitrary derived hue, the accent must be dark enough at every hue.\n")
    print(f"  {'L':>6} {'worst hue':>10} {'min vs #fff':>13}  verdict")
    white = parse("#ffffff")
    for L in (0.45, 0.50, 0.52, 0.55, 0.58, 0.62, 0.65):
        worst = (99.0, None)
        for h in range(0, 360, 5):
            rgb = oklch_to_linear(L, min(0.13, max_chroma(L, h)), h)
            worst = min(worst, (contrast(rgb, white), h))
        r = worst[0]
        v = "ok" if r >= 4.5 else ("large text only" if r >= 3 else "FAIL")
        print(f"  {L:>6.2f} {worst[1]:>10} {r:>13.2f}  {v}")


if __name__ == "__main__":
    report_contrast()
    report_ramp()
    report_cvd()
    report_hue_sweep()
    report_fill_ceiling()
    print()
```

### Output at the time of writing

```
--------------------------------------------------------------------------
1. TOKEN CONTRAST
--------------------------------------------------------------------------
  light text           #26251f on #ffffff   15.36  ok
  light text-muted     #6f6c63 on #ffffff    5.25  ok
  dark text            #e8e6e1 on #22242a   12.44  ok
  dark text-muted      #9a978e on #22242a    5.31  ok

  token                              hex         light         dark
  primary = sight                    #4a5d8f      6.19 ok      2.40 FAIL
  primary-strong                     #37476e      8.80 ok      1.69 FAIL
  accent = unreasonable = food       #d97b4f      2.92 FAIL    5.09 ok
  ok = activity                      #2f9e6e      3.23 ok      4.61 ok
  tight                              #d9a13b      2.21 FAIL    6.73 ok
  impossible                         #c4453b      4.72 ok      3.15 ok
  lodging                            #7b5bd2      4.72 ok      3.15 ok
  transit                            #8a8577      3.53 ok      4.21 ok

--------------------------------------------------------------------------
2. IS THE SEVERITY SCALE MONOTONIC?
--------------------------------------------------------------------------
  ok             #2f9e6e  L=0.262  #############
  tight          #d9a13b  L=0.406  ####################
  unreasonable   #d97b4f  L=0.295  ###############
  impossible     #c4453b  L=0.163  ########

  monotonic in luminance: False
  -> 'tight' is the BRIGHTEST step, brighter than both its neighbours

--------------------------------------------------------------------------
3. DICHROMAT SIMULATION
--------------------------------------------------------------------------
  feasibility scale
    token          normal    deutan    protan
    ok             #2f9e6e   #898970   #96966e
    tight          #d9a13b   #b4b435   #a9a93c
    unreasonable   #d97b4f   #9e9e49   #8a8a50
    impossible     #c4453b   #7d7d33   #5f5f3c
    worst confusions under deuteranopia:
      ok             vs impossible     1.21:1
      ok             vs unreasonable   1.27:1
      tight          vs unreasonable   1.28:1

  stop-kind hues
    token          normal    deutan    protan
    sight          #4a5d8f   #58588f   #5b5b8f
    food           #d97b4f   #9e9e49   #8a8a50
    lodging        #7b5bd2   #6666d2   #5f5fd2
    activity       #2f9e6e   #898970   #96966e
    transit        #8a8577   #868677   #868677
    worst confusions under deuteranopia:
      activity       vs transit        1.03:1
      food           vs activity       1.27:1
      lodging        vs transit        1.29:1

--------------------------------------------------------------------------
4. DOES ONE OKLCH LIGHTNESS HOLD ACROSS ALL HUES?
--------------------------------------------------------------------------
   hue light      vs page   Cmax   dark       vs surf   Cmax
     0 #a24466       5.68  0.211   #e680a1       5.88  0.191
    15 #a64450       5.66  0.208   #ea808a       5.89  0.177
    30 #a74639       5.62  0.209   #eb8373       5.93  0.175
    45 #a44c1d       5.57  0.149   #e7885d       5.98  0.185
    60 #9a5500       5.48  0.122   #df8f48       6.05  0.169
    75 #8d5e00       5.39  0.110   #d49838       6.14  0.152
    90 #816500       5.30  0.106   #c4a032       6.23  0.147
   105 #736c00       5.22  0.110   #b1a93a       6.33  0.153
   120 #607200       5.13  0.124   #9ab04b       6.43  0.171
   135 #467922       5.04  0.152   #80b761       6.52  0.210
   150 #1d7d3e       4.97  0.143   #62bb78       6.59  0.198
   165 #007c59       4.99  0.109   #3fbe90       6.63  0.151
   180 #007a6b       5.02  0.094   #06bfa8       6.64  0.131
   195 #007879       5.05  0.089   #00bcbc       6.59  0.123
   210 #007785       5.09  0.090   #00b9cf       6.54  0.125
   225 #007493       5.13  0.099   #22b5e1       6.46  0.136
   240 #0070a6       5.20  0.119   #4baeed       6.36  0.163
   255 #2e69b2       5.31  0.171   #6aa7f4       6.25  0.148
   270 #4c62b3       5.42  0.268   #859ff6       6.15  0.145
   285 #635bb0       5.51  0.282   #9d98f2       6.06  0.152
   300 #7555a8       5.58  0.278   #b191ea       5.98  0.172
   315 #854f9c       5.63  0.256   #c38adc       5.93  0.215
   330 #914a8c       5.66  0.237   #d285cb       5.89  0.286
   345 #9b467a       5.68  0.221   #de82b7       5.88  0.223

  worst light: 4.97:1 at hue 150
  worst dark:  5.88:1 at hue 345
  every hue clears the 3:1 floor: True

  Chroma must be CLAMPED, not fixed - cyan and purple differ by ~2.5x:
    hue 180  max chroma at L=0.52  0.094
    hue 270  max chroma at L=0.52  0.268

--------------------------------------------------------------------------
5. ACCENT LIGHTNESS CEILING FOR WHITE GLYPHS
--------------------------------------------------------------------------
       L  worst hue   min vs #fff  verdict
    0.45        150          7.02  ok
    0.50        155          5.65  ok
    0.52        155          5.17  ok
    0.55        155          4.56  ok
    0.58        160          4.02  large text only
    0.62        165          3.42  large text only
    0.65        170          3.04  large text only
```

---

## Appendix B — solving the severity ramp

§3.3 says a severity scale must be monotonic in *something*. Finding values
that are monotonic **and** clear contrast in both themes **and** stay separable
for a dichromat turned out to be over-constrained enough that guessing failed
three times, so it was solved by search instead.

Save alongside `palette_audit.py` (Appendix A) and run with `python3`.

Three findings came out of it, each of which changed the design:

1. **sRGB has far less chroma in the amber→red arc than §4.5 assumed.** The
   `C ≥ 0.19` alarm floor is unreachable at any lightness that also satisfies
   contrast. See the gamut probe below.
2. **Under deuteranopia the whole arc collapses onto one yellow line**, so
   separation between steps comes almost entirely from lightness — the steps
   have to be spread much further apart than looks necessary to a trichromat.
   This is what forced `ok` out of the ramp: dropping it freed the range.
3. **`color-mix(in srgb, …)` interpolates on gamma-encoded channels**, not in
   linear light. Modelling it the obvious (wrong) way makes a tinted badge
   background look far lighter than a browser paints it, and made the dark
   theme appear to have no solution at all.

```python
#!/usr/bin/env python3
"""Search for a feasibility ramp, instead of guessing one."""

import itertools
from palette_audit import (
    contrast, encode, max_chroma, oklch_to_linear, parse, simulate, to_hex,
    to_linear,
)

PAGE_LIGHT, SURFACE_LIGHT = "#fbfaf8", "#ffffff"
PAGE_DARK, SURFACE_DARK = "#191a1e", "#22242a"

# 'ok' is deliberately NOT in the ramp. A day that is fine should say nothing;
# silence is the signal. That frees the whole lightness range for the three
# steps that are actually alarms, which is what makes dichromat separation
# achievable at all.
STEPS = ["tight", "unreasonable", "impossible"]

# Floors for the COLOUR channel alone. Redundant glyph/shape coding stacks on
# top, so these are not the whole job.
MIN_ADJACENT_CVD = 1.35
MIN_ENDS_CVD = 1.90


def mix(fg, bg, pct):
    """CSS `color-mix(in srgb, fg pct%, bg)`.

    sRGB is a gamma-ENCODED space, so the interpolation happens on encoded
    channels and is then decoded back to linear light for the contrast maths.
    """
    return tuple(
        to_linear(255 * (encode(f) * pct + encode(b) * (1 - pct)))
        for f, b in zip(fg, bg)
    )


def probe_gamut():
    """What chroma is actually available in the alarm arc?"""
    hues = [15, 25, 35, 45, 60, 75, 90]
    print(f"  {'L':>5} " + " ".join(f"h={h:<5}" for h in hues))
    for L in [x / 100 for x in range(30, 95, 5)]:
        print(f"  {L:5.2f} " + " ".join(f"{max_chroma(L, h):<7.3f}" for h in hues))


def build(L, h, want_c):
    c = min(want_c, max_chroma(L, h))
    return oklch_to_linear(L, c, h), to_hex(oklch_to_linear(L, c, h)), c


def cvd_ok(hexes):
    """Worst adjacent and end-to-end separation across deutan and protan."""
    worst_adj, worst_ends = 99.0, 99.0
    for cvd in ("deutan", "protan"):
        sims = [parse(simulate(h, cvd)) for h in hexes]
        for i in range(len(sims) - 1):
            worst_adj = min(worst_adj, contrast(sims[i], sims[i + 1]))
        worst_ends = min(worst_ends, contrast(sims[0], sims[-1]))
    return worst_adj, worst_ends


def search(page, surface, badge_pct, l_range, label):
    page_rgb, surface_rgb = parse(page), parse(surface)
    best = None
    for ls in itertools.product(l_range, repeat=3):
        if not (ls[0] < ls[1] < ls[2] or ls[0] > ls[1] > ls[2]):
            continue
        for hs in itertools.product([75, 85], [40, 50], [15, 25]):
            built = [build(L, h, 0.30) for L, h in zip(ls, hs)]
            rgbs, hexes, cs = zip(*built)

            # CHROMA must rise with severity -- the real salience channel.
            # Raw WCAG contrast is an accessibility floor, not a salience
            # metric, and in dark mode the two conflict: forcing contrast to
            # keep rising drives lightness to 0.92, where sRGB has no chroma
            # left and the most severe step comes out the palest and quietest.
            if not all(x < y for x, y in zip(cs, cs[1:])):
                continue
            vs_page = [contrast(r, page_rgb) for r in rgbs]
            if min(vs_page) < 3.0:
                continue
            # Badge text over its own tinted background.
            if any(contrast(r, mix(r, surface_rgb, badge_pct)) < 4.5 for r in rgbs):
                continue
            adj, ends = cvd_ok(hexes)
            if adj < MIN_ADJACENT_CVD or ends < MIN_ENDS_CVD:
                continue
            # Prefer the loudest top step, then the widest CVD separation.
            score = (round(cs[-1], 3), round(adj, 3), round(ends, 3))
            if best is None or score > best[0]:
                best = (score, ls, hs, hexes, cs, vs_page, adj, ends)

    if best is None:
        print(f"{label}: NO RAMP SATISFIES EVERY CONSTRAINT")
        return None
    _, ls, hs, hexes, cs, vs_page, adj, ends = best
    print(f"\n{label}")
    for name, hexs, L, c, h, vp in zip(STEPS, hexes, ls, cs, hs, vs_page):
        print(f"  {name:14s} {hexs}  L={L:.2f} C={c:.3f} h={h:<3d} vs page {vp:.2f}")
    print(f"  dichromat: adjacent {adj:.2f}:1, end-to-end {ends:.2f}:1")
    return ls, hs, hexes, cs


if __name__ == "__main__":
    probe_gamut()
    search(PAGE_LIGHT, SURFACE_LIGHT, 0.16,
           [x / 100 for x in range(34, 74, 2)], "LIGHT")
    search(PAGE_DARK, SURFACE_DARK, 0.16,
           [x / 100 for x in range(56, 94, 2)], "DARK")
```

### Output at the time of writing

```
      L h=15    h=25    h=35    h=45    h=60    h=75    h=90
   0.30 0.120   0.122   0.106   0.086   0.071   0.064   0.062
   0.35 0.140   0.142   0.123   0.100   0.082   0.074   0.072
   0.40 0.160   0.162   0.141   0.115   0.094   0.085   0.082
   0.45 0.180   0.183   0.158   0.129   0.106   0.095   0.092
   0.50 0.200   0.203   0.176   0.143   0.117   0.106   0.102
   0.55 0.220   0.223   0.193   0.157   0.129   0.116   0.112
   0.60 0.240   0.243   0.211   0.172   0.141   0.127   0.123
   0.65 0.240   0.236   0.228   0.186   0.153   0.137   0.133
   0.70 0.194   0.191   0.194   0.200   0.164   0.148   0.143
   0.75 0.153   0.151   0.153   0.160   0.176   0.158   0.153
   0.80 0.116   0.114   0.116   0.122   0.140   0.169   0.164
   0.85 0.082   0.082   0.083   0.088   0.101   0.129   0.174
   0.90 0.052   0.052   0.053   0.056   0.065   0.083   0.128

LIGHT
  tight          #865900  L=0.50 C=0.106 h=75  vs page 5.86
  unreasonable   #852b00  L=0.42 C=0.132 h=40  vs page 8.61
  impossible     #6f000c  L=0.34 C=0.138 h=25  vs page 12.02
  dichromat: adjacent 1.39:1, end-to-end 1.94:1

DARK
  tight          #ffe1a2  L=0.92 C=0.086 h=85  vs page 13.69
  unreasonable   #ffae81  L=0.82 C=0.112 h=50  vs page 9.65
  impossible     #ff6e80  L=0.72 C=0.177 h=15  vs page 6.46
  dichromat: adjacent 1.37:1, end-to-end 1.92:1
```

For comparison, the scale this replaced collapsed to **1.21:1** end-to-end
under deuteranopia — `ok` and `impossible` were effectively the same colour.

A note on reproducing §4's hue sweep: Appendix A samples every 15°, which finds
a worst case of 4.97:1 at hue 150. `src/lib/oklch.ts` is exercised at every 1°
by the frontend tests, which finds 4.93:1 at hue 156. Both clear the 3:1 floor;
the finer sweep is simply less lucky about where it lands.
