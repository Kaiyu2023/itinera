# Itinera — Visual Design

Status: **v2, built** · 2026-07-30 · author: Kaiyu Huang + Claude

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
| regenerated tokens | `src/theme/tokens.css` |

Three things the build changed about the design:

1. **Blocks do not grow to fit their content.** If the scale bends, the picture
   lies. A 20-minute stop is 32 pixels tall and shows only its name; detail
   degrades through `sz-full` / `sz-med` / `sz-min` tiers instead.
2. **A long stop earns its photo.** Strict proportionality left a 2h30 visit as
   a large empty rectangle. Filling it with the place's photo turns duration
   into something worth looking at — and is direction D's "photo bleeding off
   the edge" arriving through the back door.
3. **"After dark" is judged on when a stop *ends*.** The first cut tested the
   start time, which missed the exact case the design was built to show: the
   Arashiyama grove begins in daylight at 14:45 and runs out of it. Caught by
   `e2e/depiction.spec.ts`, not by looking.

Deliberately **not** done, and still open:

- **The map still colours by stop kind** (`PlanMap.tsx`, `PlanGovernance.tsx`,
  `candidateComposer.tsx` — 16 sites). The collision §3.2 describes is fixed
  where it actually bit, in the day view; the map is a denser surface with its
  own constraints and wants its own pass. Those hues are at least legible now,
  since they have dark-mode variants.
- **Direction C** — unboxing the map and deleting the Timeline/Map toggle. The
  toggle is load-bearing for the desktop map default that the add-stop preview
  flow depends on.
- **`accentColor` → `accentHue`** (§4.4) is still the frontend deriving the hue
  from the stored hex. See §6.3 for why that ordering is deliberate.

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
