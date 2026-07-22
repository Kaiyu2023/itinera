# Next session: Phase A milestone 3 (governance UI interactivity)

Milestone 2 (map core) is **done** — see the commit history and
`docs/mockups/milestone-2-map/` for the approved designs it implements:

- `frontend/src/map/MapRenderer.ts` — the provider port (interface-first rule).
  `MockMapRenderer` (keyless stylised tiles: canvas basemaps in
  `mockBasemap.ts`, SVG routes, DOM markers, drag-pan + zoom) ships now;
  `GoogleMapRenderer` arrives in Phase B behind the same interface.
  `MapView.tsx` is the declarative React face; renderer selection lives in
  `createMapRenderer()` there and nowhere else.
- Plan tab (`PlanTab.tsx` + `PlanMap.tsx`): desktop `☰ Timeline | 🗺 Map`
  segmented toggle (persisted in localStorage `itinera.planView`, default map;
  timeline is the unchanged full-width view). Map view = 380px panel
  (scrubber incl. 🗾 Trip chip, compact day head/daylight/stop list, or trip
  legend + candidates) + map card with candidates layer, stop popover.
  Mobile: floating Map pill → full-screen sheet overlay (scrubber, featured
  stop card, draggable-ish two-state sheet). Deep links:
  `?view=map|timeline&day=<dayId|trip>&stop=<stopId>`.
- Gotcha learned: grid items that are scroll containers (`overflow` ≠
  visible) can collapse to 0-height rows in Firefox — pin `min-height` or
  avoid `overflow:hidden` on them.
- Headless-screenshot artifact (pre-existing, not a bug): `loading="lazy"`
  thumbs render as alt text in headless Firefox captures.

## Standing rules (do not relax)

- Claude writes the **frontend only**; Kaiyu writes the Rust/axum backend
  (learning Rust — explain when asked, never write it unasked). No
  AWS/Cloudflare until the contract freezes as `docs/openapi.yaml`.
- Interface-first for every external provider. Everything runs against
  `MockApiClient` + real fixtures (bookable Nov 14–20 2026 Japan trip).
  Mock users: Kaiyu + Persona 5 first names only — never game art/dialogue/
  assets, no "Persona" in naming or marketing. ~$0/month budget.
- Show design mockups for approval before major implementation.
- Commits: `git -c user.name="Kaiyu Huang" -c user.email="kaiyu.huang@proton.me" commit`,
  trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` plus the
  current session's `Claude-Session:` URL.

## Milestone 3 scope (per docs/DESIGN.md — mock up first, then build)

Make governance interactive against MockApiClient:
- Voting on open polls, poll lifecycle states, quorum display.
- Proposal review flow (leader approve/reject, route to poll), proposal
  diffs rendered as human-readable change lists.
- Review queue actions (approve/reject AI-originated items).
- Wire the currently-disabled "Discuss / Propose change / ＋ Propose a stop"
  buttons in the Plan tab (popover, sheet, panel) into these flows.

Then milestone 4 (money/prep interactivity) and 5 (polish, PWA, a11y,
contract freeze → `docs/openapi.yaml`, at which point the backend starts).
