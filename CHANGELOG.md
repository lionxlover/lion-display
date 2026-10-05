# Changelog

## [0.26.0] — 2026-10-05 — The caption's double-click grammar

The gap-filling program's eighth release (one gap per version, the
ledger's own next line): **the caption's double-click grammar** —
the drawn chrome's sixth follow-on, the ledger's own first named
remainder. The states arm gave the windows their verbs (0.17.0),
the operator's hand gave them the drag (0.18.0), the caption became
a grip (0.21.0) — but the one gesture every desktop user reaches
for first was still missing: the double-click on the title bar that
maximizes, and the one that restores. Windows sends
`WM_NCLBUTTONDBLCLK`; WindowServer zooms the sheet; every
compositor a user has ever touched answers the pair. Zero wire
movement: the pair only *drives* the same `maximize`/`unmaximize`
verbs the frozen surface already serves — the caption is the third
door into the states arm's one room.

* **The pair's gate** (the one mechanism): a press on the band —
  outside the close affordance — either *pairs* with the pending
  caption click (the same window, the second press inside the
  500 ms double-click window and the 8 px double-click radius) or
  *becomes* it (the clock restarts — one slot, the whole desktop,
  the way every desktop's double-click timer resets on any click).
  The qualifying second press fires the toggle *on the press* (the
  `WM_NCLBUTTONDBLCLK` doctrine, never the release) and still arms
  its grip: the press's own doctrine (consumed, focusing) holds on
  top of the toggle, exactly the way a second press rides the
  first's activation.
* **The verb's own driver**: `caption_toggle_maximize` — the
  states arm's machinery answering the pair: `machine.maximize()`
  on a floating window, `machine.unmaximize()` on a maximized one
  (the `wanted` truth's own word), the restore point captured on
  the engagement (the same capture the verb's driver takes), the
  proposal minted under the live policy, the visibility truth
  synced, the configure riding the outbox through the wake
  contract's own vehicle — the client's ack+commit realizes it,
  the same two-phase commit the client's own verbs answer (never
  tearing). A fullscreen window never reaches the door (its frame
  is the content's — no band to press); a dead window receives
  nothing (the sweeps' own honesty).
* **The two gates, measured honestly**: the time gate reads the
  device's injected clock (the same domain the frame loop and the
  input→photon budget read); the radius gate reads the pointer's
  position at the press — the pair is one *gesture* of one hand,
  never two clicks far apart on a long title bar. A slow second
  press is two clicks (the Phase 52 band behavior the whole
  story); a far second press is two gestures (the slot re-records
  at the new point).
* **The territories never touched**: the close affordance never
  pairs (its grammar is the arm-fire close ask — each click fires
  its own ask) and a close press *voids* the pending caption click
  (a click that targets the button never targets the caption); a
  press that converts into a drag voids the pair (the grammar is
  click-click, never drag-click — the hand moved the window, the
  next click is a fresh first click); the death sweeps take the
  pending click with the window (a dead band never pairs); plain
  and client-decorated windows never toggle (no band, no pair —
  the presses route exactly as they always have, zero drift).
* **The held double-click** (the Windows 11 doctrine, inherited
  free): a pair whose second press the hand then *moves* rides the
  Phase 53 demotion — the freshly maximized window releases its
  states and restores its floating size under the pointer's
  proportional grip, exactly the way every desktop's
  double-click-then-drag serves. Nothing new to build: the two
  doctrines compose.

2,327 → 2,335 tests (8 `caption_dbl_session` real-socket tests —
the maximize arm (the pair firing on the second press, the fill
1918 x 966 proposed and realized at the usable origin, the presses
never routing, the pixel oracle pinning the band and the content),
the restore arm (the client's own verb engaging the geometry, the
pair releasing it, the restore point returned), the time gate (a
slow second press never pairing, the clock restarting — the next
quick press pairing with the *slow* press's own record), the
radius gate (a far second press never pairing, the slot
re-recording at the new point), the close territory (the
affordance's own clicks never toggling, the pending caption click
voided by a close press), the drag void (a conversion voiding the
pair, the clock restarting from the drag's end), the plain/CSD
controls (the presses routing exactly as before, no toggle, no
configure beyond the activation), and the death sweep (the
destroyed window's band never pairing, the world healthy). Every
prior suite green; the freeze gates untouched for the seventh
consecutive release (zero wire surface moved).

## [0.25.0] — 2026-10-03 — The chrome ghost

The gap-filling program's seventh release (one gap per version, the
ledger's own next line): the drawn chrome's fifth follow-on — **the
chrome ghost**. Phase 48 taught the dying *content* to fade (the
owned ghost under the close spring), but the drawn band left with
the route — one abrupt frame, the claims ledger sweeping the
window's chrome while its content still faded beneath. Every
desktop a user has ever used closes the whole window: DWM's genie
takes the frame, WindowServer's zoom takes the title bar with the
content. Zero wire movement: the ghost is the compositor's own
machinery; this version teaches it to carry the chrome it drew.

* **The frozen shape**: the ghost's capture grows a chrome payload —
  the frame rect the band wore, the raster cache's shape key (the
  insets, the scaled close metrics, the Liquid variant), and the
  strip's key and rect if a title served — frozen at death exactly
  as the client style froze. The band's ink is *stateless* (the
  shape is the raster's whole truth), so the ghost never copies it:
  the render path reads the same chrome cache the living desktop
  reads — same shape, same bytes, the cache's own doctrine. The
  prepare's want grows the ghost shapes (a window destroyed before
  its first rendered frame with chrome still fades a band the walk
  can draw).
* **The whole frame leaves as one**: the grade walk's ghost insert
  pushes the band, the strip, and the content at the ghost's z slot
  — exactly the layer family its live self pushed (band beneath
  strip beneath content, all beneath the next window's family) —
  every layer at the one close-spring opacity. The dressing follows
  the live band's own rule: the chrome material at a Liquid tier,
  so a fading band's frost still reads the composed canvas beneath
  it — a fading glass pane; `Minimal` fades the flat bar.
* **The claims and the vacates grow the frame**: a ghost carrying
  chrome claims its *frame* every live frame (the strip lives
  inside the band's rect — one claim covers the whole dying ink),
  the settle vacates the frame (the band's region is the ghost's
  own to clear), and the Liquid tier's scanout reservation
  outlives the window — a fading pane still frosts, the reservation
  leaving with the ghost.
* **The zero-drift controls**: plain and client-decorated windows
  ghost content-only (no chrome payload, every Phase 48 byte
  intact); with transitions off the destroy is plain — no ghost
  ever begun, the band leaving with the route as it always has (the
  tier's honest behavior, the A/B oracle's other half).
* **The honest remainders** (named, not hidden): the caption's
  double-click grammar, the window-menu family, and the face's own
  growth (the Latin accents and beyond — data-only follow-ons, the
  rasterizer never changes).

2,320 → 2,327 tests (7 `chrome_ghost_session` real-socket tests —
the band fading with the content (the frozen shape pinned, the
monotone decay, the settled desktop equal to the never-animated
destroy byte for byte), the title strip riding the fade (the
contrast proof, the darkest stroke pixel against the band through
the fade's meaningful span), the z fidelity (a window destroyed
under another keeping its band below the survivor, the survivor's
opaque content byte-exact through the whole fade), the plain/CSD
controls ghosting content-only, the transitions-off control, the
Liquid ghost carrying the `liquid` bit with the dressed band
dimming, and the unmap arm riding the chrome). Every prior suite
green; the freeze gates untouched for the sixth consecutive release
(zero wire surface moved).

## [0.24.0] — 2026-10-03 — The chrome-aware placement

The gap-filling program's sixth release (one gap per version, the
ledger's own next line): the drawn chrome's fourth follow-on — **the
chrome-aware placement**. The placement engine answered in content
space; a server-decorated window parked at the usable origin wore its
top band *above* the visible area — 29 px of caption the operator
could neither read, grip, nor close — until the window engaged
geometry. DWM and WindowServer never park a caption off-screen: their
placement answers in frame space. Zero wire movement: the chrome's
extent is the compositor's own truth; this version teaches every
placement arm to speak it.

* **The frame takes the slot**: `place_chrome` — the pure placement
  policy's chrome-aware form (`ldp-shell::layout`). The *frame* (the
  content grown by the SSD insets — the band above, the border ring
  around) takes the policy slot and the content rides inside at the
  slot plus the insets. The phone anchor parks the frame at the
  usable origin (the band the first thing on the screen, the
  overflow still under the dock — Fill's doctrine unchanged); the
  cascade steps *captions* the way every desktop a user has ever
  used steps them; the terminal corner stack anchors the frame's
  corner (the band visible even there). With `Insets::ZERO` the
  answer is byte-identical to `place()` for every policy, every
  slot — the no-chrome path never moves, by construction and by
  proof (the identity sweep is itself a test).
* **The pixel-true realization**: the shell's chrome arm resolves
  the slot in logical space (the band is 29 logical px at every DPI)
  and then clamps the *physical* frame against the physical usable
  area using the machine's own inset formula (`scale_px_up`) — a
  fractional factor's logical rounding can never shave the band's
  top edge off the screen. The physical clamp is the last word, and
  it never disagrees by more than a rounding step (the 1.25x proof:
  logical 29 scales to 36, the applied inset is 37, the clamp holds
  the frame at y=0).
* **The migration arm grows the same truth**: `replace_root` takes
  the root's *applied* insets (the live truth — a fullscreen
  window's zero insets migrate as plain geometry) and re-places the
  *frame*: the phone stack re-anchors the frame at the new usable
  origin, the desktop clamps the frame inside the new usable area.
  `None` keeps the pre-Phase-56 doctrine verbatim (plain and
  client-decorated windows never moved).
* **The z-true chrome hit** (the placement's consequence, closed
  with it): a parked window's band now sits over the stack below
  it, and the press must follow the *visible* ink — the chrome hit
  walk is topmost-first and stops at the first claim either way. A
  window's own content stops it (the router delivers, a
  click-through hole falls past by the input region's own truth), and
  a higher window's band claims above a lower window's ink: a press
  on the visible band belongs to the band's window, never to the
  content hidden beneath it — DWM's own answer, made live by the
  placement this version ships.
* **The honest remainders** (named, not hidden): the chrome ghost,
  the caption's double-click grammar, the window-menu family, and
  the face's own growth (the Latin accents and beyond — data-only
  follow-ons, the rasterizer never changes).

2,305 → 2,320 tests (6 `placement_session` real-socket tests — the
parked window's band on-screen from row 0, the frame cascade, the
plain/CSD zero-drift interleaved with the chrome-aware stack, the
oversized frame's corner anchor, the 2x panel's doubled shift, and
the hotplug migration re-anchoring the frame with the claims ledger
repainting the chrome on the new panel — plus 5 `ldp-shell` layout
unit tests and 5 shell unit tests, the SSD suites' helper pins
updated honestly for the moved windows). Every prior suite green;
the freeze gates untouched for the fifth consecutive release (zero
wire surface moved).

## [0.23.0] — 2026-10-03 — The Liquid band

The gap-filling program's fifth release (one gap per version, the
ledger's own next line): the drawn chrome's third follow-on — **the
Liquid chrome-material dressing**. The band Phase 52 drew as flat ink
finally wears the system's own glass: the chrome material, the dock's
own, served at the machine's effects tier. Zero wire movement: the
Material family has been frozen protocol since Phase 45 (`set_material`
and its six materials) — this version is the server's own chrome
finally wearing one of them.

* **The chrome material on the band**: the layer wears
  `Material::Chrome` — one truth for all system chrome (the dock's
  own): the Sheet frost pane beneath the ink (the backdrop sampled
  from the composed canvas, blurred, desaturated, veiled — the band
  genuinely reads what sits beneath it: a red window warms the glass,
  a blue one cools it, the blur mixes the boundary between them), the
  chrome hairline tracing the frame's top edge, the corners rounded
  like every glass pane, the shadow cleared (the frame is the
  window's edge, not a floating object).
* **The veil**: the dressed band's own ink drops from the flat
  near-opaque bar to a light veil ([238, 241, 246] at 168) so the
  frost reads through — a third of the backdrop's color shows while
  the veil's tint anchors the lightness (the readability floor: over
  any backdrop the band stays light, the dark title ink keeps its
  contrast). The ring, the capsule, and the glyph keep their opaque
  words. The cache keys the material variant — the ink's stateless
  inputs are geometry *and* material.
* **The pane's truths**: the frost pane is the whole frame — the
  content hole included — with the client's content riding above it
  (the glass pane is the window's own background, the
  NSVisualEffectView doctrine). The underlay never direct-scanouts
  under a serving band (the dock's own scanout subtraction, the
  plane solver's split at `needs_backdrop`), and an underlay's
  commit flows through the glass: the claim propagates, the frost
  rebuilds over the fresh canvas, the band's bytes move.
* **The frost memo's growth**: a desktop of Liquid bands is a desktop
  of frost panes — the Phase 29 two-entry memo would have thrashed.
  It takes the MaterialCache's own doctrine (a bounded word budget
  with LRU eviction, an entry cap, the honest over-budget recompute)
  plus the `frost_rebuilds` thrash oracle: five panes, one build,
  flat forever in the steady state.
* **The Minimal freeze**: `Minimal` resolves the material plain and
  paints the flat bar — every Phase 52-54 oracle byte stands (the
  tier's quality budget, the honest degradation, never a lie); the
  headless default stays Minimal.
* **The honest remainders** (named, not hidden): the chrome-aware
  placement, the chrome ghost, the caption's double-click grammar,
  the window-menu family, and the face's own growth (the Latin
  accents and beyond — data-only follow-ons, the rasterizer never
  changes).

2,292 → 2,305 tests (8 `liquid_session` real-socket tests at the
Medium tier — the pixel oracle importing the world's own material
builder and over-rule, zero drift by construction — plus 4 renderer
unit tests for the memo's budget doctrine and 1 shell unit test for
the dressed ink). Every prior suite green; the freeze gates
untouched for the fourth consecutive release (zero wire surface
moved).

## [0.22.0] — 2026-10-03 — The title glyph

The gap-filling program's fourth release (one gap per version, the
ledger's own next line): the drawn chrome's second follow-on — **the
title glyph**. The band Phase 52 drew finally carries its text: a
typeface of the compositor's own, a rasterizer of its own, and the
strip-layer machinery that serves them. Zero wire movement: `set_title`
has been frozen protocol since the beginning — this version gives its
pixels.

* **Lion Sans (`ldp-font`)**: a utilitarian geometric sans authored
  directly in em units — 96 glyphs (ASCII, the ellipsis, the space)
  plus the notdef box every uncovered codepoint draws honestly
  (visible truth: never a crash, never silent garbage; control
  characters draw blanks). The layer carries zero dependencies — a
  display server with its own face never rides a system font
  lottery (no fontconfig discovery, no fallback waterfall, no
  per-user font breaking the title bar).
* **The analytic rasterizer**: exact per-pixel area integration —
  quadratic Béziers flattened to tolerance, row-split, and the
  winding integral evaluated in closed form (the C¹ antiderivative
  of the clamped linear edge — one fraction per piece, the
  FreeType-smooth class of math). Deterministic by construction:
  the same outline and pixel size always yield byte-identical
  coverage — the oracle doctrine the whole suite leans on.
* **The pixel-true truncation**: a title wider than the band's
  budget (the frame minus the close affordance's own territory)
  ends in the ellipsis glyph — the decision rides the scaled
  advances, never a character count, and the ink never reaches the
  button.
* **The strip layer**: the title's ink rides *above* the band, *below*
  the content — CPU coverage tinted at the dock's own model, cached
  by its stateless inputs (the title, the size, the budget: same
  inputs, same bytes). Same-shaped windows still share one base
  raster; the title's bytes ride their own layer — the cache's own
  observables prove it.
* **The title change**: `set_title` on a serving window repaints the
  strip with *no client commit* — the ink is the server's, the claim
  the server's own (the set_material doctrine, verbatim: the strip's
  old and new rects claim both ends; a band standing still still
  repaints its changed text).
* **The honest remainders** (named, not hidden): the Liquid
  chrome-material dressing for the band, the chrome-aware placement,
  the chrome ghost, the caption's double-click grammar, the
  window-menu family, and the face's own growth (the Latin accents
  and beyond — data-only follow-ons, the rasterizer never changes).

2,263 → 2,292 tests (22 `ldp-font` unit tests + 6 `title_session`
real-socket tests, the pixel oracle importing the renderer's own
over-rule — zero drift by construction). Every prior suite green;
the freeze gates untouched for the third consecutive release (zero
wire surface moved).

## [0.21.0] — 2026-10-03 — The caption drag

Phase 53: the gap-filling program's third release (the ledger's
next named line, one gap at a time): the **server-side caption
drag** — the drawn title bar becomes a *move grip*, the machinery
`start_move` owns minted by the server itself at the pointer's
first motion. Zero wire movement: the drag is pure server-side
machinery — the band was already drawn, the grip was already
armed, the drag engine was already proven; this version joins
them, exactly the way every desktop's caption serves. 2,258 →
2,263 tests, every prior suite green, both freeze gates
untouched.

* **The conversion**: a press the band claims (outside the close
  affordance) arms the Phase 52 grip as before — consumed,
  focusing, never routed. The pointer's first *motion* converts
  it: the server mints the move drag itself, no client request
  involved, riding the same `mint_pointer_drag` door the
  dispatcher's `start_move`/`start_resize` arm owns (one
  machinery, two doors — the demotion, the `LiveDrag`, the
  outbox-borne proposal all shared). The anchor is the press
  point: the window tracks the hand rigidly from the grip, the
  move's geometry applying at the input pump's cadence — server
  truth, `set_position_now`, the R2 rule repainting both ends,
  the keep band holding the title grip reachable, and *zero
  configures for the move* (position never rides the wire).
* **The click doctrine**: a press without motion never drags —
  the Phase 52 band behavior is the whole story (the press
  consumed and focusing, the release consumed, nothing fired).
  The close affordance's own grip *never* converts: the
  drag-away cancel is its doctrine — a click that left the
  button never happened, and it never becomes a move either.
  One hand guards the mint: a live drag — client-minted or
  caption-minted — is never superseded by the conversion.
* **The demotion over the frame**: dragging a *maximized* SSD
  window by its caption releases the states and restores the
  floating size under the pointer's proportional grip measured
  over the *drawn frame* (the caption the hand actually holds),
  so the restored window keeps its band under the fingers — the
  Windows 11 / macOS title-drag doctrine, the size realizing at
  the client's ack+commit cadence, never tearing. The demotion's
  configure proposal parks in the outbox (the wake contract's
  own vehicle); CSD windows demote over the content rect, every
  Phase 50 pin unchanged.
* **The death sweeps**: the dragged window's destroy ends both
  the drag and the grip (the belt-and-braces eligibility ends a
  drag that slipped through); a later motion moves nothing.

## [0.20.0] — 2026-10-03 — The drawn chrome

Phase 52: the gap-filling program's second release (the ledger's
next named line, one gap at a time): the **SSD chrome pass** and
the **server-initiated close event** — the drawn title bar, border
ring, and close affordance every server-decorated window wears,
and the frozen `toplevel.close`'s first sender. Zero wire
movement: the event has been frozen since v1; this version gives
it the trigger the frozen doc named ("the user asked to close —
window button"). 2,241 → 2,258 tests, every prior suite green,
both freeze gates untouched.

* **The drawn band**: the compositor paints the chrome its
  configure insets always reserved — the title bar (28 logical
  px), the border ring (1), and the close affordance (a warm
  capsule carrying a white ×) — as CPU ink on the dock's own
  model: no framebuffer (the honest `NoFb` demotion — a visible
  band pins the frame to the composite arm, exactly the ledger's
  subtraction the dock serves), tightly-packed premultiplied
  rows, cached per frame *shape* (the ink is stateless: geometry
  is its only input, same-shaped windows share one raster, a
  resize re-paints). One geometry answer — `chrome_geometry`,
  keyed by the *applied* configure — feeds the render pass (the
  layer beneath the content), the damage ledger (the claims), and
  the input pump (the ring hit test). CSD windows draw nothing
  (their buffer's top is their own chrome — the system grid
  reserves the hit zone *inside* it), fullscreen covers everything
  (zero insets), and a window that never applied its insets draws
  nothing (the two-phase commit owns the reservation).
* **The close ask**: the drawn button follows the caption
  doctrine every desktop serves — the press *arms* (consumed
  entirely: the client's pointer never learns a press on pixels
  it does not own, the router's grabs never see it, and the
  window takes the focus — the `activated` bit riding the
  proposal the focus truth serves), the release inside the same
  affordance *fires*: the frozen `toplevel.close` emitted over
  the real socket, parked in the outbox exactly like every
  routed event. A drag away cancels (the click that left never
  happened; the grip re-arms on the next). The client destroys
  the toplevel "when ready" — the frozen doc's own words: the
  server never force-kills, the client that ignores the ask keeps
  its window and its desktop keeps working with it. The death
  sweeps clear the grip with the window, the object, and the
  session.
* **The claims ledger**: the damage engine knows only protocol
  damage (the buffer's truth); the band is the compositor's own
  ink *around* it. The ledger — `scene.chrome_prev`, the
  vacate-claims family's own sibling — diffs every serving band's
  frame rect against the one the last pass claimed: a band that
  moved or resized claims both ends (the R2 rule's chrome
  sibling), a band that hid or died claims its old rect, a band
  that came back or newly serves claims its new one. Minimize
  hides the chrome with the ink (the byte-exact restore pins it:
  the band, the ring, the affordance, the content — the exact
  words back on unminimize).
* **The geometry regime**: a maximized SSD window's *frame* fills
  the workspace area — the content lands inset by the applied
  chrome (the title bar on-screen at every state). Every Phase 49
  CSD pin is unchanged: a client-decorated window's buffer is its
  own whole footprint, the area's origin is its position.
* **The tests' own truths**: the pixel oracle reads the palette
  the painter owns (`BAND_RGB`, `RING_RGB`, `CLOSE_RGB`,
  `GLYPH_RGB` — the words over the opaque-black desktop are the
  painter's own arithmetic); the pointer walks in
  accelerator-neutral steps (the drag suite's hard lesson, made
  a helper); the wake-point doctrine on the client side (the
  events ride the wake that follows the message — the count
  discipline waits, never a single roundtrip).

## [0.19.0] — 2026-10-02 — The focus and the view

Phase 51: the states arm's follow-ons, part one — the ledger's own
named line, served whole. Two of the three follow-ons land (the
gap-filling program's first release, one gap at a time): the
**`activated` state bit** (frozen at index 3 since v1, the machine's
flag waiting for the focus-driven activation line — now riding real
proposals) and the **operator-side space switching** (the taskbar's
line: `Spaces::switch` was built, the wire had no request — now it
has one). The third follow-on, the server-initiated `close` event,
defers deliberately to Phase 52 with its honest trigger (the SSD
chrome pass — the drawn title-bar close button — deserves its own
version; the frozen event awaits it). One request and one event
appended additively (`shell.switch_workspace` +
`workspace_switched`, the Phase 45/47/50 doctrine, the freeze
re-taken consciously: 98 requests + 123 events, 37 enums), 2,241
tests, up from 2,236.

* **The focus truth**: every keyboard transition now drives the
  bit. The press sets it (click-to-focus — the click's own
  proposal, `propose_state`, a *grace-parking* state proposal: the
  live proposal it replaces parks in the drag's acknowledgment
  grace window, so a focus move the client never asked for never
  punishes a client draining a drag's serials — the `propose_drag`
  doctrine, now for the state seams); the departure clears it (a
  dialog taking the keys, an unmap, a death); a non-toplevel
  surface holds no bit itself (the bit is toplevel vocabulary) but
  the old toplevel still clears when a dialog takes the keys. The
  machine's own law rides whole: minimize clears activation,
  activation clears minimized (the fuzz invariants, unchanged).
* **The promotion doctrine**: when the focus holder leaves, the
  keys land on the frontmost window that remains — the topmost
  toplevel or dialog in the routing order, never on `None` while a
  window could take them (the macOS key-window doctrine; popups
  hold no keys — pointer-grab surfaces). The hidden discipline
  (minimize, the view switch), the unmap discipline (a detach
  commit), and the death sweeps all route through the one
  `promote_focus` seam; a dying holder's own leave and proposal
  naturally emit nothing (its route and machine entry are gone
  before the promotion runs — the dead never speak).
* **The view switch**: `shell.switch_workspace(index)` moves the
  *viewed* space (the taskbar's arm). The clamp lands on the
  honest actual (`workspace_switched` carrying it — to the asker
  directly, to every *other* shell binder through its outbox: the
  wallpaper daemon that never asked still knows). The visibility
  sweep is the set_workspace verb's own machinery at desktop scale
  — every toplevel by its space truth, every dialog by its
  *parent's* (the sheet moves with its window, the role the spaces
  machine never tracked now following it), the sticky windows
  showing everywhere, the hidden spaces' frame requests parking
  through App-Nap's own seam. No window's own state is proposed —
  the only configures a switch can carry are the focus truth's
  own: the frontmost window *homed on the newly-viewed space*
  takes the keys (the sticky windows show everywhere, but the
  switch's keys belong to the space's own windows — an empty space
  falls back to the frontmost visible), the old holder releases
  them. One transition, never the intermediate topmost (the
  sweep's per-hide promotion stands down; the end-of-sweep
  promotion is the authority). A switch to the space already
  viewed is a full no-op — nothing sweeps, nothing is emitted.
* **Tests**: 1 machine test (the state proposal parking the
  replaced drag proposal, acknowledgeable; the minimize/activation
  interplay unchanged) + 4 `focus_session` real-socket tests (the
  press narrative between two windows — both proposals, the
  machine agreement; the dying holder's promotion; the view-switch
  narrative — the ink flip, the sticky doctrine, the frame
  parking, the space-preferred focus both directions, the parked
  frame answering on return; the clamp and the broadcast to a
  second binder, the no-op silence) + the dialog gate's return
  grown its second truth (the keyboard comes home with the
  parent's re-activation, the pointer re-enter after it) and the
  drag narratives grown the press's activation proposal (the move
  proof now counts the handshake and the activation — a move
  itself still proposes nothing; the resize's `resizing` bit rides
  beside the activation). The freeze gate re-taken consciously
  (98+123, 37 enums) with the count pins updated; the conformance
  table grown the pair (`Spaces::switch`); all tiers green.
* **Honest remainders**: the server-initiated `close` event (the
  Phase 52 line — the SSD chrome pass and its close button are the
  honest trigger), press-issued serials, edge snapping, the live
  rubber-band, dma-buf create, Vulkan, TLS, the latency lab.

## [0.18.0] — 2026-10-02 — The operator's hand

Phase 50: the interactive move/resize vocabulary the spec never
grew, served whole — the title-bar drag and the edge grip every
desktop serves, the two mechanisms the frozen shell's honest
remainder named (the "interactive move/resize vocabulary the spec
never grew", the roadmap's own line). Two requests appended
additively (`toplevel.start_move`/`start_resize`, the Phase 45/47
doctrine — no existing opcode moves, the freeze re-taken
consciously: 97 requests + 122 events, 37 enums), the `resizing`
state bit the machine has carried frozen since v1 finally riding
real proposals. 2,236 tests, up from 2,218.

* **The move** (`toplevel.start_move(seat, serial)`): the client
  asks the *compositor* to drive the geometry. The serial passes
  the freshness gate `set_selection` and `popup.grab` enforce (the
  seat's current interaction serial — the value the client learned
  from the seat's latest serial-bearing delivery; press-issued
  serials ride the device-event serial roadmap line, named here
  honestly). From the grip on: every pointer motion batch moves
  the window at the input pump's cadence — server truth through
  `set_position_now`, the damage engine's R2 rule repainting both
  ends, *zero configures* (the client never learns positions it
  cannot use; a move never proposes). The **keep band** holds 48
  logical px of the dragged window reachable inside the work area
  (the DWM/macOS "you cannot lose your window off-screen"
  doctrine; the router's own desktop clamp is the first boundary,
  the keep band the second — both pinned). The drag ends at the
  button's release: the geometry stands, nothing is emitted.
* **The resize** (`toplevel.start_resize(seat, serial, edges)`):
  the eight-edge grip vocabulary (`resize_edge`, the xdg_toplevel
  resize_edges doctrine, the WM's `_NET_WM_MOVERESIZE` edges
  before it). Every motion computes the edge-algebra target — the
  engaged edges follow the drag's delta, the opposite corner
  anchors — clamped by the client's own min/max grammar, and
  *proposes* it through the two-phase commit: the `resizing` state
  rides every live proposal (the client's own hint that committing
  now matters), the position and the client's committed buffer
  realize together (never tearing — a left/top-edge resize moves
  the origin at the realize, the opposite corner anchored). The
  release's final proposal clears the state and carries the last
  target; a resize on a maximized or fullscreen window is refused
  (`invalid_state`: the geometry states own the size — demote
  first).
* **The demotion**: dragging a geometry-stated window by its title
  releases the states and restores the floating size under the
  pointer's *proportional* grip — the Windows 11 / macOS title-drag
  doctrine. The restore size is captured at the geometry
  engagement itself (the tree's realized floating geometry — the
  buffer the client actually committed; the workspace's
  two-thirds as the never-floated fallback, the DWM default-size
  doctrine), the window detaches *now* at the anchored position,
  and the shrink realizes at the client's ack+commit cadence (the
  window never tears; a responsive client shrinks within a beat).
* **The grace window** (the machine's one honest softening, in
  `ldp-shell`): the interactive resize supersedes proposals at the
  operator's hand speed, and a client acking the configure it
  actually *saw* — superseded by the pointer's next move before
  the ack arrived — is never at fault. The last 8 superseded drag
  serials stay acknowledgeable (the acked size realizes with the
  buffer the client actually commits; the fresher proposal stays
  live); every verb proposal clears the window — the Phase 49
  strict-ack doctrine holds whole for verb-superseded serials, a
  pinned regression. The **outbox's own freshest-wins coalescing**
  covers the same cadence from the delivery side: the drag's live
  configures are position state (a new kind tag, the §10.4
  doctrine extended) — a pointer-paced flood collapses to the
  latest, exactly the 1000 Hz motion doctrine; the final proposal
  rides the plain never-dropped class.
* **One hand, one narrative**: a fresh grip supersedes any live
  drag (the retired one ends silently); the engaging geometry
  verbs (maximize, fullscreen, minimize, a workspace move) end a
  drag on the *dragged* window outright — the states take the
  geometry back; the hints, stickiness, and the un-verbs never
  kill a grip. The death sweeps end the drag with the window, the
  role object, or the client (nothing left holding a dead grip).
* **Tests**: 5 machine tests (the edge wire domain and axis
  algebra, the explicit-size proposals, the grace window's hold /
  bound / verb-clear, the verb-superseded strictness preserved) +
  5 `DragHost`/algebra unit tests (the host's begin/end/supersede,
  the delta rounding, the keep band's arithmetic, the per-edge
  resize target algebra, the configure's ten-value shape) + 8
  `drag_session` real-socket tests (the move narrative — the
  exact travel, the pixel oracle, zero configures, the release;
  the keep band over the wire; the bottom-right resize — the
  `resizing` state, the two-phase realize with the top-left
  anchored, the final proposal; the left-edge resize moving the
  origin at the realize; the graceful ack of a superseded serial;
  the demotion — the proportional anchor, the detach, the realize
  under the grip, the pixel truths; the typed refusals — stale
  serial, geometry-stated resize, out-of-domain edges; the death
  sweep). All pinned against the router's own pointer oracle (the
  drag follows the pointer whatever the accel curve did with the
  batch — the window's travel IS the pointer's travel).
* **Honest remainders**: press-issued serials on the wire (the
  device-event serial line — the freshness gate today references
  the seat's latest serial-bearing delivery), edge snapping (the
  snap-preview geometry), the live rubber-band geometry between
  client commits (the render-thread line, with the other geometry
  drivers), the `activated` bit, the server-initiated `close`
  event, the operator-side space switching, dma-buf create,
  Vulkan, TLS, the latency lab.

## [0.17.0] — 2026-10-02 — The states arm

Phase 49: the last honestly-consumed toplevel vocabulary, served
whole — the frozen 17-request `ldp.shell.toplevel` surface, every
request live, zero wire surface moved (the schema is v1's; the
freeze gate is untouched; `ldp-shell`'s `Toplevel`/`Spaces` machines
were built and tested all along — this release is the server wiring
they waited for). 2,218 tests, up from 2,201.

* **The geometry verbs** (`toplevel.maximize`/`unmaximize`,
  `fullscreen(output|null)`/`unfullscreen`): the two-phase commit,
  real. The intent lands on the machine; the proposal derives under
  the live policy — maximize fills the workspace area minus the
  insets the decoration mode reserves (SSD chrome, or the CSD
  system hit zone), fullscreen covers the whole output with zero
  insets and the output pinned (the client's own bound output
  object, validated by name; null resolves to the primary's — an
  unbound client gets an unpinned proposal). The client
  `ack_configure`s, and its next commit realizes the placement: the
  buffer it attaches is the size answer, the position completes it
  (`set_position_now` — a window lands placed, never
  origin-then-jump). The **restore point**: the position the window
  held when it first engaged a server-geometry state; the un-verbs
  return there, and a chain of geometry verbs restores to the one
  point where server geometry began. Fullscreen precedes maximized
  (the machine's invariant, observable over the wire: both flags
  ride the states, the geometry is fullscreen's).
* **The visibility verbs** (`toplevel.minimize`/`unminimize`,
  `set_workspace`, `set_sticky`): hidden from the space, kept
  alive — applied immediately (the shell's own visual decision). A
  hidden window — minimized, or homed on a space the seat is not
  viewing; sticky windows show on every one — *renders nowhere*,
  *takes no input* (a hidden keyboard focus holds no keys — the
  leave rides the focus retarget), *parks its frame requests* (App
  Nap's own seam, the Phase 45 occlusion machinery reused exactly:
  `frame_dropped(surface_hidden)` on the hide, the parked request
  answered the moment the window returns), and *leaves its outputs*
  (`leave_output`/`enter_output` over the wire, the release-gate
  truth). The vacate claims repaint the ink both directions (the
  ghost host's doctrine, now the hidden set's); unminimize restores
  the exact bytes (the A/B/A byte oracle). `set_workspace` clamps
  into the count and reports the actual through
  `workspace_changed` (plus the configure's workspace field).
* **The hints and the strict ack**: `set_title`/`set_app_id` land
  on the machine (the bounded-string doctrine — the client's
  validator refuses over-length before the wire; the server refuses
  the embedded NUL with `invalid_string`); the size pair saturates
  (a min above a max clamps to it, consistent by construction);
  `ack_configure` is strict — a serial that is not the live
  proposal is `invalid_state` (the dialog's doctrine, now the
  toplevel's; a superseding proposal kills the previous serial, so
  stale acks are detectable).
* **The machine-owned handshake**: `get_toplevel`'s initial
  configure is now the machine's own first proposal (`propose_at`,
  the one method this release grows in `ldp-shell`) under the
  *seat's interaction clock* — serial 1, the data family's
  `set_selection` reference, Phase 32's doctrine untouched; the
  machine adopts the serial as its watermark and continues the
  domain itself. The handshake now carries the **insets the
  decoration mode reserves** (CSD's 5-px system hit zone on all
  four sides — the `ssd` module's own doctrine, so CSD apps land
  on the system grid; the previous all-zero insets were the Phase
  31 simplification, a conscious honesty upgrade). The ack of the
  handshake is accepted — the strict two-phase includes the mint.
  The shell bind answers **`workspace_count`** (the spec's "after
  binding" — the shell global's first bind-time event): four
  spaces, the desktop doctrine, with `--workspaces N` (and
  `CompositorConfig::workspaces`) opting another count in (zero
  promotes to one, the machine's own rule).
* **The death sweep**: a destroyed toplevel object drops its host
  entry and releases the scene's role mapping (the surface may
  outlive the role, roleless — a re-mint replaces both mappings);
  a destroyed surface drops its toplevel entries, its spaces
  membership, and its hidden-set membership (a destroyed workspace
  resident leaves no residue — and leaves no ghost: a hidden
  window's death has no ink to fade).
* **Tests**: 6 `ToplevelHost` unit tests + 3 `propose_at` machine
  tests + 10 `states_session` real-socket tests (the handshake
  under the seat serial with the CSD insets and the accepted ack;
  the maximize narrative — the cascade restore point, the derived
  size, the position realize, the fill's pixel oracle; the
  unmaximize restore; the fullscreen cover with the output pin;
  the precedence; the stale-ack refusal; the minimize narrative —
  the frame park, the leave/enter truth, the A/B/A byte oracle;
  the workspace move/clamp/report; the sticky doctrine; the hints
  with the error codes and the saturation).
* **Honest remainders**: the `activated` bit (the machine's flag
  waits for the focus-driven activation line), the server-initiated
  `close` event, the operator-side space *switching* (`Spaces::switch`
  is built; the wire has no request — the taskbar's line), the
  interactive move/resize vocabulary the spec never grew, the
  transitions catalog's geometry drivers (the render-thread line),
  dma-buf create, Vulkan, TLS, the latency lab.

## [0.16.0] — 2026-10-02 — The knock and the goodbye

Phase 48: the frozen shell surface's two honest refusals, closed as
mechanism — the **dialog** a window's flow asks with, and the
**close fade** a window leaves with. Zero wire surface moved
(`get_dialog` and the `ldp.shell.dialog` interface have been frozen
in the v1 spec from the start; the freeze gate is untouched), and
the default desktop is byte-identical by construction — the close
fade is opt-in choreography, exactly the open fade's doctrine.
2,201 tests, up from 2,183.

* **The dialog arm served** (`shell.get_dialog`): the transient
  role attaches to a parent *toplevel* (the window, not the object —
  a re-minted toplevel keeps its dialogs); the machine
  (`ldp-shell`'s `Dialog`, built and tested for years, finally
  driven) carries the modality and its own serial clock — the
  dialog's two-phase commit validates against its own proposals (a
  stale `ack_configure` serial is `invalid_state`, the strict
  contract, not the popup's advisory placement). The initial
  configure proposes the client's own size (0x0, the xdg doctrine);
  the **centered placement** lands at the first attach
  (`Dialog::center_over` — the parent's content center, clamped into
  the usable area), riding the pending queue so the dialog never
  lands origin-then-jumps. A surface may take at most one role —
  the spec's clause, now enforced by name: toplevel, popup, dialog,
  and subsurface each refuse the second claim with `invalid_state`.
  `set_title` serves the bounded-string doctrine (an embedded NUL
  is `invalid_string`; over-length is the client's own
  `string_bytes` budget, rejected before the wire).
* **The modal gate** (the spec's clause): a *mapped* modal dialog
  gates input delivery to its parent's whole tree. The routing view
  filters the parent root and the popups rooted under it (the
  router routes roots — the gate is the membership the scene
  supplies); the gated keyboard focus holds no keys; the dialog's
  mapping commit retargets the parent's focus to the dialog
  (keyboard leave + enter over the real wire). The gate lifts the
  moment the dialog closes or dies. Modeless dialogs gate nothing
  — the control half of the same session.
* **The sheet-dies-with-window doctrine**: a dialog cannot outlive
  its parent — the parent surface's death closes every dialog it
  owns (`dialog.close`, freshly-closed only — idempotent, never a
  double event); the dialog's own surface death closes its machine
  the same way. The entry stays until the object destroy (the
  popup host's done-but-alive doctrine: a close-but-alive dialog
  keeps answering `ack_configure`).
* **The window-close fade** (the transitions catalog's close arm):
  a window leaving the desktop — destroyed by the client, or
  unmapped by a detach commit — fades out under the catalog's close
  spring over an **owned ghost**: a row-tight copy of its last
  committed raster (the pool's mapping is the client's to reuse
  the moment the release lands — the copy is the only honest
  carrier), its frozen style/color/transform, and its **z slot**
  (inserted at the window's own render-order index — a fading
  window never jumps above the windows that were above it; the
  z-fidelity proof pins it under a covering window). The ghost
  renders as a compositor-owned layer (the dock's own pattern:
  owned ink, a borrowed view, `NoFb` facts — never a plane
  offload). The damage pass claims each live ghost's rect plus its
  frozen style's effect rect, and — the load-bearing detail the
  z-fidelity test caught — **the vacated rects the settle drains**
  (`GhostHost::vacated`): a ghost is nobody's surface, no client
  frame economy re-renders its region, so the removal claim is the
  host's own, or the ghost's last ink stays on the canvas while the
  hardware planes re-assign around the hole. The settled desktop is
  the plain post-destroy bytes — the A/B byte oracle against a
  transitions-off bench. Popups and ephemeral roles leave no ghost
  (instant dismissal is their contract); with transitions off (the
  library default) the destroy behaves exactly as it always has.
* **Tests**: 6 `DialogHost` unit tests + 7 `dialog_session` real-
  socket tests (the mint/ack/centering pixel oracle, the
  second-role refusal, the stale-ack refusal, the NUL title, the
  modal gate's full narrative, the parent-death close, the modeless
  control) + 5 `ghost_session` real-socket tests (the destroy fade
  with the A/B oracle, the unmap fade, the off-by-default identity,
  the z fidelity, the popup/tooltip instant-dismiss).
* **Honest remainders** (the roadmap's named lines): the toplevel
  states arm (`maximize`/`fullscreen`/`minimize`/`ack_configure`/
  `set_workspace` still consumed — the wire shape is frozen and the
  `ldp-shell` machines are built, waiting for their wiring), the
  transitions catalog's remaining drivers (workspace/app-switch/
  fullscreen/display/lock — the geometry slide and genie ride the
  render-thread line), the dma-buf create arm, TLS transport, the
  real-panel latency lab.

## [0.15.0] — 2026-10-02 — The semantic scene

Phase 47: the LionOS display architecture's own identity, closed as
mechanism — **a display server that manages a semantic,
security-aware, GPU-optimized scene rather than merely compositing
application buffers.** The wire surface grew by exactly three
toplevel requests (appended after the frozen opcodes, the Phase 45
doctrine — the freeze consciously re-taken: 95 requests + 122
events, 36 enums), and four subsystems earned their teeth:
semantic surfaces (role, security class, scene profile — the
server's invariants on top: the lock role floors the security
class), the adaptive frame scheduler's per-surface budget floor
(gaming 1 ms, creative 4 ms, desktop the operator's own — the
identity that keeps every pre-existing golden byte-pinned), the
composition decision engine (the four paths as a first-class
vocabulary with an auditable cause ledger), and the
compositor-owned transitions catalog (nine kinds, macOS-grammar
springs, the window-open fade settling to byte-exact plain ink).
2,183 tests, up from 2,148. Zero existing pixel oracles moved —
the default desktop is byte-identical by construction.

* **The semantic triple** (`toplevel.set_semantic_role`,
  `set_security_class`, `set_scene_profile`): the client claims what
  its surface *is* (window, dialog, tooltip, overlay, lock), what it
  may *expose* (normal, private, protected, system), and how the
  machine spends its frame budget on it (desktop, creative, gaming).
  `ldp-shell` grew the typed vocabulary; the scene carries the claims
  in the `material_requests` pattern (one side-map, dying with the
  surface). The claims have teeth, not vibes: tooltips and overlays
  never animate in; the lock role floors the security class at
  protected (the lock screen is never capturable, whatever a client
  claims beneath).
* **The security-aware capture** (`Protected`/`System`): the
  protected surface's rectangle paints opaque black in every
  client-visible capture frame — the display itself keeps showing
  the content (the user's own eyes), the screenshot does not. The
  compositor is the enforcement point *because* it controls what
  becomes visible; the `Private` tier classifies for the
  screen-share negotiation (the broker-era line, honestly
  documented). The proof is the A/B/A byte-equality:
  claim → black; clear → the pixel-exact bytes restored.
* **The budget floor** (the adaptive scheduler's per-surface
  doctrine): the profile floors the deadline walk's admission
  predicate — `max(config.min_commit_lead_ns, profile.budget_ns())`.
  `Gaming` (1 ms): the tightest admission that still guarantees a
  makeable deadline — latency through reliability, the whole point.
  `Creative` (4 ms): stable pacing over single-frame latency (an
  editor that misses is worse than an editor that is one frame
  late). `Desktop`: no floor — the operator's configured policy
  *is* the doctrine. A live registration keeps the contract it was
  answered with; the replay vocabulary records `SetProfile` (codec
  tag 8, backward-compatible with every existing corpus); the
  priority ladder's every rung is the module docs'
  enforced-mechanism table (Immediate mode, quiescing, the
  reserved cursor plane, the Liquid tier, the VRR window).
* **The composition decision engine** (`ldp-planes::decision`):
  `CompositionPath` — **DirectScanout** (the client's buffer is the
  panel's), **HardwareOverlay** (the fullscreen-video shape on a
  format-refusing primary), **SplitComposition** (canvas prefix,
  overlay suffix — the Windows MPO underlay), **FullComposition**
  (the canvas is the frame) — plus the `DecisionRecord` carrying
  the shape counts and the demotion ledger's first *cause*
  (BelowSplit entries are consequences by construction; the record
  skips them to name the honest reason). Golden tests pin each path
  against canonical stacks.
* **The compositor-owned transitions**
  (`ldp-compositor::transitions` + the serve loop's animation
  self-wake): the catalog names nine kinds — window open/close,
  workspace change, app switch, fullscreen, display
  connect/disconnect, lock/unlock — each with its spring curve
  after the macOS motion grammar (critically damped state changes,
  gently bouncy play). The v1 driver serves the **window-open
  fade**: the mapping commit begins it; the pump advances it at the
  driver's clock; the serve loop's poll cadence self-wakes a
  desktop whose clients all sleep (`pump_animations` under the
  reserved `ClientId::SERVER` identity — every emission parks in
  its owner's outbox, the system wake never swallows events); and
  the settled frame is **byte-identical with the never-animated
  one** (settle is removal, the terminal value is exact). The
  library default is off — plain pixels, every equivalence oracle
  intact; `--transitions` opts the choreography in.
* **Honest remainders** (the roadmap's named lines): the geometry
  drivers (the close/workspace/display/lock curves ship, their
  slides ride the render-thread line), the private tier's
  screen-share negotiation (broker-era), the dialog role's own arm
  (`get_dialog` still refused honestly), per-surface home-output
  schedulers.

## [0.14.0] — 2026-10-02 — The foundry completes

Phase 46: the mode foundry's own named remainder, closed whole.
Phase 42 poured one family — CVT reduced blanking v1, the
digital-panel doctrine — and the roadmap's honest remainder named
the rest: the GTF and CVT *standard*-blanking families for legacy
analog sinks, and CVT-RBv2's 80-pixel blank for the DisplayPort
deep-color era. This release pours all of them, spec-faithful to
the letter — the VESA CVT 1.2 and GTF 1.1 documents were fetched
and followed formula by formula (every rounding rule, every
constant, every table row), the arithmetic is exact integers
throughout (no floats — the bit-reproducibility doctrine), and the
clock stays the foundry's own doctrine (`ceil` to integer
kilohertz, never under the asked rate — finer than the spec's own
0.25/0.001 MHz grids, the divergence documented where it lives).
Zero wire surface moved — the freeze gate untouched, the foundry
is arithmetic, not protocol. 2,148 tests, up from 2,126.

* **The family grammar** (`--synth WxH[@Hz][:family]`, default `rb`
  — Phase 42's pour unchanged, its pinned report strings
  byte-identical): one spelling, five standards.
  `rb2` (CVT 1.2 §3.4.3, reduced blanking v2): the 80-pixel blank
  (front porch 8, sync 32, back porch 40 — the sync's trailing edge
  parked at the blank's center), the fixed 8-line vertical sync
  (Errata E2: 8 *regardless of aspect ratio*), the fixed 6-line
  back porch, the front porch as the 460 µs remainder — and
  **1-pixel horizontal precision** (§3.4.3 item 3: the 1366-class
  widths pour exactly; no character grid). The economy is real and
  measured: 1080p60 pours 2000×1111 at 133 320 kHz — RB's own
  vertical total with **3.8% less clock**; 4K60 saves 2%.
  `rb2v` (§3.4.3 item 1's video-optimized multiplier): the
  1000/1001 rate — `--synth 1920x1080@60:rb2v` serves the 59.94 Hz
  class, geometry bit-identical to `:rb2`, only the clock moved
  (the spec's own guarantee, pinned by test: 133 187 kHz, 59 940
  mHz realized, never under the ×1000/1001 target).
  `cvt` (§5.3, standard "CRT" blanking): the GTF blanking machinery
  the family descends from (the duty-cycle equation `C' −
  M'·H_PERIOD/1000` with the §5.2-mandated defaults M = 600, C = 40,
  K = 128, J = 20), the 550 µs vertical sync + back porch, the 20%
  blanking floor, and Table 3-2's **aspect-mapped** vertical sync
  (4:3 → 4, 16:9 → 5, 16:10 → 6, the 1280×1024 and 1280×768
  special cases → 7, non-standard → 10 — the spec's own table
  verbatim; the common open-source `cvt` tool approximates it by
  height, a named divergence, the spec text wins).
  `gtf` (VESA GTF 1.1 §7.3, the 1999 formula): the 1-line front
  porch, the fixed 3-line sync, `ROUND` semantics where CVT rounds
  down, and the period *refinement* step — which collapses
  algebraically to `1/(rate × vtotal)`, exact in integers.
* **The anchors, hand-derived** (the spec's own spreadsheet
  arithmetic reproduced with exact rationals, then pinned): 1080p60
  pours **four distinct rasters** — RB 2080×1111 (138 653 kHz),
  RB2 2000×1111 (133 320), CVT 2576×1120 (173 108, vsync 5, vfp 3,
  vbp 32), GTF 2576×1118 (172 799, vsync 3, vfp 1, vbp 34) — the
  analog pair sharing the 656-pixel blank (120/208/328) the duty
  cycle pours for that geometry and disagreeing everywhere else;
  the clock economy ordered RB2 < RB < GTF < CVT. The classics fall
  out naturally: CVT-CRT 640×480@60 pours the industry-famous
  800×500 VGA raster through the 20% floor branch (clock exactly
  24 000 kHz), GTF 1024×768@60 pours the 1344×795 the `gtf` tool
  itself prints, and CVT-CRT 1280×1024@60 carries the Table 3-2
  special case's 7-line sync.
* **The honest refusals, each family's own**: the shared vocabulary
  (zero axes, the 16-bit wire, the absurd-refresh denominator
  collapse) refuses in all five; GTF adds two the others do not
  have — the `V_SYNC_BP < 3` band (ROUND would leave the back porch
  negative; the formula's honest domain limit) and the duty-collapse
  band (the refined period past 100 µs) — and the distinction is
  the *point*: CVT's 20% floor serves the same ask GTF refuses,
  the dividend the floor exists to pay, pinned by test.
* **The pour is the same pour**: every family rides the foundry's
  two existing gates (the EDID range-limits ceiling — the typed
  `SynthError::ClockCeiling` refusal, never a silent clamp; the
  engine's `USERDEF` commit validation), joins the protocol face's
  advertised mode list (`xrandr --addmode`'s story), and migrates
  with the re-pour doctrine — the foundry's front door is the serve
  layer's own. The `Degenerate` refusal grew the family's name (the
  error says which standard would not serve the ask).
* **The proofs**: 29 tests in the foundry (anchors, the aspect
  table, the cell-floor/round distinctions on the 1366 ask, family
  distinctness and dispatch, never-under sweeps across every family,
  the degenerate bands, the CLI grammar with its honest refusals);
  the serve-level family gate (`synth_pours_every_family_through_
  the_gate` — RB2/CVT/GTF pours under the eDP panel's declared
  ceiling); and the session-level truths over the real socket
  (`every_family_pours_end_to_end` — the engine's `USERDEF` gate
  passes the analog raster's commit, a bound client hears the pour,
  and the GTF desktop *presents* onto 2576×1118 with pixel truth;
  `the_video_optimized_rate_serves` — 59 940 mHz on the wire).
* **The new honest remainder, named**: RBv3 and Optimized Video
  Timings (CEA-861-H/I's successors to the blanking story — every
  family the *VESA* standards define is poured; the CTA's own
  remain), plus interlaced pours and margins (the spreadsheet
  options no modern panel wants and no giant's custom form offers).
  The foundry's module doc carries the full ledger, including the
  two divergences that are doctrine rather than oversight: the
  exact-integer clock (never under, where the VESA grids round
  down) and RBv1's shipped 4-line sync (the de-facto ecosystem
  convention, kept).

## [0.13.0] — 2026-10-02 — The nap, the claim, and the rebuild

Phase 45: four features the giants own and this repository did not,
each one a *named line* in the comparison documents, each one closed
the way the project closes everything — mechanism first, honest
semantics, proof over the real wire. The macOS power story
(**occlusion quiescing** — App Nap's "the server knows what is
occluded" — now the scheduler's own contract: a fully-occluded
window's frame requests park, its live registrations die with
`surface_hidden`, and the reveal answers the parked request at the
moment it can present). The macOS identity story (**per-surface
material requests** — NSVisualEffectView's doctrine over the wire:
`toplevel.set_material`, the freeze re-taken consciously for a
purely-additive request, VibrantDark's first call site, an A/B/A
byte-equality proof). The Windows failure story (**the DWM-style
session-rebuild supervisor** — `lion-supervisor`: kill -9 the
compositor, the pinned socket re-binds, a reconnecting client
presents again; the session flashes and rebuilds, the desktop
survives its compositor). And the input latency story's named
remainder (**touch/tablet axis-metadata coalescing** — shape and
orientation are per-contact position state of their own kinds, the
pressure/tilt doctrine verbatim). 2,126 tests, up from 2,121; the
wire surface moved *once*, consciously, additively (the freeze gate
re-taken, the opcode tables dense, no existing opcode moved).

* **Occlusion quiescing — the App-Nap doctrine** (the damage pass
  is the authority, the scheduler is the enforcement): a surface
  visible on **zero** outputs — fully occluded behind a declared
  opaque footprint — has its scheduler slot hidden at the pass
  (`visibility_entries`' transition check, change-driven through the
  route's `quiesced` flag, free on the steady frame); the hidden
  slot's live registration terminates with `SurfaceHidden` (the
  event the wire always carried and nothing ever emitted for this
  reason), its later `frame` requests **park** — no deadline is
  handed to a client whose pixels cannot reach the panel, the same
  doctrine the parked output serves — and flips never answer them
  (the timeline being live says nothing about the surface's own
  visibility; only the unhide does, which both re-enables
  presentation and answers what parked, at the moment the surface
  can present again). Only **mapped** surfaces quiesce (the
  App-Nap predicate is "hidden on the desktop", not "not yet
  drawn" — the frame-before-first-commit choreography is unchanged,
  pinned by the hardware-renderer suite). The contract test and the
  golden timeline were consciously re-pinned (the parked request's
  budget is measured from the answer moment — the honest remaining
  time); the end-to-end proof is
  `tests/occlusion_quiesce.rs`: the occluder's declared opaque
  footprint, the occluded window's `leave_output` +
  `frame_dropped(surface_hidden)`, the parked request surviving
  flips, the reveal's answer, and the restored presentation cycle —
  over the real socket, the deterministic render, and the pixel
  truth of the reveal repaint (no client round-trip: the desktop
  repainted from live state, the LDP clients-own-their-buffers
  doctrine paying its dividend).
* **The per-surface material request** (`toplevel.set_material`,
  Phase 45's protocol amendment): the client claims the Liquid
  material for its own chrome — `default` clears the claim (the
  server's own resolution rules dress the window again), the
  family's five materials arrive on the wire as the schema's
  `material` enum, and the request is **appended after the frozen
  opcodes** (the surface extends, never reshuffles: no existing
  opcode moved, the freeze gate re-taken consciously — the drift
  suite, the round-trip inventory, and the conformance floors all
  re-pinned to 92 requests + 122 events). The machine vocabulary
  grew the typed handle (`ldp-shell`'s `Toplevel::set_material`,
  the `Material` enum with its wire mapping); the compositor maps
  the toplevel object back to its surface (the role registry the
  shell always had, now consulted), applies the request to the
  scene (dirtying it — the changed chrome's repaint is the
  *server's*, the Liquid spread recomputing its effect rects), and
  `surface_style`'s single resolution consults the request at both
  of its call sites (the damage spread and the layer assembly —
  the two truths cannot diverge). The server keeps its two
  invariants (fullscreen-opaque stays plain; a popup is the menu
  glass) and the quality budget (Minimal dresses everything plain
  — the tier is not the client's to override).
  `tests/material_session.rs`'s new proof is the A/B/A: a
  translucent window's sheet baseline, the requested VibrantDark
  (the dark vibrant frost — the family's first *call site* for the
  material the renderer carried unused since Phase 40), and the
  clear restoring the baseline **byte-identically**.
* **The session-rebuild supervisor** (`lion-supervisor`, the DWM
  doctrine): a std-only supervisor process that owns the session
  identity (the pinned abstract socket) and the rebuild policy,
  and treats `lion-compositor` as the replaceable half — the
  Windows split at the driver boundary, here at the process
  boundary. Crash (non-zero exit or a signal) → backoff → re-exec
  with the same pin and a fresh epoch (`LDP_SESSION_EPOCH`, passed
  to the child); clean exit (0) → the session is over, exit 0 with
  it; SIGTERM to the supervisor → the stop forwarded (the audited
  `sys::terminate_child` — the crate's one home for `unsafe`),
  the graceful window honored, exit 0. `--max-restarts` bounds the
  crash loop (0 = unlimited, the DWM doctrine), `--pid-file` opens
  the operator/test window, `--compositor` names the binary, and
  the pass-through after `--` reaches the child verbatim (a
  pass-through `--socket` refused — the pin is the supervisor's
  alone). The proof is `tests/rebuild_session.rs` over **real
  processes**: the compiled supervisor spawns the compiled
  compositor, a client presents, `kill -9` lands on the compositor
  (the harsh death — no teardown, no last words), the supervisor's
  rebuild log names it, the same abstract name serves again, a
  reconnecting client presents again, and the operator's SIGTERM
  ends the session cleanly. The failure-stage verdict's named line
  in `docs/comparison-windows-macos.md` is served.
* **The axis-metadata coalescing** (the input row's named
  remainder): `touch.shape` and `touch.orientation` — the contact
  ellipse's geometry — are per-contact position state of their own
  kinds (`POSITION_SHAPE`/`POSITION_ORIENTATION`, the outbox's
  kind vocabulary), the pressure/tilt doctrine verbatim: a 120
  Hz-class flood of shape and orientation samples between display
  frames collapses to the contact's freshest ellipse and angle,
  never a queue of stale ones, never one axis riding another's
  slot, the discrete down that opened the contact delivering in
  full. The proof (`input_session.rs`) floods sixteen batches of
  motion + shape + orientation for one contact and asserts the
  freshest of each — one shape, one orientation, one motion, one
  down, one frame.
* **Two packaging glitches, found and fixed** (the zip's own
  corruption, caught by the first build): `ldp-audit`'s thin argv
  adapter was an empty file in the release archive (the library
  half intact) — restored from the sibling pattern; and seven test
  sites plus the tools-live harness read `CARGO_BIN_EXE_*` at
  *runtime* (`std::env::var`) where cargo's contract freezes the
  values at *build* time — all converted to the canonical `env!()`
  form (the tools' closed seven-binary set behind a compile-time
  lookup table), which is also the more honest reading of the
  contract everywhere.

The honest remainder, named: the dedicated render thread (the
single-mutex doctrine is load-bearing for the byte-exactness
oracles — the pipelining line stays open), the dma-buf
negotiation's wire surface (the frozen `ldp.core.dmabuf` interface
awaits its honest `create` arm), Vulkan, TLS, and the real-panel
latency lab — the giants' bought-with-time list, unchanged.

## [0.12.0] — 2026-10-01 — The quiet frame and the architecture answer

Phase 44: the damage engine's quiet-frame economy, measured; the
fuzz-gate harness hardened by its own soak; and the comparison the
field has been asking for, written down at the depth the question
deserves. The release's spine is one number: a 64-window desktop
that has *nothing to do* still pays a damage pass every commit
batch, and that pass measured **94,720 → 44,609 ns/op (−52.9%)**
against the pristine v0.11.0 engine on the identical input — the
one-moving-window load drops **126,906 → 77,199 (−39.2%)**. Zero
wire surface moved (the freeze gate is green); every equivalence
the fast paths claim is pinned by new regression tests and the
per-pixel corpus that already stood under the algebra. 2,121
tests, up from 2,116.

* **The quiet frame** (`ldp-compositor`'s damage walk, five fast
  paths): the *below-unions* table — one deep region clone per
  surface per frame, O(n²) rect copies on a desktop whose opaque
  flips number zero — is now a single backward accumulation pass
  that snapshots the union *only at flip nodes* (the rare
  surfaces whose opaque footprint actually moved); the R2 coverage
  algebra runs only for surfaces whose bounds changed (old-only,
  new-only, and the offset test all collapse to the empty region
  when bounds are equal — four region allocations per static
  window per frame, gone); the R3 flip algebra runs only for
  surfaces whose opaque footprint changed (a same-length rect
  scan replaces two subtracts and two intersects); the
  fully-visible scanout check is a one-rect comparison instead of
  a subtract-and-emptiness round-trip; and the pass records skip
  surfaces whose record would not change *and* whose accumulated
  damage is empty — the guard's damage term is load-bearing (a
  quiescent surface with pending damage still records, because
  `record_pass` consuming damage is the whole point), and the
  occlusion term is load-bearing (a surface whose occlusion
  changed with nothing else moving relatches, so the next frame's
  R1 subtract reads the truth). Three regression tests pin each
  guard's exactness: `quiet_frames_stay_empty_and_consume_damage_
  exactly_once`, `occlusion_change_alone_relatches_the_pass_
  records`, and `partially_occluded_roots_are_not_scanout_
  candidates`.
* **The delivery path's last clones** (`lion-compositor`'s frame
  loop): the repaint region moves into the working set instead of
  cloning whole (the pass struct keeps only its visible map
  afterward), and the per-route entered set is taken and
  reassembled instead of cloned per frame — the visible-mirror
  oracle and all session proofs unchanged.
* **The fuzz-soak glitch, found and fixed** (the discipline
  working as designed): `LDP_FUZZ_SCALE=8` (eight times the CI
  budget — the gate's own documented soak mode) found the
  transport fuzzer panicking inside `chunk_stream`: a mutated
  stream can park at *exactly one byte*, and the interior-cut draw
  `below(len - 1)` ran against a zero (want-chunks is always ≥ 2,
  so any one-byte source panicked). The empty-source guard
  predated the fix; the one-byte guard now returns the byte as
  its own chunk (concatenation contract preserved), the latent
  sibling in `mutate_bytes`' splice case (an empty *source*
  lineage drew `below(src.len())` against zero) is fixed the same
  hour, and the gate now runs clean at **20× scale** — 300,000
  codec iterations, 300,000 X11, 300,000 Wayland, 30,000
  transport, 5,000 dispatch connections. Two regression tests
  pin both mutator edges.
* **The architecture answer** (`docs/comparison-windows-macos.md`,
  new): the comparison the matrix always pointed at, written at
  pipeline depth — ten stages from the client's pixels to the
  lit panel, Windows' DWM/WDDM/DXGI/DirectComposition and macOS'
  WindowServer/CoreAnimation/IOSurface/Metal walked beside LDP's
  own stack, each system's mechanism set named from public
  behavior (the closed halves say "closed" and refuse to guess),
  an honest ledger of what only time buys (the vendor quirk
  decades, the ecosystem, the compute arm) and what only this
  repository has (byte-exact oracles, the frozen contract, the
  readable source, the no-GPU path as a first-class citizen), and
  a stage-verdict table that prices every gap with the roadmap
  line that closes it.
* **The benchmark ledger grows the damage rows** (`ldp-bench`,
  fresh full-size release run): "damage: quiet 64-window desktop
  pass (no changes)" and "damage: one moving window among 64
  (1 px/frame)" — the two workloads every desktop runs between
  its moving frames, now first-class measured rows with the A/B
  committed beside them.

## [0.11.0] — 2026-10-01 — The contact doctrine and the living registry

Phase 43: two of the roadmap's own named remainders closed in one
release, plus the deepest hardening pass since the crash corpus.
The input-latency row's named gap ("the touch/tablet position-state
coalescing — the pointer's doctrine extended to per-contact keys")
is now served end-to-end; the protocol-design row's named future
line ("dynamic globals — registry `global_remove` + re-advert") is
now the *living registry*: the advertisement set tells the truth
about the display set while sessions watch. Neither cost a byte of
wire surface: the frozen v1 snapshot is untouched, `ldpc snapshot
--check` still passes, and the whole release rides server policy
and emission doctrine. 2,116 tests, up from 2,107.

* **The per-contact coalescer** (`ldp-compositor`'s `CoalesceKey`
  grows a `sub` discriminator; `lion-compositor`'s emission sites
  classify the contact-carrying streams): a 120 Hz touchscreen
  parking sixteen two-finger batches between display frames now
  delivers TWO motions — each finger's own freshest sample in the
  slot that finger's first sample took, one frame terminator, every
  down intact — where the pre-43 outbox delivered all thirty-two
  (the touch family fell into the discrete catch-all: never dropped,
  never replaced, never collapsed). The doctrine is the pointer's
  own, extended honestly: `touch.motion` is position state keyed
  (object, motion, contact); `touch.frame` is the shared per-object
  terminator; `touch.down`/`up`/`shape`/`orientation` are discrete
  input that seals *their own contact's* stream (a finger's down
  never freezes another finger's sample); `touch.cancel` seals
  nothing — a new sequence must begin with a `down`, which carries
  its own seal, so a cancel-barrier would swallow the *next*
  sequence's first sample into a pre-cancel slot. The tablet's
  per-tool axes are position state of their own kind (pressure,
  tilt, rotation, slider — a 240 Hz pen stream collapses to one
  motion, one pressure, one tilt, one frame); `tablet.wheel` rides
  the delta doctrine (`relative_motion`'s: every step delivers);
  tool lifecycle and buttons seal their tool's stream. Proven at
  all three tiers: the library key arithmetic (`coalesce.rs`), the
  outbox doctrine (four new inline proofs), and the session flood
  over a real socket (`latency_session.rs` — two fingers, one
  drain, two motions each carrying its own freshest coordinates).
* **The living registry** (dynamic globals, Phase 44's own
  mechanism): the dark state now *withdraws* the output global and
  the relight re-advertises it, so a client's registry stream is
  the display topology's changelog — not a static burst that goes
  stale the moment the last monitor unplugs. The ordering is the
  spec's own sentence, served exactly: every live object of the
  interface is revoked first (`revoked_reason::interface_removed`),
  then `registry.global_remove` lands on every live registry object
  (the zero-argument barrier the frozen v1 wire always declared);
  re-advertisement replays `registry.global` with the schema's own
  version range. The seam is a new `Dispatcher::on_registry` hook
  (the same shape as `on_bind`: default no-op, the static server
  unchanged) so the compositor learns each session's fan-out
  targets; binds that race a withdrawal are answered with the
  withdrawal's own vocabulary. The ldp-client `GlobalRemove`
  decoder was **drift-fixed** to the spec's zero-arg shape (it
  historically demanded a `String` argument the snapshot never
  declared — the first conformant emission would have failed the
  decode). Pinned by the new `the_registry_tracks_the_display_set_
honestly` session proof: revoke-before-barrier ordering, the
  withdrawn bind, the relight's re-advertisement, the fresh bind's
  cascade, and a second dark round — plus `ldp-server`'s
  handshake-side hook proof.
* **The hardening wave** (the deepest since the Phase 19 crash
  corpus; every fix carries its regression test):
  the **subsurface depth cap** (`MAX_SUBSURFACE_DEPTH = 64`) — a
  client-authored chain tens of thousands of links deep turned the
  next damage pass's recursive walks into a stack overflow, a
  deterministic server kill from protocol-legal requests; the cap
  refuses the link at creation, typed (`TreeError::TooDeep`), the
  same error path an unknown parent takes. The **cross-client
  pending-attach wipe** — `drop_client`'s map-wide `clear()`
  discarded a *surviving* session's in-flight attach (its next
  commit attached nothing, no error delivered); the sweep is now an
  orphan-only retain. The **saturation doctrine extended to
  occlusion** — subsurface position accumulation down a chain now
  saturates like the very next line's `Rect::translate` always did
  (a client parking `i32::MAX` twice no longer panics the walk in
  debug or wraps release geometry). The **disjoint-rect union-find**
  — the GL path's damage merge restarted its whole double scan
  after every merge (O(n³): a legal 4096-rect overlapping damage
  list parked `begin_frame` for minutes from one request); the
  union-found form examines every pair exactly once and computes
  the same canonical fixpoint. The **bounded damage accumulation**
  — the per-request 4,096-rect limit never bounded the *accumulated*
  region between commits (millions of rects parked by a client that
  never commits); past twice the per-request cap the pending region
  folds to its own bounding box, a conservative superset that is
  always-correct repaint. The **replay allocation guard** — the
  decoder sized its vectors from untrusted counts before the
  remaining bytes could refute them (the codec's own §9 doctrine,
  applied; a `u64::MAX` count is now the typed `Truncated` refusal).
* **The delivery-path economy** (measured on the same 2-vCPU rig;
  every oracle kept byte-exact): the transport writer's per-chunk
  front `drain` memmoved the whole congested tail on every
  EAGAIN-sized accept — a 2 MiB ceiling draining at 4 KiB chunks
  copied ~500 bytes per byte sent; the unsent-front cursor compacts
  only when the dead prefix is both ≥ 64 KiB and at least half the
  buffer (amortized ≤ 1 byte-move per byte sent, zero
  reallocation). The frame loop's delivery copy is now a borrow
  (the renderer's readout was always a `Cow::Borrowed`; the
  `to_vec()` was a full-canvas copy per frame — 8 MiB at 4K — whose
  only reader could borrow the same slice). The display model is
  allocation-stable: the output slot's own `Vec` carries the base
  (capacity persists across frames) and the planes blend in place —
  the historical path allocated and copied a fresh full canvas
  every frame to produce bytes the caller already held. The
  plane-blend inner loops clip their row/column ranges once, not
  per pixel.

The scores this moves: **Input latency & frame pacing** 90 → **93**
(the touch/tablet remainder the row itself named — now a lead over
every contender's 90); **Protocol design** 92 → **94** (the last
named "future" line of the registry's own contract served); the
engineering-controlled average follows the rows.

## [0.10.9] — 2026-10-01 — The mode foundry

Phase 42: the display-size field's answer — **"every display size"
made literal, with arithmetic instead of a quirk table**. The
comparison row's own sentence priced the remainder: "the giants keep
their 90s on decades of EDID/timing quirk coverage — that depth
cannot be simulated." This release ships what *can* ship honestly:
the timing machine itself, the EDID timing parse, and the quirk rows
that make the foundry's diagnoses escapable — lifting the field
88 → **90**, a tie with macOS and Windows. 2,107 tests, up from
2,084.

* **The mode foundry** (`--synth WxH[@HZ]`, `ldp-display`'s
  `timing.rs`): VESA CVT reduced-blanking timing synthesis, the
  digital-panel doctrine — the 160-pixel compact blank, the 3-line
  vertical front porch, the 4-line sync, and the 460 µs minimum
  vertical blanking period solved *exactly in integers* (no floats —
  the repo's bit-reproducibility doctrine), pinned against the
  canonical anchor (`cvt -r 1920 1080 60` pours htotal 2080 / vtotal
  1111 — both reproduced bit-exactly). The clock doctrine is
  exactness, not the grid: `ceil(htotal × vtotal × refresh / 1 MHz)`
  in integer kilohertz, never *under* the asked refresh (the VESA
  tool's 0.25 MHz grid lands 1080p RB60 at 59.93 Hz; ours lands at
  60.000087 — the sub-millihertz wire residual, stated, not hidden).
  The honest remainder, named: the GTF and CVT standard-blanking
  families for legacy analog sinks, and RBv2's 80-pixel blank, are
  roadmap lines; every digital panel this project models serves RB.
* **The pour is a user-defined mode** — the kernel's own vocabulary
  (`USERDEF` type bit, the `drmModeAddMode` lineage), validated by
  the display engine at commit time against its declared synthesis
  envelope (the mock's gate mirrors the real driver's atomic check —
  the second of the foundry's two gates), and *adopted by the
  protocol face*: the poured mode joins the output's advertised mode
  list (`xrandr --newmode --addmode`'s story, protocol-side — a bound
  client sees it among the firmware's modes, flagged current).
* **The EDID timing parse** (`edid.rs`): the base block's timing half
  decoded — the detailed timing descriptors (pixel clock in 10 kHz
  units, the split-byte active/blanking geometry, the sync edges, the
  image size; the *first* DTD is the sink's preferred timing, marked
  like the kernel marks it), and the monitor range limits (the
  vertical/horizontal rate bands and the maximum pixel clock — the
  declared envelope). One honest granularity note, stated in the
  module docs: the DTD wire quantizes clocks to 10 kHz (the NTSC-rate
  fixture lands at 148 350, not 148 352 — the kernel's own decode
  takes the same step).
* **The two gates, both honest**: the connector's EDID range limits
  gate the pour *before* it is ever proposed (a clock above the
  declared ceiling is the typed `SynthError::ClockCeiling` refusal
  naming the declaration — never a silent clamp, never a truncation);
  the engine's envelope validates the `USERDEF` mode again at commit
  time. The sized miss's honest menu now teaches the escape too ("no
  2560x1440 mode … pour one with --synth 2560x1440@HZ").
* **The migration re-pours**: a hotplug swap pours the same
  synthesized raster on the next display (each connector's own
  ceiling gating its pour); a connector whose declared ceiling
  refuses falls back to the unsized doctrine — the sized doctrine's
  honesty, verbatim (the migration never dies on the operator's
  preference).
* **The EDID audit** (`edid::audit`): the bring-up diagnoses the
  connector's truth — a rotten EDID (bytes that fail their own
  checksum), a preferred-timing lie (stale firmware naming a smaller
  mode than the connector serves), a declared clock ceiling the
  enumerated list exceeds. Every finding names its quirk row; the
  rows name the operator's escape. The default desktop's selftest
  line prints them (the preset's HDMI fixture demonstrates the lie —
  its EDID prefers 720p while its connector enumerates 1080p).
* **The quirk table accrues the timing trio** (5 → 8 rows):
  `edid-rotten` (the sink's identity block fails its own checksum —
  escape `--synth`/`--resolution`), `edid-preferred-lie` (stale
  firmware — escape `--resolution`, or the pour when even the list
  is wrong), and `pixel-clock-ceiling` (the enumerated list exceeds
  the declared envelope — escape a mode under the ceiling, or the
  foundry's typed refusal). A new `Timing` quirk class joins the
  table's vocabulary.
* **The mock panels' EDIDs grew their timing truth**: the preset's
  eDP panel carries its 1080p60 preferred timing and a declared
  600 MHz envelope; the HDMI monitor carries the stale 720p
  preferred timing (the audit's fixture) and the 340 MHz HDMI 1.4-era
  ceiling — a 4K60 pour serves on the panel and is refused on the
  monitor, both gates proven.
* Docs: comparison (the display-size row 88 → 90, the reasoning
  rewritten around what shipped), architecture §14 (the foundry
  doctrine: the two gates, the exact-clock arithmetic, the
  adoption), admin-guide (the `--synth` flag, the three quirk rows,
  the audit's diagnosis workflow, the selftest transcript), user
  guide, roadmap (the timing line closed), README.
* The session proof (`synth_session.rs`, five criteria): the pour
  serves a size the list lacks (pixel-exact at 2560x1440 over the
  real socket), the pour serves a refresh the list lacks (1080p at
  144 Hz on a 60 Hz panel), the above-ceiling refusal names the
  declaration, the audit diagnoses the stale firmware by name, and
  the migration re-pours then falls back past the ceiling — all
  end-to-end through the bring-up, the protocol, and the engine's
  commit validation.

## [0.10.8] — 2026-10-01 — The quirk ledger

Phase 41: the VRR field's answer — **the multi-year driver quirk
table, answered with the structure the decades fill**. The comparison
row's own sentence priced the remainder: "the remaining gap is the
multi-year driver quirk table, not the mechanism" — the giants carry
per-vendor quirk tables behind their driver panels (NVIDIA's
per-monitor refresh-range lists, AMD's EDID quirks, Windows'
per-display VRR tuning), decades deep, NDA-flavored, none of it
reproducible in CI. This release ships what *can* ship honestly: the
ledger's structure with the first rows tabled as mechanism-grade
operator escapes — the honest floor, the LFC cadence and latch, the
sibling escape — lifting the field 82 → **85**, a tie with Wayland
and Windows. 2,084 tests, up from 2,063.

### Added — the honest floor (`--vrr-floor N`, ldp-vrr::quirk)
* **`ldp_vrr::quirk::apply_floor`**: panels whose advertised VRR
  range flickers at the bottom (the backlight pumping while the
  scanout clock stretches toward the advertised minimum rate — the
  advertising class's VRR arm, the same doctrine as `--hdr-peak`: the
  EDID claims a stretch the hardware cannot sustain) get the
  operator's honest minimum refresh rate clamped into the window —
  the stretch ceiling shrinks to `1e9/floor_hz`, the slip floor stays
  the panel's own (the operator narrows the stretch they do not
  trust, never the panel's capability). `FloorOutcome` carries the
  audit trail (passthrough / below-advertised / clamped with both
 nanosecond bounds); the errors name the operator's fixable mistake
  (`AboveNominal` — the floor exceeds the mode's own rate, the fixed
  grid would leave the window; `AboveRange` — the malformed-window
  guard).
* **The wiring is total**: the clamp runs at bring-up *before any
  consumer*, so the scheduler's widened deadline
  (`vrr_window_ns = effective_max − nominal`), the `output.vrr`
  advertisement clients pace against, and the LFC cadence's stretch
  ceiling all read the effective window — while the *device's* own
  claim stands untouched (the mock's timeline still stretches inside
  its advertised window; the giants' overrides live in the display
  stack above the driver, never rewriting the hardware). An
  unservable floor refuses boot with the typed message.
* **The per-output CSV** (`--vrr-floor 57,0`): the `--scale` grammar
  mirrored — a lone rate blankets every output, a comma list names
  one rate per output in output order (`0` that output's
  passthrough), extras reuse the last entry (the stretch rule).
  `ldp_vrr::quirk::parse_floor_list` owns the grammar; the boot
  report prints the per-output audit trail
  (`vrr: on (per-output VRR); floors [20833333 ns -> 17543859 ns, -]`).

### Added — the LFC cadence and the anti-flap latch (ldp-vrr::refresh)
* **`lfc_interval`**: content slower than the window bridges on
  repeats locked to the content's own phase — `k = ceil(P/max)`
  repeats at `P/k` apart, so every content flip lands exactly on a
  repeat slot (judder-free by construction — the giants'
  low-framerate-compensation arithmetic, "LFC at 2x content" for a
  40 Hz stream on the 48-144 panel: k=2, 12.5 ms). `None` when the
  content is in-window (the repeats are idle) or when the window is
  too narrow to bridge (the miss ruling stands); every cadence
  respects both window bounds, pinned by the table test.
* **`RefreshSelector::lfc_engaged`** — the latch: engaged by the
  first missed window, held through boundary-hugging flips (a flip
  landing within `min` of the ceiling is *not* recovery), cleared
  only by a flip landing comfortably in-window (one full minimum
  interval clear of the ceiling). The flap — content hovering at the
  boundary alternating stretch/repeat, the visible pumping the driver
  tables name — rides warmed repeats instead. `lfc_repeat_at(P)`
  answers the next aligned slot; the engine passes both through.

### Added — the sibling escape (`--vrr-uniform`)
* The mixed-desktop quirk: a fixed-sync display flickering while its
  VRR sibling stretches (the cross-CRTC clock coupling the pre-2020
  driver era worked around by disabling VRR on mixed desktops). The
  default stays the modern doctrine — per-output VRR, the
  per-display behavior Windows serves; `--vrr-uniform` collapses the
  whole desktop to one fixed sync across the seam: no output arms
  (whatever its own capability), no widening. The collapse only bites
  when the desktop actually spans the seam (at least one window, at
  least one without); the floor pass still clamps the advertisement
  (the operator's truth about the panel is the truth regardless of
  arming).

### The quirk table accrues (ldp-display::quirks)
* Five rows tabled, was three: **`vrr-floor-flicker`** (advertising —
  brightness pumping on slow content; detection distinguishes the
  deep-stretch band from the VRR mechanism itself; escape
  `--vrr-floor N`) and **`vrr-sibling-flicker`** (sync — the
  fixed-sibling flicker; detection names the coupling; escape
  `--vrr-uniform`) join `psr-flicker`, `vrr-flicker`, and
  `hdr-peak-bloat`. Rows are stable public commitments: names and
  escapes never move; the selftest prints every one.

### Proofs
* `vrr_floor_session.rs` (6, over the real socket, against the mock
  panel's own advertised 48-144 Hz window): the floor clamps every
  consumer (the wire event's 57,000 millihz minimum, the scheduler's
  narrowed widening, the slot's effective window, the audit trail,
  the report fragment — and the CRTC still arms, the device's own
  advertisement still standing); a floor below the advertised
  minimum is a no-op (the Phase 31 bytes); the per-output
  passthrough lets the window stand; an unservable floor refuses
  boot naming the mistake; the uniform collapse disarms the mixed
  desktop (the capable output's CRTC un-armed, zero widening, the
  report naming the collapse) with the per-output contrast armed
  beside it; the floor and the collapse compose (the advertisement
  clamped, the arming collapsed).
* ldp-vrr's own suites grow by fifteen: the floor's full
  cross-product (passthrough, below-advertised, the clamp's bounds,
  both rejections, the grammar's mirrors and failures) and the LFC
  set (the cadence table across five content rates, the narrow
  window's decline, the aligned slots, the latch's
  engage/hold/clear, the boundary flap held, the inert-off case).

### Docs
* comparison.md (VRR 82 → **85** — the tie with Wayland and Windows;
  totals 1,864 → 1,867; the adoption view 87.1 → 87.4, the distance
  to Windows' 90.0 down to 2.6; the determinism row's 2,084 tests
  over 219 suites), roadmap (the Phase 41 section), README (the
  v0.10.8 milestone), admin-guide (the quirk-ledger diagnosis
  workflow: symptom → detection → escape, per row), user-guide (the
  honest window on the wire and what clients should pace against),
  maturity (2,084 tests, the floor session on the matrix),
  architecture §13 (the ledger doctrine: where the clamp lives, the
  cadence arithmetic, the latch state machine).

## [0.10.7] — 2026-10-01 — The deep material

Phase 40: the visual-quality field's answer — **the glass stops being
one flavor**. The comparison row priced the remaining distance with
the honest sentence "WindowServer/DWM remain the reference ceilings
(backdrop blur everywhere)" while the worklog's own next-candidate
line named the split: "the remaining materials distance — the giants'
backdrop-blur depth and animation polish". The engine had macOS-class
materials since Phase 27 — rounded corners, memoized soft shadows,
3-pass frost, spring motion, byte-equal across backends — but three
distances to the giants stayed open: the frost's saturation dial only
*desaturated* (`0..=255`, while Windows' acrylic and macOS's vibrant
materials boost chroma *past* the backdrop's own), no material traced
the luminous hairline that makes glass read as glass, and the desktop
spoke two ad-hoc styles instead of a family — so "backdrop blur
everywhere" meant two surfaces, not the vocabulary. This release
closes all three: **the vibrant domain**, **the edge light**, and the
**material family** — menus and popups in glass, the dock in chrome,
byte-equal across software and GL by construction, pinned by
hand-computed oracles and an end-to-end session over the real wire.
2,063 tests, up from 2,050.

### Added — the vibrant domain (the saturation dial extends past identity)
* **`BackdropParams::saturation` is `u16`, `0..=510`**: the Phase 27
  semantics hold byte-identically on `0..=255` (255 = the backdrop's
  own color, the identity; lower desaturates towards luma — the
  reference cross-suite pins the legacy arm), and `256..=510` is the
  **boost**: each channel extends *away* from its BT.601 luma by
  `(saturation − 255) / 255` of its own luma-distance, rounded half
  away from zero, clamped into the premultiplied domain by the veil's
  existing channel clamps. The vibrant order is documented and
  oracle-pinned: blur, **boost**, veil. `510` (the sanitized ceiling)
  adds the full luma-distance once more — the ~2× saturation reads
  land at the top; the menu material's 383 is the 1.5× acrylic read.
* **Named materials**: `BackdropParams::vibrant_light()` (blur 22 × 3,
  saturation 383, the light menu veil) and
  `BackdropParams::vibrant_dark()` (saturation 340 under a heavy dark
  veil — the control-center glass).
* **Proofs**: the randomized fast-path-vs-reference corpus now draws
  saturation across the whole `0..=510` domain with the identity, the
  full desaturation, and the menu's boost as deterministic corners
  (the in-test reference carries its own boost arm written from the
  documented rule); the golden suite pins the boost against the
  desaturation mirror on a cherry backdrop — hand-computed, the two
  arms diverge in opposite directions from identity
  (`249/39/39` boosted vs `151/81/81` desaturated, `200/60/60` the
  identity between them).

### Added — the edge light (the luminous hairline)
* **`LayerStyle::edge_light`** (`EdgeLightParams`: color + alpha,
  width fixed at one pixel and honest about why): the stroke traced
  *inside* the rounded silhouette, drawn **after the ink** — the
  light catch that separates glass from its backdrop (WindowServer's
  material edge, acrylic's border light).
* **`edge_light_material`** (`ldp-renderer::effects`): the ring's
  coverage is exact by construction — the silhouette's own
  antialiased coverage minus the coverage of its **1-pixel Minkowski
  erosion** (rect shrunk 1 px per side, radius less 1), so the ring
  follows the corners on the same coverage curve the ink clips with;
  degenerate destinations (thinner than the stroke) are all edge.
  Ring pixels carry the stroke premultiplied by `alpha × ring`;
  the interior stays fully transparent.
* **The ring is imagery, like the shadow**: `EdgeMemo` memoizes it per
  `(dest, radius, params)` — words for the software path, texture
  bytes converted once for the GL stream — with the frost memo's
  discipline (linear scan, small cap, honest eviction: the oldest
  ring falls out, menus come and go).
* **Both backends, one draw order**: software draws the ring through
  the same damage-clipped `apply_material` the shadow and frost use
  (a post-ink stage in `submit`); GL draws the memoized texture as
  the stream's after-ink quad — the identical words, so the paths are
  byte-equal *by construction*. The steady-state transparency oracle
  rides the hairline: a warm renderer's edge memo serves the same
  bytes a cold render builds.
* **Proofs**: the SDF-derived ring oracle (every pixel equals the
  erosion difference, recomputed in the test from the SDF rules); the
  alpha-scaling identity; the golden frame oracle — straight edges
  full stroke, interior untouched, the corner's antialiased arc
  (`106`) and its soft inner ring (`85`) hand-computed; the
  randomized cross-backend equivalence corpus now generates hairlines
  on half its styled layers.

### Added — the material family (backdrop blur everywhere the giants mean it)
* **`ldp_renderer::Material`** — the named vocabulary every surface
  role speaks, one resolution instead of two ad-hoc styles:
  **Panel** (an opaque window — the Phase 27 policy verbatim),
  **Sheet** (a translucent surface — the Phase 27 policy verbatim),
  **Menu** (the popup glass: the vibrant frost, crisper corners, the
  light hairline), **VibrantDark** (the control-center glass),
  **Chrome** (the dock: the sheet's frost, the shadow cleared, the
  gentler chrome hairline). `Minimal` resolves every material to the
  plain style — the library default keeps its byte-exact legacy
  pixels; every other tier keeps the look, `Low` holding the identity
  without the blur (the veil and the hairline stand).
* **The compositor speaks it** (`surface_style`): the desktop's own
  fullscreen opaque surface stays undressed; **a popup wears the menu
  glass** — the damage expansion and the layer assembly resolve
  through the family, the popup arm keyed off the live `PopupHost`.
  The damage-equivalence constraint is pinned in tests: the menu's
  shadow parameters equal the sheet's at every tier, so repaint
  spread is a function of the shadow alone and the two call sites'
  effect rects agree by construction. **The dock wears chrome**
  (`SystemDock::style` — the Phase 28 shadow-cleared doctrine, now
  with the hairline).
* **Session proof** (`material_session.rs`): the menu glass end to
  end through the real wire — a popup over a cherry window, the
  frame's every visible word the *system's* material (the ink fully
  translucent): the interior is the vibrant frost hand-computed
  (`250/102/103` — the boost arm under the veil), the straight edges
  carry the hairline (`252/156/157`), the wallpaper's own pixels
  stand untouched, and the steady state keeps the bytes. The
  Phase 36 popup session (plain styles at `Minimal`) still passes
  unchanged — the routing is tiered, the default's bytes frozen.

### Changed
* `LayerStyle` carries `edge_light` (default `None` — plain, the
  Phase 26 bytes); `is_plain` and the sanitization contract extended
  (the vibrant ceiling `510` clamps alongside the blur and pass
  ceilings).
* The dock's style resolves through the family (`Chrome`): the same
  frost, the same cleared shadow, plus the hairline at every tier
  above `Minimal`.
* The comparison's visual-quality row: 82 → **90** — a tie with
  Windows, WindowServer the named remainder (per-surface material
  requests over the shell protocol is the roadmap line). The field
  sits outside both view averages, so the straight total moves
  1,856 → 1,864; the head-to-head vs Windows gains the tie
  (9-6-2 → 9-5-3).

## [0.10.6] — 2026-10-01 — The fresh frame

Phase 39: the input-latency field's answer — **the freshest sample
rides the frame, and the clock tells the truth about the landing**.
The comparison row priced the field at 75 with the honest sentence
"the giants keep their 90s on years of measured end-to-end tuning
(unredirection, event coalescing)"; the architecture had the
deadline grid, the measured input→photon budget, and the direct
scanout candidacy, but the two tunings the sentence named were
unserved: the router delivered *every* pointer event the millisecond
it arrived (a 1000 Hz mouse parks forty-eight messages per display
frame into a frame-cadenced client's mailbox — every one of them
stale by the time it renders), and the presentation feedback under
VRR reported the nominal grid the panel was not running (the
measured-interval band rejected the LFC fast end as a duplicate and
the stretched slow end as a stall — the panel's own window was never
consulted). This release serves both halves of the giants' tuning,
with the doctrines pinned in CI: **position-state coalescing** in
the emission path (one wake per display frame, the freshest
coordinates, the delta stream intact) and the **VRR-aware
presentation clock** (the measurement band is the panel's own
window; an adaptive surface's verdict fires at the flip that carries
its content, pacing at the fast end while it keeps up — the frame
pacing follows the content, which is the point of adaptive sync).
2,050 tests, up from 2,025.

### Added — the position-state class (§10.4's table grows a row)
* **`EventClass::InputState`** in `ldp-compositor::coalesce`: the
  class whose replacement geometry differs from every other — the
  survivor overwrites the *oldest* pending slot in place (its order
  relative to every other queued event never moves), so a discrete
  event is never pre-empted by a newer sample. The class exists for
  position state: a pointer's absolute `motion` sample and its
  batch's `frame` terminator. Unit tests pin the doctrine: the
  sixteen-batch flood collapses to one motion carrying sample
  sixteen's coordinates at the first batch's slot; the surviving
  stream is legal for every consumer (absolute consumers see the
  freshest position; batch framing survives).
* **`CoalescingQueue::seal`** — the discrete-input barrier: a
  button, an enter/leave, or any other discrete pointer event
  freezes the pending position state it rode with (the X11
  flush-before-button doctrine, expressed as a freeze — the click's
  context is the sample it queued after, never a fresher one), and
  later same-key items queue fresh slots after the barrier. Sealing
  an absent key is a no-op.
* **The served outbox is the §10.4 emission path** (the doctrine
  was library-only since Phase 7 — the live `Outboxes` was a bare
  FIFO): per-client `CoalescingQueue`s, the class declared where
  the semantics are known — `routed_entry` classifies the seat
  vocabulary (`motion`/`frame` are position state;
  `relative_motion` is plain input — every delta delivers, full
  fidelity for raw-input consumers; the discrete vocabulary seals),
  `sched_entries` keys the presentation feedback per surface and
  kind (`frame_dropped` stays unkeyed — terminal signals survive).
  The unclassified default keeps the exact pre-Phase-39 FIFO:
  nothing coalesces unless an emission site says so. Capacities:
  the never-drop classes stay unbounded in the served outbox (the
  pre-existing behavior — the transport's dead-peer teardown guards
  the pathological client), position state caps at 64, presentation
  at 512 — under pressure, superseded samples and stale feedback are
  expendable, discrete input never is.
* **Session proofs** (`latency_session.rs`, 5 tests): the flood —
  sixteen parked batches, ONE delivered `motion` (the freshest
  coordinates, matching the router's own position), sixteen
  `relative_motion` deltas, one `frame`; the barrier — a click
  between two samples delivers both motions with the click between
  them, the pre-click sample keeping *its* coordinates; the budget —
  the flood rides the next frame within one nominal period (the
  coalesced delivery changes the mailbox, never the render).

### Changed — the VRR-aware presentation clock
* **`FrameClock::arm_vrr`** (`ldp-compositor::predictor`): the
  measured-interval validity band becomes the panel's own physical
  `[min, max]` window when probed — the LFC fast end (a repeat at
  the minimum interval) and the stretched slow end are both honest
  single-period landings, where the legacy half..1.5× band rejected
  the fast end as a duplicate and the wide-window slow end as a
  stall. `presented.refresh` now reports the cadence the panel
  actually ran. The deadline grid is untouched by arming
  (predictions keep the nominal cadence — only the *measurement*
  learns the window, because only the measurement claims to report
  what the panel ran). Arming ignores windows that do not strictly
  contain the nominal period — the legacy band stands, never a
  guess. Wired from `--vrr` through `SchedulerConfig.vrr_min_ns`
  (new field; the bring-up now carries the window's *min* side
  alongside the Phase 31 max-side widening — `widen_scheduler_for_vrr`).
* **The adaptive opportunity target** (`sched_build_adaptive`): an
  adaptive-mode surface under a fully probed window targets the
  panel's next refresh **opportunity** — the earliest in-window
  landing, `last_landing + lead × min` floored at the request —
  instead of a nominal grid cell. Under VRR the panel is
  event-driven: a flip lands at the earliest legal instant after the
  previous landing, so a nominal cell may already sit *behind* the
  flip that carries the content; the verdict now fires at the
  content's own flip (`presented_at` = the honest landing), and the
  deadlines pace at the fast end while the client keeps up. The
  bump walk escalates one `min` step at a time (the same bounded
  ladder), the expiry keeps Phase 31's widened window (a late commit
  may consume the stretch), and an unprobed window (min side
  missing) keeps the nominal walk — the honest degradation. The
  pinned v1 replay format does not carry the min side: replays
  exercise decision reproducibility, and the measurement band feeds
  only the refresh *reporting* — the existing corpora stay
  byte-valid.
* **Session proofs** (`latency_session.rs`): under `--vrr` an
  adaptive surface committing at readiness lands its flips at the
  panel's minimum interval (6.944 ms on the 48-144 window — the
  mock's own landing rule, exactly as the kernel clamps) and every
  `presented` carries `refresh` 6,944,444 — the fast end the legacy
  band rejected; without `--vrr` the same client paces to the fixed
  16.667 ms grid and the feedback says so (the Phase 25 bytes, the
  contrast pinned).

## [0.10.5] — 2026-10-01 — The honest peak

Phase 38: the HDR field's answer — **the panel's real peak and the
content's own mastering metadata finally reach the ink**. The
v0.10.0 HDR pipeline *advertised* a peak and *collected* per-surface
metadata, but the two never met: the PQ canvas was the static
1000-nit description whatever the panel claimed, the metadata fed
only the mode controller's stack summary (a comment in the fold
promised "the negotiated peak … recorded for the render's
tone-mapping ceiling" — the recording never happened), and a panel
that advertises more peak than it sustains (the EDID bloated-peak
class every real HDR panel carries) had no honesty knob. This
release closes all three: the **negotiation vocabulary**
(`ldp-hdr::negotiation` — the panel's *effective* peak, the
negotiated canvas ceiling, the per-surface mastering refinement),
the **luminance tail** in the renderer's color pipeline (every HDR
layer's ink rides the BT.2390-structured knee from its declared
mastering range onto the negotiated ceiling — the system's rolloff
replaces the panel's hard clip, the "the system owns the materials"
doctrine extended to light itself), and the **operator's honesty
knob** (`--hdr-peak N` — the bloated-peak quirk's escape, the
advertisement and the ink both carrying the effective peak).
The quirk table accrues its first row (`hdr-peak-bloat`, the
advertising class), the selftest prints the HDR negotiation line,
and the session proofs pin the whole chain end to end: the same
10,000-nit code value renders as *different honest ink* for a
600-nit panel, a 400-nit panel, dimmer content, and the operator's
cap — the negotiation literally visible in the scanout (2,025
tests, up from 2,013).

### Added — the negotiation vocabulary
* **`ldp-hdr::negotiation`** (new module, pure): `PanelPeak` —
  the panel's *effective* peak, the advertisement clamped by the
  operator's cap (the min: a cap above the advertisement is the
  operator guessing brighter than the panel's own claim, and the
  claim wins); `negotiated_ceiling` — the canvas's ceiling for one
  frame, the stack's brightest content (max CLL) clamped to the
  panel (dimmer content negotiates to itself; brighter content
  maps down; unknown keeps the panel — the v0.10.3 table, promoted
  from a crate-private helper to the exported vocabulary); and
  `layer_mastering` — the per-surface refinement: the layer's
  static HDR10 mastering bounds when the client declared them, the
  description's own otherwise (the CTA `0`-unknown sentinels never
  fabricate; above-ceiling declarations fall back to the
  description — the belt under the dispatcher gate's braces).
* **The fold finally records** (`World::negotiated`): the stack
  summary's fold over the desktop now concludes the negotiated
  ceiling and hands it to the render pass — the comment that
  promised this since the v0.10.1 era is finally true. The PQ
  canvas carries the ceiling as its `luminance_max`
  (`output_desc_hdr`): the honest record of what the frame will
  hold.

### Added — the luminance tail
* **`TonePolicy`** (`ldp-renderer`): the per-layer directive the
  compositor concludes — `Pass` (SDR layers ride the BT.2408
  anchor; every pre-Phase-38 pixel oracle byte-identical), `Clip`
  (HDR content with no declared metadata — the honest default:
  never exceed the panel, nothing else known), and `Eetf` (the
  BT.2390-structured knee from the content's declared mastering
  range onto the negotiated ceiling — the per-surface luminance
  metadata refinement). `ColorPipeline::with_tone` bakes the
  policy into the linear domain: the decoded luminance (nits, via
  the target's luma row) reads the tail's scale, and the scale
  multiplies all three channels — hue preserved by construction,
  the same contract `ldp-color`'s f64 reference carries, here in
  the renderer's f32 doctrine (the renderer owns its own curves;
  the golden suite anchors the published points: identity below
  the master range, the monotone knee between, saturation at the
  ceiling).
* **The fast paths honor the tail**: a tail-bearing layer takes
  the scene-linear path whatever its description says (the
  encoded-space copies must not bypass the rolloff), and the GL
  v1 path *refuses* a tail-bearing layer typed — its
  encoded-space stream cannot carry the rolloff, and silently
  dropping the negotiated ceiling would render the wrong ink (the
  honest-boundary doctrine; the frame routes to the software
  arm).

### Added — the operator's honesty knob
* **`--hdr-peak N`** (with `--hdr`): the panel-peak truth — what
  the panel actually *sustains*, the escape for the bloated-peak
  quirk class. The advertisement (`output.hdr_caps`), the canvas's
  negotiated ceiling, and every HDR layer's tone policy all carry
  the *effective* peak: what clients can rely on is what the
  render pass will actually deliver (the negotiation's reply
  side, both directions). Validated up front (1..=10,000 nits);
  the default keeps the `--hdr` pipeline's own 600-nit
  advertisement. The selftest prints the HDR line (the effective
  peak, the operator cap marked), the usage documents the knob,
  and `CompositorConfig.hdr_peak` serves the library path.
* **The quirk table's first accrued row**: `hdr-peak-bloat`
  (class *advertising* — the table's third class): panels whose
  EDID peak exceeds what a meter reads on a full-white field;
  detection by measurement; escape `--hdr-peak N`; cost —
  highlights above the measured peak compress one stop earlier,
  honestly, instead of the panel's own late clip. The v0.10.4
  doctrine ("the mechanism shipped; the rows accrue") accrues
  its first row.

### The session proofs
* **`hdr_session.rs` grows four proofs** over the real socket:
  the negotiated ink (a PQ red quad's 10,000-nit code value rides
  the EETF — the luminance lands at the ceiling, the channel at
  the ceiling's luminance share, the model pinned to ±3 LSB by
  the independent f64 PQ encode; *not* the raw pass-through the
  panel would clip late), the dimmer content (mastering 400 →
  the negotiated ceiling 400, the ink at the content's own
  level), the operator cap (`--hdr-peak 400` → the ink and the
  advertisement both at 400 — the quirk row's route, proven end
  to end), and the advertisement's reply (`hdr_caps` carries the
  effective peak). The renderer's golden suite grows four more:
  the clip's exact ceiling code, the EETF's published anchors
  (saturation at the display peak, the monotone sweep, the
  identity segment below the knee), `Pass`'s byte-identity, and
  the GL refusal.

## [0.10.4] — 2026-09-30 — One desktop, every arrangement

Phase 37: the multi-monitor field's answer — **every way a desk can
put its displays on one desktop**. The Phase 31 doctrine served one
arrangement: extend, every display carrying its own portion of one
left-to-right desktop. This release serves the other two thirds of
the operator's real vocabulary: **`--outputs mirror`** places every
display on the *same* desktop — each showing the same content
cropped to its own mode (the overlap at the origin is itself the
protocol's clone signal, the same vocabulary X11 and the Wayland
compositors serve), the desktop staying the primary's bounds so a
mixed-size mirror crops instead of inflating, and a display joining
mid-session joining the *mirror*, not an extension. Beside it,
**per-output scales** (`--scale 1,2` — one factor per output in
output order): the eDP advertises 1x, the HDMI 2x, the positioning
shell resolving its logical canvas on the primary's own factor, and
a hotplug newcomer inheriting the doctrine's last entry (the stretch
rule). And the **quirk table**: the display ecosystem's long tail
named, each row carrying its symptom, its detection, and its
operator escape — the mechanism ships today with the two rows we can
honestly claim (`psr-flicker` → `--no-psr`, `vrr-flicker` → serve
without `--vrr`), the giants' decades of rows accruing with
hardware. The seal run's own bug hunt caught two real bugs: the
renderers parked each output's canvas under its layout *origin* — an
identity that stops being unique the moment two displays overlap,
which is exactly what a mirror is — so the 720p display restored the
1080p canvas it had just parked, truncated into the wrong shape (the
crop proof's per-pixel equality caught it; the canvases now file
under their full geometry, and same-size mirrors *share* one canvas,
which is the mirror's own meaning), and a hotplug newcomer silently
reset its scale to identity, losing the operator's `--scale`
mid-session (the stretch rule fixes it — `--scale 2` + a monitor
joining = the newcomer at 2x, not a lie at 1x) (2,013 tests, up
from 2,001).

### Added — the mirror doctrine
* **`--outputs mirror`** (the clone arrangement): every served
  display sits at the layout origin; the desktop is the *primary's*
  bounds (a union would inflate the desktop to the largest display
  and place windows into area no display anchors). Each display
  renders the same stack at its own mode — a smaller display crops
  the top-left, a larger letterboxes in the desktop's own painted
  background (the full-bounds seed paints every display whole from
  its very first flip, bring-up and hotplug alike — no fb-init
  edges ever reach a panel). The dock renders on *every* mirrored
  display (the dock is the desktop's own chrome, and the mirror
  shows the desktop; an extended desktop keeps the macOS doctrine —
  the primary's own). Visibility is honest geometry: a window
  enters every bound display object that shows it, and a buffer
  both displays scan out releases after *both* flip past it — the
  release gates span the mirror. Topology motion keeps the
  arrangement: a display leaving is `OutputRemoved` (the survivor
  keeps mirroring), a display joining is `OutputAdded` at the
  origin — it joins the mirror, never an extension.
* **The identical-pixels proof**
  (`binaries/lion-compositor/tests/mirror_session.rs`): one
  gradient window commits and both same-size displays' scanouts are
  *byte-equal* — the same words at the same offsets, the mirror's
  own meaning; the mixed-size crop proof holds *per-pixel* (every
  row of the 720p display equals the primary's first 1280 columns
  of the same row — a scaled or shifted copy could never pass); and
  the topology proof walks the unplug/replug with the newcomer
  painting whole and the mirror continuing byte-equal.
* **The arrangement survives re-arrangements**: the re-arrange arm
  resolves layouts under the world's standing arrangement — the
  extended arm appends left-to-right (the Phase 31 bytes), the
  mirrored arm holds the origin (Phase 37); the library default
  stays `Extended`, byte-identical for every equivalence corpus.

### Added — the per-output scale doctrine
* **`--scale F1,F2,…`** (one factor per output, in output order):
  output *i* advertises factor *i* over its cascade (`output.scale`,
  Q8.8), extras reusing the last entry — and a hotplug newcomer
  takes the last entry too (the **stretch rule**: the operator's
  list never runs out of factors as the topology moves). A lone
  factor keeps the Phase 31 doctrine (every output at `--scale F`),
  byte-identical. The positioning shell resolves its logical canvas
  on the *primary's own* factor — a 1x-primary with a 2x-secondary
  serves a 1920-logical desktop and the secondary's clients render
  at 2x for their display's density; the renderer stays native
  (the framebuffer is the mode's pixels, unchanged).
* **The newcomer's inheritance**
  (`World::newcomer_scale`): the rule the hotplug arm applies —
  the per-output list's last entry, or the primary's own factor
  when the list is empty. The session proof pins both shapes: the
  list `[1,2]` advertising 256 then 512 across two binds, and the
  lone `--scale 2` surviving a monitor swap with the newcomer at
  512.

### Added — the quirk table
* **`ldp-display::quirks`** — the display ecosystem's long tail as
  a *mechanism*: every tabled quirk is named (`psr-flicker`,
  `vrr-flicker`), classified (panel, sync), and carries its
  symptom (what the operator sees), its detection (how to confirm
  it is this quirk and not a bug), its **escape** (the CLI route
  around it — `--no-psr`, serving without `--vrr`), and its cost
  (the honest trade the escape makes). The doctrine is *named, not
  guessed*: a quirk is only tabled when the system can route
  around it deterministically — a quirk with no escape is a bug
  report. `quirks::report()` is the selftest's own line (the
  operator reads the table where they read everything else); the
  admin guide prints the table proper. The rows start one deep
  where the giants' are decades deep — that distance is the
  comparison table's own honest remainder, and the mechanism is
  here for the rows that accrue.

### Fixed — the seal run's own bug hunt
* **The canvas identity** (both renderers): the per-output canvas
  parking filed under the layout *origin* — unique on an extended
  desktop, meaningless on a mirror (every display at the same
  origin). The software renderer would restore the canvas it had
  just parked and *resize* it — the 720p display compositing the
  1080p canvas's first words as if they were its own rows (the
  crop proof's per-pixel equality caught exactly this mangle);
  the GL renderer destroyed and recreated a target on every
  alternation, losing every undamaged pixel. Both now file under
  the full geometry (origin, size, format): a different-size
  mirror keeps its own canvas, a same-size mirror *shares* one —
  which is the mirror's own meaning (identical content, one
  canvas). Unit-pinned in `ldp-renderer`
  (`same_origin_different_size_keeps_its_own_canvas`).
* **The hotplug scale reset**: a display joining mid-session built
  its protocol face at identity scale — the operator's `--scale 2`
  silently lost to the newcomer's 1x (a latent Phase 31 bug: the
  bring-up assigned the factor, the re-arrange path never did).
  The stretch rule fixes it; the session proof pins it.

### Changed
* `--outputs <multi|single|mirror>` (the CLI's doctrine surface
  grows its third arm); `--scale <F|F,F,…>` (the factor surface
  grows its per-output shape). Both keep their existing parses
  verbatim — the lone factor and the two-value doctrine are
  byte-identical to Phase 31.
* The selftest prints two new lines: the arrangement
  ("outputs: extended/mirrored …") and the quirk table
  ("quirks: 2 quirk(s) tabled: …") — the operator's honest
  startup truth.
* `OutputArrangement` (scene) and `output_scales` (config + world)
  are the new doctrine carriers; `CompositorConfig` grows both
  with library defaults that keep every Phase 31 byte.

### Exit criteria
* The mirror session (`mirror_session.rs`, 7 proofs, ten-for-ten in
  the flake hunt): the origin bring-up, the identical-pixels
  byte-equality, the mixed-size per-pixel crop, the topology motion
  with the newcomer painting whole, the per-output scale cascades,
  the stretch rule (and the bug it fixes), and the spanning release
  gates.
* The extended-desktop regression (`multi_output.rs`, 12 proofs):
  the Phase 31 exit criteria unchanged and green — the arrangement
  field grew without moving the doctrine beneath it.
* `verify.sh` green in-tree and from a fresh extraction of the zip:
  fmt, clippy `-D warnings`, 2,013 tests / 0 failed, rustdoc
  `-D warnings`, spec lint, dep policy, ldpc drift, the API-freeze
  gate, release coherence.

## [0.10.3] — 2026-09-30 — The rootless door

Phase 36: the ecosystem field's biggest step — **every X11 window
becomes a first-class LDP window**. The rootful bridge shipped in
v0.10.0 was "the door in the wall": the whole X screen arrived as
*one* window, a screenshot of a foreign world. This release splits
that screen at the natural seam: the rootless driver exports each
top-level X window on its own LDP surface — managed windows ride the
`toplevel` role (the positioning shell's cascade places them, the
Liquid materials dress them, they are windows among windows),
override-redirect windows ride the brand-new **popup role**
(`get_popup`'s anchor/gravity machine, the constraint solver over the
usable area — a menu is exactly what the role was drawn for). Beneath
it the compositor learned to *serve* the popup role end to end
(mint, solve at attach, `configure`/`ack_configure`, `reposition`,
`dismiss`/`done`, the parent-death sweep) — a native LDP capability
with its own session proof, not a bridge-only hack. And the seal
run's own bug hunt caught three real bugs: the bridge's pointer
`enter` events arrived at (0,0) for every surface (a latent v0.10.0
arg-lift bug — the enter carries (surface, x, y), the motion only
(x, y), and one reader served both), the X face's spontaneous output
(the input events the driver injects) parked in the server's
per-client queue until the foreign client sent its next *request*
(a menu waiting for its click would have waited forever), and the
rootless pool's growth re-zeroed the shared mirror while only the
newly minted windows re-committed — the newest window rendered, its
elders went black (the session suite caught it as a one-in-two
flake; it is now a grow-only mirror and ten-for-ten). Plus the
quiescent-keepalive doctrine: the compositor parks cross-client
events for the owner's next message, so a bridge that only *reacts*
(X drawing, presentation ticks) would never poll its input out — one
`connection.sync` per idle turn (the protocol's own keepalive, "never
advances protocol state") keeps the parked events flowing (2,001
tests, up from 1,986).

### Added — the popup role, served
* **`ldp.shell.popup` on the wire** — `get_popup`'s full service
  surface in the compositor's dispatcher: the mint validates the
  role surface and parent (both must be live surfaces of the
  connection; the parent may be null — the bar-menu doctrine, the
  anchor lives on the output itself), the machine is `ldp-shell`'s
  popup solver (`Anchor`/`Gravity`/offset/constraint grant), and the
  **placement proposal rides the surface's first attach** — the
  buffer's size is the solver's input, the position rides the
  pending queue so the menu *lands placed* with the mapping commit
  (never origin-then-jump — the same doctrine as toplevel
  placement, which popups explicitly never take). `popup.configure`
  carries the parent-relative placement; `ack_configure` accepts any
  serial the client saw (placement is advisory); `reposition`
  re-solves against the standing world and moves the mapped popup
  immediately (`set_position_now`); `dismiss` answers `done`; and
  the **parent-death sweep** dismisses every child (a menu cannot
  outlive its window) with `done` to each. `grab` enforces the
  serial-freshness doctrine (the same equality the data family's
  `set_selection` enforces; the button-press serial record arms with
  the device-event feed). `get_dialog` is honestly refused — window
  management remains the shell's own roadmap line, never a silent
  no-op. `PopupHost` (binaries/lion-compositor/src/shell.rs) is the
  policy home: 4 unit tests + the full session proof below.
* **The native session proof** (`binaries/lion-compositor/tests/
  popup_session.rs`) — a real client over the real socket: the
  parent toplevel placed by the cascade, the menu's attach solving
  to `(100, 44)` parent-relative (bottom-left anchor, down-right
  gravity), the configure/ack/commit handshake landing the menu's
  pixels at the anchor on the scanout, a `reposition` re-solving to
  `(0, 44)` and the move rendering after the menu's re-present, the
  client `dismiss` answered by `done`, and the parent's destruction
  arriving as the surviving child's `done`.

### Added — the rootless X11 driver
* **`ldp-x11-bridge::rootless` — the per-window export engine**.
  The export set is the X tree's root children (every mapped
  InputOutput window): managed windows mint
  `create_surface`+`get_toplevel` (client decorations, the ICCCM
  title), override-redirect windows mint `create_surface`+`get_popup`
  anchored at their own screen rect (parent null, top-left anchor,
  down-right gravity, every constraint strategy — the solver's
  unconstrained answer *is* the X geometry; the constraints only
  catch the off-screen tail). Windows deeper in a subtree stay where
  the X protocol puts them — painted into their top-level's export
  by the **subtree composite** (`Server::window_frame`: the window's
  own store as the base, every mapped InputOutput descendant painted
  over bottom-to-top, foreign subtrees never occluding — the
  display-side compositor owns cross-window overlap), with the
  visibility arithmetic carried by the new `WindowTree::region_within`
  (the ancestor walk stopping at the export window — the subtree's
  own frame of reference). The X protocol's geometry stays the X
  clients' truth (ConfigureNotify answers from the X tree; the
  bridge never moves an X window — it is not a window manager); the
  LDP display's placement is the screen's truth (the shell's
  cascade; the popup solver's anchor). **Input translation bridges
  them honestly**: LDP pointer events are surface-local,
  surface-local is export-local, and export-local + the X window's
  own absolute origin = the X root coordinates the X protocol
  reports — a click hits the pixel the user sees; the X client sees
  the click at its own geometry. One pool, per-window buffers at
  packed offsets (grown high-water — a pool never shrinks), the
  per-window damage from `Server::take_top_damage` (the same union
  the rootful driver commits whole), and the root window itself is
  not exported (the XQuartz-rootless doctrine: no root-window
  content; the LDP desktop's own background shows). 9 unit tests
  including the OR-window popup mint, the per-window damage
  isolation, the input coordinate translation, and the
  host-seam pool lifecycle.
* **The rootless session proof** (`binaries/lion-bridge/tests/
  bridge_x11_rootless_session.rs`) — the exit criterion, end to end
  through a real Unix socket: three X windows at three fictional X
  geometries ride three LDP surfaces placed by the cascade (window
  one at step 0, window two at step 1) and the popup anchor (the OR
  menu at its own screen rect), the scanout pixel-exact under all
  three doctrines at once; the compositor's pointer motion over
  window two's *display* position arrives at the X client as a
  `MotionNotify` with the **X geometry's** root coordinates (the
  coordinate-fiction proof: root (202, 103) = X origin (200, 100) +
  surface-local (2, 3)); a draw into one window commits that
  window's surface alone (the damage isolation); and an unmap tears
  the export down (role, buffer, surface — the generic
  `connection.destroy` path) while its neighbors stand. The rootful
  path keeps its own regression suite (`--x11-rootful`, the
  v0.10.0 session test unchanged).
* **`--x11-rootful`** — the operator's escape to the whole-screen
  window (the v0.10.0 doctrine). **The rootless mode is the new
  default** for every X11 face: an X application's windows are
  windows now.

### Fixed — the seal run's own bug hunt
* **The bridge's pointer `enter` coordinates** (a latent v0.10.0
  bug): `pointer_xy` lifted the first two arguments uniformly, but
  `enter` carries (surface, x, y) and `motion` carries (x, y) —
  every enter arrived at (0, 0), the surface object read as the x.
  The rootless session's coordinate proof caught it; the lift now
  matches both shapes.
* **The X face's spontaneous output parked until the foreign
  client's next request**: the face's client pump returned on
  EAGAIN *before* flushing the server's per-client output — input
  events the driver injected (the whole point of the bridge) sat in
  the queue until the X client happened to send something. The
  flush now runs on **every** turn, read or not.
* **The rootless pool's growth re-zeroed the shared mirror**: a
  window joining (or any geometry change) re-laid the pool and
  re-zeroed the mirror, but only the freshly minted windows
  re-committed — the standing windows' pixels went black on the
  next sync while the newest window rendered fine. The mirror is
  grow-only now (the pool was already high-water); the session
  suite caught it as a one-in-two flake and it is ten-for-ten after
  the fix.

### Changed
* **The quiescent keepalive** (`LdpLink::sync`) — the compositor
  parks cross-client events (input among them) for the owner's next
  message, and a bridge that only *reacts* never polls them out. One
  `connection.sync` per idle turn — the protocol's own keepalive,
  "never advances protocol state" — keeps the parked events flowing;
  a few 32-byte frames a second on an otherwise idle link.
* `Phase` (the driver lifecycle enum) is shared by the rootful and
  rootless drivers; the `RecordingHost`/`TokenCheck`/`DriverHost`
  seams serve both shapes (the process binary hosts either with one
  wiring).
* The bridge's `LdpLink` watches are idempotent (the rootless id
  plan is dynamic — the face re-syncs its routing tables every
  turn) and carry the popup vocabulary (`PopupConfigure`/`PopupDone`
  routed by watched popup ids).

### Exit criteria
* The rootless session (`bridge_x11_rootless_session.rs`): three X
  windows — two managed at fictional X geometries, one
  override-redirect menu — all three pixel-exact on the compositor's
  scanout through a real Unix socket, the menu anchored at its own
  screen rect by the popup solver, the pointer motion over window
  two's display position arriving at the X client with the X
  geometry's root coordinates, the per-window damage isolation, and
  the unmap teardown with its neighbors standing.
* The native popup session (`popup_session.rs`): mint, solve at
  attach, configure/ack/commit landing placed, reposition with the
  live move, dismiss answered, the parent-death sweep.
* The rootful regression (`bridge_x11_session.rs`, `--x11-rootful`):
  the v0.10.0 exit criterion unchanged and green.
* `verify.sh` green in-tree and from a fresh extraction of the zip:
  fmt, clippy `-D warnings`, 2,001 tests / 0 failed, rustdoc
  `-D warnings`, spec lint, dep policy, ldpc drift, the API-freeze
  gate, release coherence.

## [0.10.2] — 2026-09-30 — The sleeping panel

Phase 35: the machine that sleeps when nothing changes. The most
expensive thing a display server does is *nothing visible* — the
display engine scans an unchanged framebuffer out of memory tens of
thousands of times per second, forever. This release closes that
account: **panel self-refresh** (the eDP feature the giants ride —
the panel holds its own GRAM copy while the content is static, the
pixel clock quiets, the memory bus idles), the **GPU clock
governor** (the DVFS seam over the landed-flip load), the **energy
ledger** (power as accounting, not assertion — a documented
parametric cost model and a CI-reproducible static-scene ratio), and
a genuine production bug fix the phase's audit surfaced: the idle
ladder only ticked under `drm_ready`, but a *truly* idle machine has
no DRM events — the ladder that exists to detect inactivity never
advanced when the machine was actually inactive. It ticks on every
serve-loop turn now (1,986 tests, up from 1,944).

### Added — the static-frame power path
* **`ldp-power::psr` — the panel self-refresh machine** — the pure
  decision layer, one per output: entry *earned* through consecutive
  quiet flip opportunities (hysteresis 2 by default — a scene that
  bursts one empty frame between animations never pays the exit cost
  to re-enter), exits *named* — `Damage` (a flip was submitted: the
  kernel's own implicit rescan), `Blank` (the ladder's Off rung),
  `Backlight` (the Dim rung's ramp — a real panel retrains), and
  `Unsupported` (the device refused the engage: the machine retires
  for the session — a power hint degrades, never retries, never
  dies). Input is deliberately *not* an exit: touching the machine
  with no visual consequence leaves nothing to rescan. The machine
  counts engagements, per-cause exits, and cumulative self-refresh
  nanoseconds — the ledger's raw material (11 tests).
* **`ldp-power::governor` — the GPU clock ladder** — asymmetric
  hysteresis over the honest load signal (landed flips against the
  pacing grid's capacity per decision interval): **up is instant**
  (one interval above 2/3 load steps the rung — under-clocking a
  heavy frame is visible, and the compositor never spends frame
  pacing), **down is patient** (three consecutive intervals below
  1/3 before stepping down — a tooltip animating over a still
  desktop must not thrash the clock). Four abstract P-rungs
  (`P0..P3`), the residency histogram, and the no-clock doctrine
  (the host passes durations) — pure arithmetic, CI-proven,
  including the hold zone (a half-rate animation holds its rung
  exactly) and the burst-interrupts-streak case (10 tests).
* **`ldp-power::ledger` — the energy accounting model** — the
  parametric cost table (render by P-rung 420–1850 mW, static
  scanout 350 mW, self-refresh 45 mW, dimmed 190 mW, blanked 8 mW;
  transitions and wakes priced — every figure a *labeled model* of
  public eDP/SoC ballparks, overridable by a host with better
  numbers) and the accumulated account: time-in-state, exits, wakes,
  total millijoules, and the headline **static-scene ratio** — this
  machine's static energy over the always-scanning counterfactual,
  the number the comparison table's power row carries. A session
  that never sleeps returns 1.0; a sleeping one returns the floor
  plus its honest hysteresis prefix (9 tests: the floor ratio beats
  4:1 and never claims zero, the rungs price separately, the model
  is overridable, saturating arithmetic never invents energy).
* **The KMS vocabulary** (`ldp-display`) — `AtomicRequest::
  connector_psr` (the `panel self refresh` property write — the
  mock's connectors all carry it; a real panel reports it in the
  connector walk, a sink that does not simply never engages), and
  the mock's **faithful PSR semantics**: an engaged CRTC's timeline
  *freezes* (`advance_to` skips it — ten seconds of wall time cross
  with zero vblanks, zero flips; `next_event_at` reports nothing
  due), and either an explicit property release or a submitted page
  flip (the kernel's implicit rescan) re-anchors the grid at the
  commit's own time — so the next flip lands exactly one full
  nominal period later. **The exit is never free**, whichever
  vocabulary releases it — proven at the device level over the
  preset's eDP pipeline (`tests/psr_timeline.rs`, 6 proofs: the
  round-trip, the free sleep, the explicit release's rescan, the
  implicit flip release's rescan, the untouched-grid control, and
  the once-only cost).
* **The world's power cadence** — `World::psr_tick`, the serve
  loop's every-turn arm (≤ 250 ms of poll time; the tests drive it
  at their own cadence): the ledger's interval closes in the folded
  state (blanked > dimmed > all-asleep > render-at-rung > scanout),
  the governor's window feeds the ladder, and every output's machine
  steps — a quiet output earns its hysteresis, a busy one's count
  resets (quietness is *consecutive*), an engaged busy one exits
  (its damage's flip is coming, and the device will release at the
  flip's submission regardless of what we write). Engagements apply
  as immediate connector property commits; a rejection retires the
  machine for the session. `render_slot` disturbs at every flip
  submission (the canonical Damage exit — the mock clears the
  property and re-anchors, and the world's `psr_live` mirror
  follows); the ladder's Dim rung disturbs by name with the explicit
  release write (no flip carries a backlight transition); the blank
  carries the release in the DPMS commit itself.
* **The reports** — `lion-compositor --selftest` prints the power
  line (`power: psr on (the panel sleeps when static)`); teardown
  prints the ledger's verdict: `power: ledger: <mJ>, static-ratio
  <r>, self-refresh <s> of <S>, exits <n> (<k> rescan), wakes <w>`
  — the operator reads the machine's own account at shutdown.
* **`--no-psr`** (CLI + `CompositorConfig::psr`, default **on**) —
  the operator's escape for panels whose self-refresh flickers (the
  real-world quirk the eDP ecosystem actually ships); the
  counterfactual the ratio is measured against, still scanning the
  static framebuffer on the untouched grid.

### Fixed
* **The idle ladder never advanced on a truly idle real machine** —
  the Phase 31 wiring ticked `tick_idle` only when the DRM fd was
  readable, but a quiescent machine has no DRM events (no flips, no
  hotplug): the ladder that exists to detect inactivity never dimmed
  or blanked when the machine was actually inactive. A real-machine
  bug the mock suites could not see (their tests tick the ladder
  directly). The ladder now ticks on every serve-loop turn — the
  poll timeout is the pace-maker; a DRM event remains the immediate
  wake.
* `power_input_session::the_ladder_dims_blanks_and_relights` — the
  bind-cascade drain (one roundtrip before the ladder starts; a
  straggler handshake message landing mid-ladder marked activity and
  re-dimmed the machine under parallel load — 20 consecutive suite
  runs clean).

### Changed
* `ldp-display`: the mock's connector catalogs already carried the
  `panel self refresh` property (registered since the HDR walk) —
  the phase wires its *semantics*; `CrtcTimeline::reanchor(now)` (the
  rescan's grid restart) joins the crate-internal timeline API.
* `lion-compositor`: `CompositorConfig` grows `psr` (default `true`
  — the doctrine; `--no-psr` is the escape); `OutputSlot` grows the
  per-output `psr` machine, the bring-up walk's `psr_capable`
  (name-resolved through the backend's own property tables — mock
  and libdrm agree on what "not found" means), and the `psr_live`
  device-side mirror; `World` grows the governor, the ledger, and
  the power-cadence clocks; `render_slot` opens with the PSR
  disturbance; the teardown's session summary adds the power line.

### Exit criteria
`binaries/lion-compositor/tests/power_psr_session.rs` (7 proofs)
and `crates/ldp-display/tests/psr_timeline.rs` (6): static content
puts the panel to sleep and the device goes *silent* (ten seconds of
mock time, zero vblanks, zero flips, the property reading back on);
the ledger prices the sleep (ten seconds minus the prefix, the
static ratio under 0.25 and honestly above 0.10, 42 poll turns = 42
priced wakes); damage wakes the panel at exactly one refresh
interval (the implicit release, the exit named `Damage`, the rescan
paid once); the governor climbs under a full-rate burst and decays
to the floor in stillness while the panel sleeps; the Dim rung
retrains the panel by name (the explicit release — no flip carries
it) and the hysteresis re-earns the sleep while dimmed; the blank
vacates the sleep by the blank's own name (the DPMS commit carrying
  the release) and the wake restarts from Scanout — re-earned,
never inherited; and `--no-psr` keeps scanning the static frame on
the live grid (the counterfactual, zero self-refresh nanoseconds).

## [0.10.1] — 2026-09-30 — The hardware compositing path

Phases 33 and 34: the GPU stops being the only blender in the machine.
Phase 33 (the evdev input path — landed in-tree immediately after the
v0.10.0 seal and changelogged here) wires the real `/dev/input`
devices through the seat router onto the wire. Phase 34 is the
GPU-compositing field's answer to the giants' MPO: the
**plane-assignment engine** (`ldp-planes`, the 26th crate) maps the
layer stack onto the display hardware's own blend order — the
**zero-composite frame** (a fullscreen opaque client's own framebuffer
on the panel, the renderer emitting *no pass at all*), the underlay
split (canvas beneath, overlays above), the import walk that turns
client buffers into kernel framebuffers, YUV and scaled GL uploads,
and the dma-buf feedback tranches — every piece CI-proven over the
mock KMS device's real plane inventory, every composite layer carrying
a *named* demotion reason (1,944 tests, up from 1,909).

### Added — Phase 34: the plane engine and the hardware arm
* **`ldp-planes` — the plane-assignment engine** — the pure decision
  layer over the KMS plane model: `PlaneInventory` (the per-CRTC
  capability walk — kinds, `IN_FORMATS` pairs, zpos slots — read
  through the object-property table the atomic requests write into),
  `LayerFacts`/`OutputFacts` (what the compositor knows per layer:
  placement, buffer geometry, color, opaque coverage, the import
  walk's framebuffer, the Liquid styling's demotion facts),
  `Assigner::solve` (the split-point doctrine — the composite set is
  a prefix, the offloaded set a suffix, because a composited layer
  blends into the canvas on the primary and nothing can render
  between hardware planes; the zpos-monotone depth-first search over
  per-layer capability sets with backtracking; the overlay budget
  fit; the two zero-composite shapes — on the primary, or on the
  lowest capable overlay when the primary refuses the format — and
  the repair that shrinks the suffix when the search fails). The
  demotion ledger names every reason a layer composites: `NoFb`,
  `Scaled`, `Transformed`, `ColorMismatch`, `OutOfBounds`,
  `Translucent`, `NeedsBackdrop` (frost's truth is the blend),
  `Styled` (corners and shadows exist only in the render path — a
  plane would scan a rounded window square), `FormatUnsupported`,
  `NoSlot`, `BelowSplit`, `BottomNotCovering` (22 solver tests:
  the inventory walk, the zero-composite proofs, the split doctrine,
  the budget push-up, the zpos-monotonicity inversion case, the
  backtracking swap, the styled/frost demotions, determinism, the
  reserved cursor).
* **The import walk** (`KmsBackend::import_gem`, mock +
  dlopen-real) — `drmPrimeFDToHandle` behind the audited FFI seam;
  the mock mirrors the kernel's object-identity semantics (same fd →
  same handle) so the whole orchestration is CI-provable with real
  client buffers. `FbSpec::planar`/`from_layout` build the
  `AddFB2WithModifiers` requests from a buffer's validated geometry.
  The walk runs once per buffer object; refusals cache as honestly
  as successes (a memfd on real hardware composites — the report
  line says so, never a silent fallback).
* **The frame loop's hardware arm** — every frame grades the stack
  (the import walk + the facts), solves, and takes one of three
  shapes: **zero-composite** (no renderer pass, no canvas write, no
  readout — the client's buffer on the primary; the display model
  still composes so the pixel oracles keep their meaning), **split**
  (the composite prefix renders into the canvas on the primary, the
  offloaded suffix rides overlays above — the Windows-MPO underlay
  shape), or **full composite** (the v0.10.0 bytes verbatim). The
  multi-plane atomic commit carries the whole plane state:
  assignments on with their stacking zpos, **retired overlays off**
  (a stale overlay hovering above the canvas is the bug that rule
  forbids — the mock's modeset detector was corrected to the
  kernel's real rule that a plane disable is a page-flip-class
  commit, not a modeset). Release gates, presentation feedback, and
  the scheduler's pacing grid ride the same flip they always did.
  The stale-canvas rule: a frame returning from the all-planes shape
  takes full damage (the persistent canvas is stale by exactly one
  zero-composite frame). `--dump` pins the frame to the composite
  arm (the diagnostic doctrine: screenshots observe the composed
  truth).
* **The display model** — `World::scanout_words()` now serves *what
  the panel shows*: the canvas with every plane-carried layer
  blended in zpos order through the renderer's own canonical sampler
  and blend kernel (with a word-copy fast path for opaque alpha-free
  layers — a fullscreen compose in milliseconds, not seconds, in
  debug builds). The existing pixel oracles — session suites,
  capture, the equivalence corpora — keep their meaning across all
  three frame shapes unchanged.
* **YUV and scaled GL uploads** (`ldp-renderer`) — the GL path
  composites every format the software path composites: the 32-bit
  RGB family keeps the fast word-swizzle arm; YUV-family / RGB888 /
  RGB565 decode through the software path's own sampler (the
  identical fetch + primaries-derived matrix + range expansion +
  quantize — the upload is byte-equal to what the CPU renderer would
  blend, by construction, pinned by the extended equivalence
  corpus). Scaled placements upload the *source* pixels and scale in
  the draw: the GPU's NEAREST sampler and the reference evaluator's
  continuous-nearest rule (pixel centers through the inverse scale,
  floor) are the same math the software path's `Mapping::Scaled`
  runs. The boundary tests that asserted the old refusals now pin
  the new byte-equality.
* **dma-buf feedback tranches** (`ldp-gpu::feedback`) — the
  allocation-advice policy: tranche 0 the (format, modifier) pairs
  both the GL device imports *and* a plane scans out (zero-copy end
  to end, compressible modifiers preferred over linear), tranche 1
  the GL-only fallback; empty-intersection honesty; deterministic
  resolution. The protocol surface that carries tranches to clients
  is the named roadmap line — the policy it will serve is shipped.
* **The startup and shutdown reports** — `planes: output 0 (CRTC
  42): 2 overlays, cursor reserved` at bring-up (the inventory walk,
  printed with the renderer/effects/shell/input lines), the session
  summary at teardown (all-planes frames, imported buffers), and the
  per-frame plan report line (the demotion ledger's first phrase)
  for the operator's one glance.
* **Exit criteria** (`binaries/lion-compositor/tests/planes_session.rs`,
  6 tests): the zero-composite session (the client's registered
  framebuffer *is* the primary's scanout — asserted on the device
  state — with the visible truth the client's own pixels and zero
  render passes); the underlay split (the window on overlay 51 at
  zpos 1 with its exact src/dst, the canvas on the primary, the
  display model blending the overlay over the canvas); the styled
  demotion (a High-tier window never rides — its pixels live in the
  render path); the overlay retirement (the window's destruction
  turns the overlay *off* — no stale scanout); the release gates
  under direct scanout (a superseded buffer frees after the
  replacing flip lands, zero render passes the whole way); the
  inventory report line.

### Added — Phase 33: the evdev input path (landed post-v0.10.0, changelogged here)
* **The serving half of the input stack** — the compositor opens the
  machine's real `/dev/input` devices, decodes and normalizes their
  frames through `ldp-input`'s evdev codec, builds the routing view
  of the scene, feeds `ldp-seat`'s router (focus, grabs, pointer
  acceleration, the xkb keymap), and delivers routed protocol events
  through the outbox vehicle the presentation and data families use.
  Click-to-focus keyboard policy (the shell's routing doctrine: the
  scene supplies focus, the router never guesses); roots are the v1
  routing targets. The wake contract is honest and documented: routed
  events park in per-client outboxes and stream at each client's
  next wake point — a push-side session wake (an eventfd per
  session) is the self-scheduled-render-thread roadmap line.

### Changed
* `ldp-display`: `PlaneInfo` grows `PartialEq` (the inventory's
  identity); the mock's modeset detector treats a plane *disable*
  (fb → 0) as a normal page-flip commit — the kernel's rule, which
  the multi-plane retire path exposed; `import_gem` joins the
  `KmsBackend` trait (mock + the dlopen'd real driver with
  `drmPrimeFDToHandle`).
* `ldp-renderer`: `sample_premul_u8`, `pack_canonical`, and
  `unpack_canonical` join the public API (the display model and the
  GL decode arm share the canonical sampling/blend vocabulary).
* The GL upload record carries the texture's own dimensions
  (buffer-resolved payloads; a scaled placement scales in the draw).

## [0.10.0] — 2026-09-29 — The served stack

Phases 31 and 32: the compositor stops being a renderer with a
protocol attached and becomes a *served stack* — several displays at
once, the fractional-scale truth, the HDR pipeline, adaptive sync,
the idle ladder, the frozen v1 API contract, and the clipboard the
compatibility bridges need — the bridges themselves landing as the
in-tree `lion-bridge` door, with the zero-copy EGLImage arm joining
the GL renderer, all end-to-end through the real wire (1,909
tests). Everything below is `scripts/verify.sh`-green.

### Added — Phase 31: the multi-output doctrine and the tier stack
* **The multi-output doctrine (`--outputs multi|single`)** — the
  serve loop of Phase 25/26 served one pipeline; the multi doctrine
  serves *every* pipeline the allocator finds: each connected
  display on its own CRTC and plane, enabled and scanning out, laid
  out left-to-right into one logical desktop. A client binds
  `ldp.core.output` once per display it cares about — each bind
  mirrors the next output (round-robin, its own geometry and VRR
  truth). Per-output visibility: a window on the primary enters only
  the primary; a window spanning the seam enters both, each output's
  scanout carrying its own portion. Topology motion mid-session: a
  non-primary leaving is `OutputRemoved` (the survivor re-flows),
  the primary leaving promotes the survivor, a display joining
  extends the desktop, mixed sizes lay out at their own geometries.
  Release gates: a buffer spanning two outputs releases after *both*
  flip past it; an output leaving satisfies its own gate. The
  library default stays single-output, byte-identical (15 exit-
  criterion tests, `binaries/lion-compositor/tests/multi_output.rs`).
* **The fractional scale doctrine (`--scale F`)** — the wire
  vocabulary (`output.scale` Q8.8, `surface.set_buffer_scale`,
  `preferred_scale`) becomes true: the operator's factor advertises
  on every output, the positioning shell resolves its layout on the
  *logical* canvas (a 1080p panel at 2x serves a 960x540
  phone-density desktop), placement lands in physical pixels, and
  clients learn the rendering truth at bind and at `enter_output`
  (`tests/scale_session.rs`).
* **The HDR pipeline (`--hdr`)** — the panel's truth advertises over
  `output.hdr_caps` (PQ, wide gamut, the peak), the render pass
  folds the desktop's stack into the mode controller's hysteresis
  (an HDR surface takes the composite onto the PQ canvas after the
  dwell — a lone popup cannot flap the panel), and the renderer's
  cross-description pipeline blends every layer scene-linear with PQ
  encoding on the tail (SDR content rides at the BT.2408 reference
  white) (`tests/hdr_session.rs`).
* **Adaptive sync (`--vrr`)** — every VRR-capable output's page-flip
  commits arm `VRR_ENABLED`; the ldp-vrr deadline policy widens the
  scheduler's commit window to the CRTC's advertised range (the mock
  timeline clamps exactly like the kernel) (`tests/vrr_session.rs`).
* **The idle ladder (`--idle MS`)** — the ldp-power machine ticks at
  the device clock: Dimmed at the timeout, Off at twice it (DPMS
  blanking, the scheduler parked, pending frame requests dying
  `output_off`), activity — every client message — lights it back
  (`tests/power_input_session.rs`).
* **The API stability freeze gate** — `ldpc snapshot --check`: the
  frozen v1 surface must match the compiled spec byte-for-byte; a
  moved name, opcode, argument, or version breaks the build until
  the change is consciously re-frozen (`scripts/verify.sh`).
* **The zero-copy EGLImage arm (`ldp-renderer`)** — a layer imported
  as an EGLImage never touches a CPU payload: the upload path is
  placement-only validation and the command stream binds the image
  as the texture directly (`create_texture_from_egl_image` — the
  DMA-BUF import doctrine's first land, the audited FFI surface
  grown to carry it). The command-stream goldens join the oracle
  family (`tests/gles_stream.rs`): exactly which passes the GL
  renderer emits for a known frame — the damage-rect passes, the
  layers a pass skips (a layer that misses the damage rect must not
  draw), the per-pass scissor, the readback, and the pinned shader
  sources — because the real GPU executes this exact stream, a
  regression here is a rendering regression even when pixels happen
  to match.

### Added — Phase 32: the data family over the wire
* **The clipboard service (`ldp.data.data_device_manager` served)**
  — the Phase 13 manager becomes a served protocol family: devices
  mint per seat (`get_data_device`), sources offer MIME types
  (`create_data_source` + `offer`), and `set_selection` /
  `set_primary_selection` install a source on the seat's slot. The
  publication routes to every *other* device client on the seat: the
  `data_offer` announcement arrives as a **real object the receiver
  can send requests on** (the server-chosen id crosses the outbox as
  a create-first instruction — the store's new
  `insert_server_at`/`create_announced` path), followed by the MIME
  enumeration and the `selection` event. The receiver narrows
  (`accept` → the source sees `target`), then `receive`s through a
  real pipe: the write end crosses the request, the permission gate
  runs (the same-user baseline carries `ClipboardRead` until the
  broker deployment narrows it), and the source's `send` event rides
  the same descriptor back — the payload landing byte-exact in the
  receiver's read end. Teardown honesty: the owner destroying its
  source announces `selection(null)` to the receivers; a session
  dying mid-selection hands the clipboard back the same way
  (`binaries/lion-compositor/tests/data_session.rs` — the full
  choreography, the byte-exact transfer, the freshness doctrine, and
  the teardown truth).
* **The seat's interaction serial clock** — `set_selection`'s serial
  must equal the seat's *current* serial (the freshness doctrine the
  manager has always enforced). The client learns serials from the
  serial-bearing deliveries the seat issues — today that is the
  shell's `configure`, now drawn from one monotonic clock
  (`World::next_seat_serial`); the device-event path (the evdev
  roadmap line) will draw from the same clock. A stale serial is the
  fatal `invalid_state` the wire's only-rejection channel carries.
* **The five device mints** — the seat's `get_*` requests now mint
  all five device objects (pointer, keyboard, touch, tablet,
  gestures) at their true interfaces (`ldp.input.pointer` …).
* **`clip-client --live`** now probes the real global
  (`ldp.data.data_device_manager`) and reports the served family.
* **The compatibility door (`lion-bridge`)** — Wayland and X11
  applications run on lion-display through one bridge process: a
  Wayland *display* (`--wayland PATH`, repeatable) whose clients
  become LDP windows one-for-one, and an X11 *display* (`--x11
  PATH`, repeatable) whose whole screen becomes one rootful LDP
  window (`--screen WxH`, default 1024x768). The pure protocol
  machinery stays in the `ldp-wayland-bridge`/`ldp-x11-bridge`
  crates; the process owns what those crates refused to — the
  sockets, the pool and keymap descriptors, the MIT-SHM SysV
  mappings, the event clock, and the single-threaded poll engine
  (one poll turn over every descriptor — each foreign socket, each
  LDP link, the listeners — nothing blocks; a quarter-second cap
  keeps shutdown and the idle ladder honest). The security doctrine
  carries: every foreign session rides its own LDP client identity,
  and the process refuses to run without a bridge-scope token
  (`--token FILE`, the Phase 16 vocabulary — the compositor's
  broker-era enforcement is the named roadmap line, the bridge's
  gate runs today). The exit criterion is the honest end-to-end: a
  Wayland client's pixels *and* an X11 client's pixels reach the
  compositor's scanout
  (`binaries/lion-bridge/tests/bridge_{wayland,x11}_session.rs`),
  the faceless invocation refused, the link and X11 bootstraps
  probed (`tests/{link,x11}_probe.rs`). The binary ships in the
  `lion-compositor` deb package — the bridge is a served feature,
  not an out-of-tree extra.

### Fixed
* **The shell's initial `configure` was a wire violation** — Phase
  31's minimal service emitted four of the ten arguments the schema
  declares (`signature_mismatch` the moment a real client validated
  it). The proposal now carries the full serial / states / size /
  insets / workspace / output set. Found by the Phase 32 end-to-end
  suite — the first client ever to bind the shell and read the
  configure.
* **The rootful X bridge died on its first commit** — the shell's
  initial configure carries size 0x0 ("the client picks its own
  size", the xdg doctrine), and the X driver treated it as a
  literal root resize: the X screen collapsed to nothing, the root
  store emptied, and the frame tick's pool resize tried to *shrink*
  — `shm pools may only grow`, a fatal protocol violation that
  killed the session before a single X pixel could reach the
  scanout. A 0x0 proposal is now the no-op it always was (the
  rootful bridge's size is the operator's `--screen`); a real
  proposal still resizes. Found by the bridge session suite (the
  end-to-end EC was never green until now).
* **Two X11 wire-order bugs the bridge session suite caught** —
  `MIT-SHM Attach` parsed `shmseg, read-only, pad, shmid` (the
  SysV id at the tail) where the real protocol carries `shmseg,
  shmid, read-only, pad`; and core `PutImage` parsed `drawable, gc,
  width, height, …` where the real `xPutImageReq` carries `width,
  height, dst-x, dst-y, drawable, gc, left-pad, depth`. Both now
  match the real wire (a real X11 client's bytes parse correctly);
  the pure crate's test helpers encode the real order with the
  XID and the shmid distinct, so a parser that mixes the fields
  cannot pass; the drivers' other drawing opcodes were audited
  against `xproto.h` field-for-field (they match).
* The `lion-bridge` X11 face's debug drain carried a dead loop
  (removed); the bridge tests' lint findings (byte-grouping,
  same-type cast, `contains`) cleaned.

### Changed
* `lion-compositor --outputs <doctrine>|--scale <F>|--hdr|--vrr|
  --idle <MS>` (CLI + `CompositorConfig`) — the tier stack of
  Phase 31, each independently opt-in, each defaulting to the
  byte-identical prior behavior.
* The outbox gained a create-first instruction (`OutboxEntry::
  creating`) — the delivery vehicle for server-chosen ids crossing
  sessions (the data family's announcements; the output migrations
  keep their revocation path unchanged).

## [0.9.0] — 2026-09-29 — The desktop matrix

Phase 30: every display size, every graphics card, and the machines
with none. The compositor now *meets the machine it boots on*: the GL
probe reads the context's identity and classifies the GPU, the
operator can force any offered mode by size, the desktop matrix is
benchmarked end to end, and the honest world comparison is a
committed document (1,839 tests). Everything below is
`scripts/verify.sh`-green.

### Added
* **GPU-class capability probing (`ldp-gpu`)** — the context's
  identity joins the probe: `glGetString` (vendor / renderer /
  version — core GLES 2.0, the new audited FFI entry) read once at
  bring-up into `GlIdentity`, and `GpuClass` — Discrete, Integrated,
  Virtual, SoftwareRasterizer, Unknown — derived from the renderer
  string by pure table-driven logic (`classify`), exhaustively
  unit-pinned: the software markers (llvmpipe, swrast, softpipe,
  SwiftShader, OSMesa…), the virtual devices (virgl, SVGA3D, QXL…),
  the discrete and integrated tables, and the honest fallback — an
  unrecognized GPU is `Unknown` and *counts as hardware* (never a
  silent degrade). The selection policy (`lion-compositor::renderer`)
  composes the pinned Phase 24 matrix with the one new rule: **Auto
  refuses a CPU-rasterizer GL context** — when the "hardware" is
  llvmpipe, the specialized software backend (word blending, no GL
  state machine, no shader compiler) is the faster CPU path, and the
  reason rides the report line: `renderer: software (GL is a CPU
  rasterizer: …; the specialized software backend is faster)`.
  `--renderer gl` still forces GL over any context; the startup line
  names the device: `renderer: gles (hardware: EGL + GLES 2.0
  context up, discrete GPU: AMD Radeon RX 7900 XTX)`.
* **The sized output doctrine (`--resolution WxH`)** — the bring-up
  and *every hotplug migration* prefer a mode of exactly the forced
  size: `ldp-display::serve::select_pipeline_sized` (exact-area
  filter, preferred flag then highest refresh among matches — the
  unsized path stays the pinned doctrine verbatim); a size nothing
  offers is the typed failure naming every size the panel does offer
  (`no 2560x1440 mode on any connected connector (offered: 3840x2160,
  1920x1080)`) — never a silent nearest-neighbor guess;
  `OutputGlobal::select_size` re-points the protocol face (the
  `current_mode` flag, the geometry, the renderer's output
  description) at the forced mode; a migration to a display without
  the size falls back to the unsized doctrine (the world remembers
  the operator's preference for the next display that offers it).
  `Mode::panel_4k60` joins the fixtures (594 MHz over 4400×2250 —
  exactly 60.000 Hz).
* **The desktop matrix benchmark rows (`ldp-bench`)** — the Liquid
  desktop frame (wallpaper, four rounded-and-shadowed windows in the
  quadrants, one frosted dock bar, proportional geometry per size) at
  **1920×1080, 2560×1440, 3440×1440 (21:9 ultrawide), 3840×2160**,
  at the High tier and at the tier the no-GPU doctrine picks for each
  size, steady state plus (at the two ends) the cold first frame.
  Measured (2 vCPU, 5-run medians): High steady **7.31 / 11.49 /
  15.11 / 24.67 ms** across the four sizes — 60 Hz holds at the High
  tier through the 21:9 ultrawide; the no-GPU doctrine's tier
  **5.96 / 7.95 / 10.96 / 18.99 ms**. The first full run caught a
  real regression: 4K High measured **194 ms** — the shadow memo's
  fixed 6 Mi-word budget sat under a 4K desktop's working set
  (~6.5 Mi words), so the LRU evicted and rebuilt every material
  every frame. The budget now scales with the output (`begin_frame`
  pays twice its pixels, floored at the phone era's 6 Mi so Phase 29
  behavior is byte-exact): **194 → 24.67 ms (7.9×, same bytes)**,
  pinned by the new flat-rebuild-counter oracle.
  `docs/benchmarks.md` re-baselined with the full matrix.
* **`docs/comparison.md`** — the honest field-by-field comparison
  against Wayland, X11/Xorg, macOS WindowServer, and Windows DWM:
  twenty fields scored out of 100 with the reasoning attached, the
  measured spine (the desktop matrix, the Phase 29 phone frame, the
  **19 MiB peak-RSS** full bring-up), and every gap named with its
  roadmap line — the totals read one way: on the fields a display
  server's own engineering controls, this codebase leads; on the
  fields only time and adoption buy, the gap is priced without
  sentiment.

### Changed
* `lion-compositor --resolution <WxH>` (CLI + `CompositorConfig`):
  forces the output's mode; the DRM path reports
  `scanout: 1920x1080 XRGB8888 (forced via --resolution 1920x1080),
  pitch …`; the library default stays the plain preferred-mode
  doctrine (every prior pixel oracle untouched).
* The renderer bring-up report carries the GPU identity (the device
  name and class in the one line operators read).
* README: the Phase 30 bullet, the v0.9.0 status, the comparison
  document in the docs index. Roadmap: the Phase 30 section and the
  v0.9.0 milestone row. Maturity: v0.9.0, gates 1,839, row 30.
  User-guide §3.10: the desktop operator's section (the GPU-class
  ladder, `--resolution`, the no-GPU doctrine). Admin-guide and
  packaging README follow.

### Fixed
* **The 4K shadow-memo thrash** — the first full desktop-matrix run
  caught the 4K High steady state at **194 ms per frame**: the shadow
  `MaterialCache`'s fixed 6 Mi-word budget sat under a 4K desktop's
  working set (~6.5 Mi words for five materials), so the LRU evicted
  and rebuilt every material every frame. The budget now scales with
  the output — `begin_frame` pays twice its pixels, floored at the
  phone era's 6 Mi so every phone-sized oracle keeps its byte-exact
  Phase 29 behavior (the budget only ever grows). 4K High steady
  state: **194 → 24.67 ms (7.9×, same bytes)**, pinned by
  `four_k_desktop_steady_state_does_not_thrash_the_shadow_memo`
  (the flat `shadow_rebuilds()` counter across steady frames) and the
  `scaled_budget_keeps_a_large_working_set_resident` unit oracle.
* Otherwise nothing user-visible: Phase 29's kernels are untouched;
  the new policy composes *above* the pinned selection matrix, and
  the library defaults keep every Phase 26–29 oracle byte-exact.

## [0.8.0] — 2026-09-29 — The steady-state pass

Phase 29: the Liquid visual engine at 60 Hz on a weak CPU. The
phone-frame benchmark — a 1080×2340 output, a plain wallpaper, four
rounded-and-shadowed app cards, one frosted translucent panel, full
damage every frame — measured **178.6 ms per frame** at Phase 28
(5.6 fps); it now measures **8.70 ms steady state** (20.5×) and
**64.8 ms first frame** (2.75×), same machine, byte-identical
output (1,816 tests). Everything below is `scripts/verify.sh`-green with new
oracle suites pinning every equivalence.

### Added
* **The material memos (`ldp-renderer::effects`)** — the macOS rule
  that shadows are *imagery*: `MaterialCache` (a shadow built once
  per (params, destination, ink radius), linear-scan lookup, LRU
  under a 6 Mi-word budget, the honest retained-scratch recompute
  past it) and `FrostMemo` (the frost material rebuilt only when
  the backdrop's own words changed — a full comparison, no hashing,
  no clocks; the steady state skips the blur entirely). Both are
  pure memoization: the same inputs produce the same bytes, so
  `submit` stays a pure function of (framebuffer, damage, layers).
* The corner-arc band geometry: `corner_band_xs` / `CornerXBand` —
  pixel centers sit exactly half a pixel from every *straight*
  edge, so fractional coverage only ever comes from the arcs; the
  bands carry them (tight for sanitized radii, conservatively whole
  for degenerate capsule radii reachable through the public API).
  99.6% of a phone window's pixels never run the SDF's `sqrt`
  again; `merged()` walks the bands disjointly (the narrow-rect
  overlap the GL corpus caught as a double-fold).
* `StyleState` — the software renderer's retained styled-path state
  (the memos plus the frost working buffers): zero allocation per
  steady-state frame.
* The GL backend's memo wiring: the shadow's texture bytes convert
  once per unique shadow, the frost readback lands in one retained
  buffer (one allocation per output size, not one per frosted layer
  per frame), and both backends draw from the same material bytes.
* The phone-frame benchmark rows (steady state and first frame) in
  `ldp-bench` — the low-end device this project exists for, measured
  as a permanent suite shape.
* Ten oracle tests: the reciprocal-division proof (exhaustive over
  the sanitized domain), the Phase 27 blur/shadow/frost/fold kernels
  restated verbatim as references and cross-checked over randomized
  shapes (including the degenerate radii), the band-soundness sweep
  (every pixel outside the bands is exactly bulk), the merged-band
  single-visit property, both memos' hit/miss semantics, and the
  steady-state transparency oracle (a warm renderer's frames stay
  byte- and stat-identical to a cold render's).

### Changed
* The box blur kernel: edge-free interior loops (the
  `Option`/clamp plumbing only runs within `radius` of a border),
  a 16-column strip walk for the vertical pass (each row touch
  reads one contiguous slice — the old per-column walk paid a cache
  line per row per column), the window mean's division replaced by
  an exact 48-bit reciprocal multiply for the sanitized window
  domain (`win ≤ 257`, proven exhaustively; larger public radii keep
  the plain divide), and a retained-scratch entry point
  (`blur_passes`) for zero-allocation callers.
* The shadow material builds band-limited: the bulk interior is a
  constant fill, the ink hole zeroes the bulk span and scales only
  the arcs, and the blur runs in place over the material's own
  words.
* The frost material's fast paths (byte-equal, including the
  garbage-input clamp an invisible `over` performs): the veil's
  per-pixel products fold into constants, full saturation skips the
  luma lerp, and a transparent veil skips the blend.
* The frost snapshot and material application are word-level: the
  canonical word *is* the ARGB-family output word (the BGR family
  is one channel swap), material rows are served as slices, opaque
  material pixels word-write, and the holed ink's transparent runs
  are skipped by scan.
* The styled span dispatch is per-row: a row outside the corner
  bands takes the *plain* compositing path (the word copies
  included) while paying the styled statistics exactly as before;
  band rows run the coverage fold with the SDF only inside the
  bands.
* The copy paths broadened to same-family formats: an XRGB window
  over an ARGB output word-copies (the X byte forced), an ARGB
  window over XRGB copies per opaque chunk — byte-equal to the
  sampling paths by the field orders.
* The 1:1 direct-mapping blend loop is monomorphized per source
  format (no per-pixel mapping match or format dispatch — the
  translucent-interior hot path).

## [0.7.0] — 2026-09-29 — The positioning shell

### Added
* `ldp-shell::layout` — the placement engine, pure arithmetic: `Edge`,
  `DockConfig`, `DeviceClass` (portrait → phone, landscape → desktop),
  `Layout::resolve` (classify, dock, carve — thickness clamped against
  half the edge), the placement policies (Fill / Cascade / Center,
  the cascade caught at the usable edge, oversized windows anchored),
  `clamp_into`, and the honest report line.
* `SurfaceTree::set_position_now` — the immediate position apply for
  the shell's re-layout arm; the damage engine's R2 coverage rule
  reads live positions, so a shell-moved surface repaints correctly
  without a commit.
* `lion-compositor::shell` — the positioning shell: `Shell` (layout,
  placement counter, the migration re-place), `SystemDock` (the
  ARGB ink — a haze plus a row of app pills at a stable buffer
  identity — the resting rect, the intro rise), `DockMode` +
  `ShellConfig` (`--dock auto|off`, `--dock-thickness`).
* The compositor integration: placement at a root's first attach
  (riding the pending queue — the position applies with the mapping
  commit), the migration relayout (every mapped root re-places into
  the new usable area in the migration frame itself; a new screen
  replays the dock's intro), the dock as the topmost render layer
  (Phase 27's styled machinery — the frost snapshots the composed
  backdrop, the corner coverage folds at upload), the repaint rules
  (the vacate rule while rising; the frost's claim when damage
  intersects the dock's rect at styled tiers), and the honest
  direct-scanout subtraction under chrome.
* The startup's third line: `shell: …` (the resolved layout doctrine,
  the dock's reservation).
* 35 tests (1,769 → 1,804): the layout suite, the immediate-move
  damage rule, the shell module suite (the ink, the rise's monotone
  settle, the vacate rule), and `shell_session.rs` — the end-to-end
  exit criterion.

### Fixed
* The capsule painter's inverted body geometry (body *rows* instead
  of body *columns*) — a wide pill degenerated into two end-circles;
  the dock's pills now render as true capsules.

### Changed
* The CLI defaults to `--dock auto` (the operator gets the phone
  shell); the **library default stays off** — placement keeps
  creation positions and no chrome draws, so every Phase 26/27
  pixel oracle stays byte-exact (the same contract as the effects
  tier's Minimal default).

## [0.6.0] — 2026-09-29 — The Liquid visual engine

Phase 27: the macOS-class material language for the Linux phone —
rounded corners, soft shadows, frosted glass, and the spring motion
that ties it together, all integer-deterministic, all byte-equal
across the software and GL backends, all tiered for low-end hardware.
Everything below is `scripts/verify.sh`-green (test count in
`docs/maturity.md`).

### Added
- **`ldp-renderer::style` — the Liquid vocabulary and the low-end
  doctrine.** `LayerStyle` (corner radius, shadow, backdrop frost)
  rides `SurfaceLayer` with a plain default — a Phase 26 layer
  renders byte-identically (the whole prior corpus pins it).
  `EffectTier` (Minimal / Low / Medium / High) maps the machine to
  the language: GL serves the full 3-pass blur; software on a
  phone-sized output serves 2-pass; software on a desktop-sized
  output keeps the identity with a tint veil; the headless CI path
  stays plain unless asked. `EffectChoice::parse/resolve` is the
  `--effects` surface.
- **`ldp-renderer::effects` — the pure material kernels.**
  `rounded_coverage` (the canonical rounded-rect SDF — IEEE-exact
  ops only, bit-stable), `blur_words` (a separable sliding-window box
  blur over premultiplied words, full-window normalization, clamp
  edge for frost / transparent edge for shadow spread),
  `shadow_material` (the blurred silhouette, **holed under the ink**
  so a translucent pane never sees its own shadow), `frost_material`
  (blur + BT.601 desaturation + tint veil), and `over_premul` — the
  one shared integer `over` the composite path, the GL reference
  evaluator, and the material draws all execute (factored so the
  three call sites cannot drift).
- **The styled software path.** Per styled layer, the macOS order:
  snapshot the backdrop, replace it with the **rounded** frost
  material, draw the shadow (fringe lands in the corner cutouts),
  then composite the ink with the corner coverage folded into the
  source alpha (`scale_premul` then the shared over — the GL upload
  fold's exact sequence, the byte-equality anchor). All
  damage-clipped through the frame's row index; `RenderStats` gains
  `pixels_effect`.
- **The Liquid GL stream — byte-equal, no seam growth.** Styled
  frames switch the GL submit to a **layer-major** order (every
  layer visits every damage rect in its own passes) so the frost
  readback snapshots exactly the backdrop state the software path
  sees; the corner coverage folds at texture upload; the shadow and
  frost materials ride as their own texture draws through the
  existing `draw_layer` vocabulary. The unstyled stream is untouched
  (the Phase 24 goldens pin it).
- **`ldp-compositor::spring` — the motion vocabulary.** A
  critically-damped spring with fixed 4 ms substeps (semi-implicit
  Euler): bit-reproducible from the frame-timestamp sequence,
  monotone under critical damping, bounded under one-second steps.
  The showcase's glass panel slides its pills in on it — the client
  drives the animation through the presentation loop, exactly the
  iOS shape.
- **`--effects <auto|high|medium|low|minimal>`** on lion-compositor
  (default `auto`), the resolved tier in `World.effects`, the honest
  second startup line (`effects: high (3-pass blur frost, panel
  shadows, corners)`), and the per-surface policy in the frame loop:
  opaque windows get corners + shadow, translucent surfaces get the
  frost too, the fullscreen wallpaper goes undressed — **the system
  owns the materials, the macOS doctrine** (nothing in the protocol
  carries them). Styled damage expands to the effect rects so a
  freshly-mapped window's shadow paints in the same frame.
- **The showcase's glass panel**: a full-width translucent ARGB bar
  over the sunset — the compositor frosts it, the spring settles its
  app pills by the final frame (`setup_surface_format` joins the
  tools for the translucent family).

### Fixed
- **The GL stream's latent overlapping-damage double-composite.** Two
  damage rects sharing a pixel made every layer draw there twice —
  invisible for opaque layers (idempotent), a self-darkening bug for
  translucent ones. The GL damage clip now normalizes to disjoint
  bounding boxes (the software row index always merged the spans).
  Found by the styled-session equivalence test; the effect-rect
  expansion is what first fed the GL path overlapping damage.

## [0.5.0] — 2026-09-29 — Live output re-arrangement

Phase 26: the serving compositor follows the display topology. A
hotplug event is no longer counted and ignored — the serve loop
re-probes the whole topology, compares the re-selected pipeline with
the live one, and acts: a monitor swap migrates the live pipeline
mid-session; the last display going away leaves the compositor
honestly dark while the protocol keeps serving; the first re-plug
relights and the waiting desktop paints again. No session is lost
across any transition. Everything below is `scripts/verify.sh`-green
at 1,727 workspace tests.

### Added
- **`lion-compositor::rearrange` — the re-arrangement decision and
  its four transitions.** `World::rearrange` snapshots every
  connector's status (`ldp_display::hotplug::topology_statuses` —
  the "before" the next signal diffs against), re-asks the one
  question `serve::select_pipeline` answers, and compares the answer
  with the live pipeline: same pipeline → `Spurious` (stability over
  preference — a connector appearing while the served one lives is
  topology noise); different pipeline → `Migrated` (monitor swap, or
  a mode renegotiation on the same connector — the *pipeline
  comparison* decides, not the status diff, because a sink can
  re-negotiate its mode list without any status change); no pipeline
  while lit → `Dark`; a pipeline while dark → `Relit`. The disable
  the device rejects *for a connector that is no longer connected*
  is the one tolerated arm (the kernel tore that pipeline down with
  the connector).
- **The migration choreography — Phase 25's vocabulary, replayed.**
  `stop_pipeline` (the applied disable, the in-flight flip abandoned,
  framebuffers removed through `KmsBackend::rm_fb`, the DRM lease's
  dumb buffers destroyed — mappings drop with the chain) then
  `light_pipeline` (the fresh chain in the world's configured store
  kind: kernel dumbs on the DRM driver, shadow or anonymous-mapping
  on the mock; the applied enable; the output model rebuilt from the
  new connector; the scheduler re-anchored; the bring-up flip landed;
  the whole scene marked dirty — the desktop re-renders onto the new
  display on the next pump). `allocate_drm_scanout` is the one
  allocation both bring-up and re-arrangement share.
- **The honest dark state.** `World.output`/`World.scanout` are
  `Option` — dark is a type, not a flag. Going dark: the scheduler
  parks (`FrameScheduler::park` — pending frame requests die with
  `output_off`, requests made during the dark window *defer* until
  light returns, exactly the DPMS-off mirror the scheduler was born
  with); every pending buffer fence flushes immediately (nothing is
  being read anymore); bound output objects are revoked. While dark:
  commits still commit (the scene absorbs state — the desktop waits
  for light), attach swaps release their fences without flip gating,
  grabs answer `failed(no_output)`, and binding the output global
  revokes the fresh object immediately (the client learns "no
  display right now" instead of waiting for a cascade that cannot
  come). Relight answers the deferred requests on the new timeline.
- **The client story — the protocol's revocation vocabulary.** The
  output object a client holds mirrored a pipeline that no longer
  exists: it is revoked (`capability_revoked`) through the outbox
  wake-point delivery (the same FIFO every cross-client event uses),
  and the re-bind delivers the fresh cascade. Uniform on purpose: a
  mode renegotiation and a monitor swap are the same event to a
  client. The outbox gains revoke entries (`OutboxEntry::revoke`),
  drained through `DispatchCtx::revoke` with the same liveness gate
  events get.
- **`FrameScheduler::reanchor(nominal)` — the timeline handoff.**
  The scheduler's clock restarts on the new output's nominal refresh:
  windowed registrations on the dead timeline die with `OutputOff`,
  deferred requests survive to be answered when the bring-up flip
  anchors the fresh clock — the unanchored state a scheduler is born
  in, so new requests defer the same way. The scheduler unit suite
  pins all four behaviors (drops, deferred survival + new-nominal
  answers, unparking, input ordering across the swap).
- **`capture_error::no_output` — the spec's second growth.** A grab
  on a dark compositor answers `failed(no_output)` instead of lying
  with `out_of_memory`; `spec/capture.toml` grows the value through
  the full ldpc pipeline (generated tables, blob, reference docs —
  the drift gate proves the regen). The latent `Value::Uint32` where
  the schema wants `Value::Enum` in the `failed` emission path (both
  reasons) is fixed with it — outbound arg validation would have
  rejected the event.
- **The Phase 26 exit criteria — `hotplug_rearrange.rs`.** THE swap
  test: the served connector dies mid-session, the migration moves
  the pipeline (applied state asserted on the mock: new connector
  bound, old unbound, plane fed, fresh FB scanning out), the client's
  output object is revoked, the re-bind delivers the new cascade, and
  the very same surface presents its next frame on the new display —
  pixel-exact scanout (the 2x2 quad over black at the new geometry).
  The dark test: everything released, the session still serving,
  fences flushed, deferred requests, refused grabs. The relight test:
  the deferred request answered on the new timeline, the desktop
  painting again. Plus stability, re-anchoring (a 144 Hz swap answers
  frame requests at the new nominal), mode renegotiation, and
  bind-while-dark.

### Changed
- **`World` owns the DRM lease.** The kernel objects (GEM handles,
  framebuffers) move from the `Compositor` handle into the world, so
  live re-arrangement can release and re-mint them as the topology
  moves; teardown reads them from the world, and a dark teardown is
  the honest no-op (the re-arrangement already released everything).
- **The serve loop's device service is the hotplug arm.**
  `service_device_events` — the zero-timeout drain that never moves
  the mock's clock — now runs the re-arrangement whenever a hotplug
  event rode along, and returns its summary; `serve_kms` prints the
  operator's report line ("hotplug: migrated eDP-1 (1920x1080) ->
  HDMI-A-1 (1920x1080)", "dark (protocol serving; waiting for a
  display)", "lit — the desktop paints again").
- **The pump's dark short-circuit.** A dark world skips render and
  flip (there is nothing to render into and nothing to land), keeps
  absorbing commits (the waiting desktop), and flushes releases —
  the headless time doctrine holds: the clock moves only at the
  re-arrangement's bring-up waits, which are waits on the device.

## [0.4.0] — 2026-09-28 — The real-KMS serve loop

Phase 25: `lion-compositor --mode drm` serves for real on hardware —
DRM-Master takeover, kernel dumb buffers mapped for CPU composition,
the applied atomic modeset, page flips on the DRM fd, LDP clients on
the abstract socket, clean teardown on exit. The serve choreography
is one code path over both backends, CI-proven on the mock device
with the real `mmap` delivery path byte-equal. Everything below is
`scripts/verify.sh`-green at 1,715 workspace tests.

### Added
- **`ldp-display::driver` — the serve loop's device seam.**
  `DisplayDriver` unions `KmsBackend` (state: topology, properties,
  commits, events) with the time-and-wait surface a serve loop needs:
  `now()` (the mock's injected clock; the kernel's CLOCK_MONOTONIC on
  real hardware — one `Mono` domain with the flip timestamps, so
  scheduler math is backend-independent) and `wait_events(timeout)`
  (the mock advances its deterministic clock exactly to the next due
  event — the headless doctrine, unchanged; the real driver blocks in
  `poll(2)` on the DRM fd). The Phase 24 rehearsal named this seam as
  Phase 25's core: the wake-point integration.
- **`ldp-display::serve` — the choreography both backends share.**
  `select_pipeline` (the one output-selection question every caller
  asked separately, now asked once), `enable`/`disable` (the applied
  atomic commits that light up and extinguish a pipeline — the
  Phase 24 rehearsal request minus `TEST_ONLY`, and its teardown
  mirror: plane released, CRTC off, connector unbound), and
  `write_frame_rows`/`read_frame_rows` — the pitch-honoring CPU
  delivery pass into mapped scanout (the kernel's row pitch may
  exceed `width * 4`; rows copy independently, padding untouched).
- **The mapped dumb-buffer layer.** `DrmBackend::map_dumb` —
  `DRM_IOCTL_MODE_MAP_DUMB` (the request number pinned against the
  published ABI by a unit test that once again caught an arithmetic
  slip in the expected constant: 16<<16 is 0x10 in the size field)
  followed by `mmap(PROT_READ|PROT_WRITE, MAP_SHARED)`; `DumbMapping`
  owns the region and unmaps on drop. `anon_mapping` mints the same
  lifetime discipline without a kernel object — the CI vehicle for
  the real delivery path. `poll_readable` and `monotonic_now` join
  the audited `drm::sys` FFI surface, each call carrying its SAFETY
  note.
- **`lion-compositor --mode drm` — the real serve loop.** Bring-up:
  master rights, two kernel dumb buffers, framebuffer registration,
  both mappings zeroed opaque black, the applied atomic enable, the
  bring-up flip latched as the scheduler's first anchor. Service:
  `serve_kms` polls the listening socket *and* the DRM fd — LDP
  clients on one, page flips and topology events on the other — with
  SIGINT/SIGTERM (async-signal-safe handlers, the audited `sys`
  layer) as the exit path. Teardown: the applied disable commit, then
  framebuffers released, dumb buffers destroyed, master dropped —
  the display left dark, not wedged. `--probe` keeps the zero-risk
  TEST_ONLY rehearsal (the Phase 24 `--mode drm` behavior) for
  operators; `--mode drm` on a machine without DRM nodes fails typed
  and loud — never a silent headless fallback.
- **The serve-loop equivalence suites.** `kms_serve.rs` (the Phase 25
  exit criterion): the identical two-frame client session driven
  through the shadow store and through the **mapped** store (real
  `mmap` regions, pitch-honoring writes — the DRM path's delivery
  vehicle) produces byte-equal scanout; teardown's applied disable
  asserted against the mock's live state (plane/CRTC/connector all
  released); the serve loop's own device service (hotplug) drains
  without moving the deterministic clock; and the binary's
  `--mode drm`/`--probe` on nodeless machines fail honestly with the
  reason on stderr. `ldp-display`'s `serve_loop.rs`: applied
  enable/disable state assertions, pipeline selection, the
  mapping-round-trip with a padded pitch.

### Changed
- **`World.device` is a `Box<dyn DisplayDriver>`.** The frame loop's
  device calls flow through the trait — mock and real execute the
  same render → flip → land → release choreography; the mock's
  `wait_events` reproduces the old advance-to-landing semantics
  exactly (the 1,696 prior tests re-prove it). `ScanoutChain` gained
  the store split: `Shadow` (the headless oracle) and `Mapped` (real
  mappings at the kernel's pitch); `scanout_words()` reads either.
- **`--mode auto` serves DRM when a device opens.** The mode probes
  quietly and, on success, enters the real serve loop (the old
  behavior — probe and exit — is gone; `--probe` replaces it for
  operators who wanted exactly that).
- `OutputGlobal::from_backend`: the output model over any backend's
  connector snapshot (EDID optional — real sinks without one still
  bring up), VRR reported honestly as unsupported until the detection
  walk ships.

### Boundaries (honest)
- Hotplug events on real hardware are counted and reported; live
  output re-arrangement (re-probe, re-scanout, scene migration) is
  the next display milestone. VRR-on-real-nodes detection is not yet
  wired (the mock's VRR window still serves the headless path). The
  release-fence story is unchanged: the eventfd stand-in in headless,
  kernel out-fences documented for the DRM path.

## [0.3.0] — 2026-09-28 — Hardware acceleration by default (the macOS doctrine)

Phase 24: GPU compositing through a real EGL+GLES backend whenever
the machine has a GL stack, software fallback with the reason carried
— *by default, never a silent surprise* — plus a real-hardware DRM
scanout rehearsal. Everything below is `scripts/verify.sh`-green at
1,696 workspace tests.

### Added
- **`ldp-renderer::gles` — the GL rendering backend behind the same
  `Renderer` contract.** The object-safe `GlesApi` command seam
  (programs, textures, offscreen targets, scissored passes,
  premultiplied-over draws, readback), the `GlesRenderer` (damage
  rects become passes; layers that miss a rect never draw; per-format
  upload swizzles; the pinned GLSL composite shaders), the reference
  evaluator `RefGles` (executes the command stream with the exact
  integer blend rule), the `RecordingGles` stream recorder, and the
  selection policy (`RendererChoice::{Auto, Gles, Software}` +
  `resolve`): **Auto is hardware-first** — a working GL context wins,
  a missing stack falls back to software with the reason in the
  one-line startup report, and a forced `gl` without hardware is a
  hard typed failure, never a silent CPU path.
- **`ldp_gpu::gles` — the real FFI backend.** `GlesContext` owns the
  whole hardware story: dlopen'd `libEGL.so.1`, the initialized
  display, an RGBA8888 pbuffer-capable config, a GLES 2.0 context,
  and the 48-entry `gl*` table resolved through `eglGetProcAddress`
  (every `unsafe` in the tree carrying its `SAFETY` note, the
  transport/display precedent). `RealGles` implements the renderer's
  seam over it, migrating the context to the calling thread per
  command under the world mutex (a dedicated render thread is the
  next GL milestone). On a machine without a GL stack the constructor
  fails typed — the honest headless outcome, CI-pinned.
- **`lion-compositor --renderer auto|gl|software` (default: auto).**
  The compositor's whole render pipeline — window-management redraws,
  animation frames, damage repaints, the capture path — flows through
  the selected backend; the startup prints the honest report
  (`renderer: gles (hardware: EGL + GLES 2.0 context up)` or
  `renderer: software (GL unavailable: …)`). The Phase 24 exit
  criterion test drives the **entire client session** through the
  reference GL backend and demands **byte-equality** with the
  software-driven session over the full 1920x1080 scanout
  (`binaries/lion-compositor/tests/hardware_renderer.rs`).
- **The DRM hardware rehearsal (`--mode drm`).** Beyond topology
  probing, a real bring-up rehearsal: `drmSetMaster` (the modeset
  privilege), two dumb scanout buffers for the mode's geometry
  (`DRM_IOCTL_MODE_CREATE_DUMB` through the audited `drmIoctl` seam,
  the request numbers pinned against the published ABI by a unit
  test), framebuffer registration, and the full atomic enable commit
  under `TEST_ONLY` — validated by the kernel, never applied (the
  user's screen never blinks) — then unwound and reported step by
  step. `ldp-display`'s FFI grew the master/dumb layer for it.
- The GL equivalence corpus (`crates/ldp-renderer/tests/
  gles_equivalence.rs`): randomized layers (all four 32-bit RGB
  formats, off-screen placements, random damage, both damage-empty
  and multi-rect frames, a second frame on the same renderers for the
  damage-persistence contract) rendered through both backends and
  compared byte-for-byte; the command-stream goldens
  (`gles_stream.rs`) pin exactly what the renderer emits per frame,
  including the drop teardown and the skip decisions.

### Changed
- Workspace version 0.2.0 → 0.3.0; every `--version` surface now
  reports it; the showcase scene's subtitle mark reads `v0.3.0`.
- `RendererError` gained `Gles(GlesError)` and `UnsupportedLayer` —
  layers outside the GL v1 subset (YUV, transforms, scaled placement)
  fail typed instead of rendering wrong; the caller composites those
  frames with the software backend. `ldp-renderer`'s `gles` and `view`
  modules are now public API surface.
- `ldp-gpu` depends on `ldp-renderer` at runtime (RealGles implements
  the renderer's seam); still zero external dependencies beyond
  `libc` — the dependency policy gate stays green.
- `Compositor::headless` takes the config by value (the injected
  renderer backend is a move-once resource for the test seam); all
  in-tree callers converted.
- `binaries/lion-compositor`'s DRM mode: probe → probe + rehearsal
  (the outcome report carries the `RehearsalReport`).

### Scope boundaries (the honest lines)
- **Real-KMS scanout *service* is the next display milestone**
  (Phase 25): the rehearsal proves every bring-up step on real
  hardware, but the always-on serve loop (wake-point event
  integration, flip landing at real vblank, outbound presentation
  pushes from a frame thread) is not shipped in 0.3.0.
- **The GL v1 path composites the 32-bit RGB family, 1:1 placements,
  `Transform::Normal`**: YUV shader uploads, scaled sampling, the
  DMA-BUF zero-copy import, and a dedicated render thread are the
  next GL milestones. On a physical GPU the fixed-function blend may
  differ from the integer reference by ±1 LSB (float hardware); the
  reference evaluator is the conformance oracle and the command
  stream is identical.

## [0.2.0] — 2026-09-28 — Milestone release: the protocol grows up

The consolidation of the two post-v0.1.0 phases — remote transport
(Phase 21) and capture + PNG + the next performance tier (Phase 22) —
plus the Phase 23 release work: one coherent version across every
surface, a uniform `--version` on every shipped binary, and the
release-coherence gate that keeps them honest. Everything below is
`scripts/verify.sh`-green at 1,672 workspace tests.

### Added
- **`--version` / `-V` on every shipped binary** (Phase 23). All
  fourteen executables — `lion-compositor`, the eight operator tools
  (`ldp-info`, `ldp-debug`, `ldp-validate`, `ldp-profiler`,
  `ldp-audit`, `ldp-input-debug`, `ldp-grab`, `ldp-remote-gateway`),
  `ldp-bench`, `ldpc`, and the four examples — print exactly
  `<name> <workspace version>` and exit 0. The shared
  `Args::take_version_flag` carries the convention across the tools,
  every usage text documents the flag, and each package's live suite
  pins the output against `CARGO_PKG_VERSION`
  (`tools_live.rs::every_tool_reports_its_version` plus the
  per-package `version_flag_reports_the_release` tests) — so a future
  bump that misses a binary fails CI.
- **`scripts/version_check.py` — the release-coherence guard**
  (Phase 23, wired into `verify.sh` after the drift gate). One
  version, stated everywhere it matters: the workspace `Cargo.toml`,
  every member manifest (inheritance or equality), the `Cargo.lock`
  pins of the in-tree packages, the top `debian/changelog` entry
  (the base of any `~phase` pre-release must match), the README
  milestone status, the CHANGELOG milestone heading, and the showcase
  scene's version mark. A release can no longer be half-bumped — the
  gate fails the build naming every drifted file.
- The final all-in-one bundle `lion-display-v0.2.0.zip` (this
  repository at the milestone, packaged from the release commit via
  `git archive`; re-proven by a fresh-extraction `verify.sh` run).

### Changed
- **Workspace version 0.1.0 → 0.2.0** — the single-point bump every
  member inherits; `Cargo.lock` regenerated; every `--version`
  surface now reports `0.2.0`.
- `examples/showcase` — the scene's subtitle mark now reads `v0.2.0`
  (the deterministic render every pixel-exact suite pins moves with
  it).
- `debian/changelog` — the `0.2.0` release entry rolling up the
  `0.2.0~phase21` / `0.2.0~phase22` pre-releases; `debian/control`
  descriptions raised to the v0.2.0 census (9 modules, 34
  interfaces, 213 operations; eight live tools plus `ldp-bench` and
  `ldpc`).
- `README.md` — status raised to the v0.2.0 milestone; the Phase 23
  entry; Remote and Capture rows in the feature matrix; the
  repository layout and installation sections updated for the 0.2.0
  packages.
- `docs/roadmap.md` — Phase 23 recorded as delivered with its exit
  criteria; the milestone table now carries the delivered `v0.2.0`
  row and the remaining `v0.3.x` directions.
- `docs/maturity.md` — the matrix title, the always-on gates table
  (34 members, 1,672 tests, 9 spec modules, the new
  release-coherence gate), the Phase 23 row, and the beyond-section
  raised to v0.2.0.
- `docs/user-guide.md` / `docs/admin-guide.md` — the header census
  (9 modules, 34 interfaces, 213 operations, 1,672 tests) and the
  operator-tool count (eight live tools) updated; the packages'
  `--version` surface documented.
- `scripts/build-deb.sh --verify` — the fresh-root stage now pins the
  verified `.deb` pair to the *current* changelog version instead of
  globbing `dist/` (explicit names, mirroring `do_package`), and
  asserts every installed binary's `--version` output equals the
  changelog version — the packaging-level arm of the
  release-coherence gate. The glob was a latent hazard caught on this
  release's first packaging run: stale `0.2.0~phase21` debs left in
  `dist/` matched the wildcard and unpacked *over* the release pair,
  silently downgrading the fresh root to pre-`--version` binaries.
  Now impossible by construction.

### The v0.2.0 census
- 34 workspace members (25 crates under `crates/`, `ldpc` +
  `ldp-tools` under `tools/`, `lion-compositor`, the four examples,
  `fuzz`, and `tests`), 9 spec modules, 34 interfaces, 213 operations
  (91 requests, 122 events), 32 enums, 16 bitsets — one protocol,
  compiled from one TOML source of truth with the drift gate green,
  and *grown once after v1.0* (capture, Phase 22) with the pipeline
  absorbing it whole.
- 1,672 tests, five deterministic fuzz targets under one CI gate, the
  cross-crate stress gate, and the committed benchmark report
  (`docs/benchmarks.md`) including the Phase 22 renderer rows (the
  7.4× opaque word-copy win and the per-frame damage row index) and
  the Phase 21 remote suite.
- Zero unsafe code in the server/compositor/client/tool paths;
  `ldp-core` still has zero dependencies; the MSRV CI job still pins
  1.75.
- Over the wire and across hosts: the showcase example remains
  pixel-exact through both remote gateways, and so do captures
  (the Phase 21/22 exit criteria, re-proven green in this release's
  verify run).

## [0.2.0-phase22] — 2026-09-28 — Capture, PNG, and the next performance tier

The second post-v0.1.0 phase: the protocol's first *growth* (a ninth
module landing through the full ldpc pipeline — spec, generated tables,
drift gate, server, client, tool, remote relay), a zero-dependency PNG
encoder, and two renderer optimizations with permanent benchmark rows.
Everything below is `scripts/verify.sh`-green at 1,700+ workspace tests.

### Added
- **`spec/capture.toml` — the `ldp.capture` module (v1).** The first
  protocol surface added *after* v1.0, proving the spec→ldpc→registry
  pipeline absorbs growth: `capture_manager` (global) with `grab()` →
  a read-once `frame(fd, width, height)` snapshot (premultiplied
  ARGB8888 words, exactly the scanout) or `failed(out_of_memory)`.
  The spec, generated tables, introspection blob, and reference docs
  regenerate byte-identically through `ldpc gen`; the drift gate, the
  round-trip corpus, and the conformance walker pick the three new
  operations up automatically (210 → 213 ops, 8 → 9 modules).
- **`crates/ldp-png` — the zero-dependency PNG encoder.** Deterministic
  output (bit-identical across platforms and runs — a dumped frame is
  diffable in CI), adaptive per-row filters (None/Sub/Up/Average/Paeth,
  minimum-sum-of-|signed byte| heuristic), and a fixed-Huffman
  hash-chain deflate (32 KiB window, bounded chain walks, strictly
  deterministic tie-breaks). 24 tests including a full **independent
  decoder** in the round-trip suite: its own inflate, CRC-32, Adler-32,
  and un-filter re-derive the file's pixels from the RFC — the encoder
  is proven, not trusted.
- **The capture implementation.** The compositor advertises the global,
  copies the scanout out under the render lock (never torn, never
  blocking the frame loop on client I/O), and ships it as a fresh memfd
  through the `frame` event's descriptor — the same whole-file
  vocabulary keymaps and ICC profiles use, so **`ldp-remote` relays
  captures with a one-arm tracker change** and the loopback gate proves
  the frame pixel-exact through both gateways. The end-to-end suite:
  pixel-exact grabs against the scanout oracle, repeated-grab
  stability, the empty-scene baseline, and FD-count stability across
  grab-heavy session lifecycles.
- **`ldp-grab` — the seventh operator tool.** Bind, grab, write: PNG
  (default) or raw PPM (`--format ppm`, byte-exact against the
  scanout), `--count N --interval SECS` for multi-frame sequences.
  Runs live in CI against the in-process compositor as the compiled
  binary (the tools_live precedent).
- **CI future-proofing**: the **MSRV job** (the `rust-version = "1.75"`
  promise is now built and unit-tested on 1.75.0, not merely declared)
  and the **dependency policy guard** (`scripts/dep_policy.py`, wired
  into CI and `verify.sh`: any Cargo.lock entry outside the in-tree
  set + libc + ldpc's toml/serde front-end fails the build).
- **`ldp-bench` Phase 22 rows**: the ARGB opaque-pixel composite and
  the 12-window/32-rect desktop shape — the two benchmarks the
  optimizations target, as tracked numbers.

### Changed
- **Renderer: the `CopyPremul` path.** Same-format 1:1 untransformed
  fully-opaque ARGB8888/ABGR8888 layers (real window interiors — the
  most common compositing shape) now word-copy 64-pixel chunks when
  every alpha is 255 (bit-identical to the blend: the `over` of an
  opaque source is the source, and `pack ∘ unpack` is the identity on
  a same-format word) and fall back to the per-pixel blend on mixed
  chunks, so translucent edges blend exactly as before. **7.4x** on
  the tracked benchmark (51.26 ms → 6.84 ms median, same machine,
  before/after).
- **Renderer: the per-frame damage row index.** `RowIndex` builds every
  output row's merged damage x-intervals once per frame (counting sort
  into a retained arena; all buffers reused — the Phase 21
  `Region::subtract` allocation doctrine) and `composite_layer` serves
  each layer-row by clipping the row's intervals to the destination —
  O(intervals) instead of a rescan of every damage rectangle per
  layer. Cross-validated against the reference collector on a
  randomized corpus (the collector is retained `#[cfg(test)]` as the
  oracle). **~12%** on the desktop-shape benchmark (524.7 µs →
  464.7 µs median).
- The protocol-shape inventory pins updated for the ninth module:
  9 modules, 34 interfaces, 91 requests, 122 events, 32 enums, 213
  operations, 10 generated files.

## [0.2.0-phase21] — 2026-09-28 — Remote transport, perf, future-proofing

The first post-v0.1.0 phase: the roadmap's v0.2 "remote transport"
direction, delivered as a new crate plus a measurable optimization and
permanent new benchmark coverage. Everything below is
`scripts/verify.sh`-green at 1,623 workspace tests.

### Added
- **`crates/ldp-remote` v0.1.0 — LDP over the network.** The TCP relay
  layer: an **edge gateway** accepts ordinary local `AF_UNIX` clients
  (unmodified `ldp-client`, `ldp-tools`, and every example work
  unchanged — they dial the edge exactly as they would dial the
  compositor) and bridges each one to a **hub gateway** near the
  compositor over authenticated TCP. The wire is a 16-byte envelope
  protocol (`LDR1`, v1) whose length fields are validated **before**
  any allocation — the Phase 19 length-bomb lesson applied at layer
  zero of a new surface — with negotiated caps
  (`max_envelope`, `pool_total_cap`), a constant-time bearer-token
  `HELLO`/`HELLO_ACK` handshake, and `PING`/`PONG` keepalive with
  dead-peer deadline teardown that unblocks every pump thread through
  a shutdown-set discipline designed to make fd-reuse races
  impossible. The FD relay vocabulary translates every
  descriptor-bearing protocol path into bytes: shm pools ship whole at
  `create_pool` and then per-commit buffer windows before every
  `surface.commit` (the write-then-commit shm contract, preserved
  across a network), pool growth ships as `POOL_RESIZE`, eventfd
  release fences ship as counter snapshots, read-once files (ICC
  profiles, keymaps) ship whole, and clipboard-class pipes stream
  bidirectionally as chunks. GPU descriptors (`dmabuf.create`,
  `fence.import_sync_file`/`import_syncobj`) are rejected explicitly
  with `BYE(unsupported_fd)` — never silently wrong. The
  decode-driven object table both sides maintain resolves against the
  same `ldp-protocol` schema registry the server itself uses, so the
  relay's notion of "which object is which interface" cannot drift
  from the protocol it forwards. 42 tests (36 unit + 6 end-to-end);
  unsafe exists only in the audited `sys` module (memfd/eventfd/pipe2/
  poll/shutdown, each with a `SAFETY` comment — the
  ldp-transport/ldp-tools precedent).
- **`ldp-remote-gateway`** — the operator binary (`hub`/`edge` modes,
  `--token-file`, cap and keepalive flags; hex or raw 32-byte tokens).
- **The Phase 21 end-to-end gate**
  (`crates/ldp-remote/tests/remote_loopback.rs`): the showcase example
  — full client library, introspection, a 960×540 surface, ping-pong
  pools, four committed animated frames with presentation verdicts and
  release fences — runs **unmodified** over loopback TCP through both
  gateways and the compositor's scanout is proven **pixel-exact**
  against the deterministic render; plus concurrent-client isolation,
  the token fast-fail, the 2 GiB length-bomb rejection (one header
  read, zero sessions), keepalive teardown of a silent peer, and FD
  count stability across repeated session lifecycles.
- **`ldp-bench` renderer and remote suites** (permanent rows in
  `docs/benchmarks.md`): `renderer: 3840x2160 opaque composite (full
  damage)` — the Phase 8 EC as a tracked number — and `remote:
  envelope codec round-trip` / `remote: pool-update codec round-trip`.

### Changed
- **`Region::subtract` is ~2× faster** (373.9 → 189.0 ns median on the
  64-cutter corpus, same machine, before/after): the subtraction now
  swaps two retained scratch buffers instead of allocating a fresh
  vector per cutter (O(1) allocations for the whole difference), and
  rectangles disjoint from a cutter pass through without entering the
  four-way split machinery. Damage algebra feeds every frame's
  occlusion subtraction, so the win lands in the per-frame budget.
- **Toolchain future-proofing evidence**: the full workspace (fmt,
  clippy `-D warnings` under pedantic, 1,623 tests, rustdoc
  `-D warnings`) is green under the current stable toolchain
  (Rust 1.98), two years of releases past the declared MSRV (1.75).

### Scope
- Remote v0.2 is **single-user remote display**: the hub attributes
  whole sessions to the gateway's local account (`SO_PEERCRED` cannot
  cross a network); multi-user remote identity is broker-era work.
  Token auth is a bearer secret, not TLS — private links only until a
  transport-security phase exists. dmabuf/explicit-sync descriptors
  are refused, not proxied. These boundaries are stated in
  `docs/maturity.md` (boundary 6 rewritten from "no remote transport"
  to the delivered single-user scope).

## [0.1.0] — 2026-09-27 — Milestone release: the 20-phase bootstrap complete

### Added
- **Debian packaging** (`debian/`, `packaging/`): source package
  `lion-display` (native 3.0) with two binary packages —
  `lion-compositor` (the reference server + the hardened systemd
  unit) and `ldp-tools` (the six operator tools, `ldp-bench`,
  `ldpc`, and the three examples under
  `/usr/lib/lion-display/examples/`). The rules are deliberately
  debhelper-free: plain POSIX make driving cargo +
  `dpkg-gencontrol` + `dpkg-deb --build --root-owner-group`, with
  `Rules-Requires-Root: no` — the whole build runs unprivileged
  anywhere `dpkg-dev` and a Rust 1.75+ toolchain exist. Docs ship
  as package documentation (admin guide + maturity matrix with the
  compositor; user guide with the tools); md5sums ship for
  `dpkg -V` integrity checks.
- **`scripts/build-deb.sh`** — the packaging pipeline in three
  stages (`--build` / `--package` / `--verify`): release build,
  `dpkg-buildpackage -us -uc -b`, and the fresh-root install gate —
  both `.deb` files install with real `dpkg` against a pristine
  root inside a user namespace, installed status is asserted,
  every installed binary is smoke-tested (compositor `--selftest`,
  tool/example `--help` walks), and the unit passes
  `systemd-analyze --root=... verify`. This is the Phase 20 exit
  criterion "installs cleanly in a fresh container" made
  executable; the gate ran green for this release.
- **`packaging/systemd/lion-compositor.service`** — the shipped
  unit: `--mode auto --quiet`, `Restart=on-failure`, drop-in
  override documented inline, and default-on hardening
  (`NoNewPrivileges`, `ProtectSystem=strict`, kernel-tunable/
  module/cgroup/log/clock/hostname protection, namespace and
  SUID/SGID restrictions, `MemoryDenyWriteExecute`, empty
  capability set, `@system-service` syscall filter — with the
  filter's coverage of the stack's syscall needs verified
  syscall-by-syscall against the systemd definitions: memfd_create,
  socketpair, sendmsg/recvmsg, eventfd, epoll_create1, ftruncate,
  fallocate, dup3, pipe2 all resolve into member groups).
- **`docs/user-guide.md`** — the client-developer guide: building
  (verify.sh, release profile), the compositor's three modes and
  every flag, the one socket convention (`--socket` /
  `LDP_SOCKET`, abstract namespace, `SO_PEERCRED` identity), the
  six operator tools with their real usage surfaces and typical
  sessions, the three examples, the `ldp-client` session shape
  (handshake, generation-safe proxies, deadline-driven commits,
  class lanes), the deterministic-headless doctrine, the
  environment-variable reference (`LDP_SOCKET`, `LDP_STRESS_FULL`,
  `LDP_FUZZ_SCALE`, `LDP_CORPUS_DEBUG`, `LDP_CORPUS_TRACE`), and a
  troubleshooting section mapped to real exit codes and error
  strings.
- **`docs/admin-guide.md`** — the operator guide: package
  contents (usr-merge file map), source-package building, service
  management (enable/edit-drop-in, what `--mode auto` does on
  headless vs DRM machines, why clean probe exits don't loop), the
  three-layer security model as it affects operations
  (kernel-attested identity, manifest-brokered capability tokens,
  the hash-chained audit and how to verify it), enforced resource
  ceilings with the stress-gate evidence, performance expectations
  with the committed benchmark rows, and troubleshooting keyed to
  journalctl symptoms.
- **`docs/maturity.md`** — the honest scope matrix: four maturity
  levels (CI-hardened / integration-proven / mock-verified /
  staged) defined by evidence class; the always-on cross-cutting
  gates table (fmt, clippy -D warnings, 1,576 tests, rustdoc -D
  warnings, spec lint, drift check, architecture lint,
  forbid(unsafe), zero-dependency core); the per-phase matrix
  (Phases 1–20 → landed artifacts → level → the named suites that
  prove it); and six explicit scope boundaries (real-hardware
  pixel delivery staged, GL backend staged, bridges
  in-process-only, broker UX staged, logind protocol-level not
  live, no Vulkan/VR/remote) stated the way the runtime states
  them.
- The final all-in-one bundle `lion-display-v0.1.0.zip` (this
  repository at the milestone, packaged via
  `scripts/package-phase.sh 20`).

### Changed
- `docs/roadmap.md` — Phase 20 marked delivered with the exit
  criteria recorded as met.
- `README.md` — status raised to the v0.1.0 milestone; the
  repository layout now includes `debian/`; the documentation
  index links the three guides and the maturity matrix; install
  section points at the packages and `build-deb.sh`.
- `packaging/README.md` — rewritten as the as-built description
  of the layout, build, and verification pipeline.

### The v0.1.0 census
- 31 workspace members (22 protocol/system crates, `ldpc` +
  `ldp-tools`, `lion-compositor`, 3 examples, `fuzz`, `tests`),
  8 spec modules, 33 interfaces, 210 operations, 90 requests,
  120 events, 31 enums, 16 bitsets — one protocol, compiled from
  one TOML source of truth, with the drift gate green.
- 1,576 tests, five deterministic fuzz targets under one CI gate,
  a 15-minute full stress gate with committed numbers, and a
  release benchmark report in `docs/benchmarks.md`.
- Zero unsafe code in the server/compositor/client/tool paths;
  syscall seams isolated in the audited transport/display/gpu/
  session crates; `ldp-core` has zero dependencies.

## [0.1.0-phase19] — 2026-09-27 — Test, fuzz, stress, benchmarks

### Added
- `crates/ldp-test` v0.1.0 — the deterministic test-support library
  every Phase 19 harness shares: `rng` (SplitMix64, seed-addressed:
  a finding names a seed and replays exactly), `mutate`
  (structure-blind byte mutators plus envelope-aware message mutators
  over the 16-byte LDP header and a deterministic stream chunker),
  `conformance` (the spec-walking conformance runner: for every
  interface, direction, and opcode compiled into the registry — 8
  modules, 33 interfaces, 210 operations — synthesize the canonical
  argument list from the signature, prove the strict
  encode/decode/signature round-trip, then sweep the per-type
  boundary matrix: integer extremes, at-limit and one-past-limit
  strings, null-object nullability, undeclared enums, over-width
  bitsets, wrong-ownership new_ids, FD-index rejections; plus the
  rng-driven `spec_message` generator the fuzzers build corpora
  from), `json` (the hand-rolled writer behind the reports), `bench`
  (the benchmark harness: warmup, timed runs, median/mean/p95/min/
  max, `ns/op` and `MiB/s` rows that never mix units), and
  `benchmarks` (the suites behind `ldp-bench`).
- `ldp-bench` (bin) — the Phase 19 benchmark runner: performance
  (codec encode/decode/signature-check on real messages, transport
  round-trips over a real socketpair at 1 KiB and 64 KiB, region
  union/subtract, the 60 Hz scheduler decision cadence, sRGB/PQ
  transfers and BT.709→BT.2020 matrix application, a 16 MiB clipboard
  stream through a real pipe) and security (the constant-time token
  walk — valid, forged, cross-app — the hash-chained audit log's
  append and full verification, and the 18-operation permission
  matrix sweep). `--json`/`--markdown` reports, `--quick` for
  dev-profile smoke. Every timed loop runs its full `ops_per_run`
  inside the timed region with `black_box` anchors, and the physical
  sanity of the transport numbers is pinned by
  `examples/transport_check.rs` (the first draft of this harness once
  reported ~496 GiB/s for a socketpair round-trip by dividing one
  round-trip by an ops count the closure never performed — the check
  exists so that class of bug can never ship).
- `fuzz/` (`ldp-fuzz`) — five deterministic fuzz targets, no external
  framework: `codec` (spec-synthesized valid messages must round-trip
  bit-exactly and pass strict signature validation; envelope-aware
  and blind mutants must come back as structured errors in both
  validation modes against varying declared FD-table sizes),
  `transport` (real socketpairs: fragmented delivery, declared
  FD-count lies, real SCM_RIGHTS `/dev/null` batches, half-delivered
  crash shapes; the process FD table must be byte-stable across the
  whole run), `dispatch` (a real `ldp-server` accept loop: fuzzed
  bursts of mutated requests, aligned garbage, forged target object
  ids, and event opcodes smuggled into the request direction —
  sessions die with structured verdicts, are fully reclaimed, and the
  FD baseline is restored), `x11` (short and BIG-REQUESTS framing in
  both endiannesses; consumed sizes must equal payload + header),
  `wayland` (the pinned-schema valid round-trip plus mutants whose
  `Incomplete` bounds must exceed the buffered bytes and make strict
  progress when honored). The CI gate (`tests/fuzz_ci.rs`) runs all
  five under one serialized test with a counting panic hook across
  every thread, asserts both accept and reject paths on every target,
  and proves seed determinism for the in-memory targets;
  `LDP_FUZZ_SCALE` multiplies budgets for soak runs.
- `tests/` (`ldp-integration`) — the cross-crate gates, all driving
  the real in-process Phase 10 compositor through a harness that
  replicates the testbench shape as a library (bounded event
  recorder, memfd pools through the FD-carrying factory path, the
  full frame cycle, and `canary_session` — the healthy-session
  proof): `stress` (THE stress gate — 32 concurrent clients with
  mixed workloads: frame cycles, sync round-trips, buffer churn,
  crash-and-reconnect; compressed ~3 s mode in CI, `LDP_STRESS_FULL=1`
  for the 15-minute gate), `crash` (the corpus: eight protocol cut
  points plus a raw mid-frame transport crash and a 24-round seeded
  churn — every crash must drain and leave the server
  canary-healthy), `hotplug` (device-level `Reprobe` diffs for
  connect/unplug/replug plus the same injections into the
  compositor's own mock device while a client keeps committing and
  presenting across every churn step).
- `docs/benchmarks.md` — the committed Phase 19 report: release-build
  micro-benchmark tables (JSON schema documented), the full
  15-minute stress gate numbers (32 clients, 900.6 s, 30,056
  sessions — 4,909 ending in abrupt disconnect and reclamation —
  104,918 committed frame cycles, 129,819 frames composited, canary
  healthy, scene drained, FD table back to baseline), and the fuzz
  CI budget table.

### Fixed
- **Wayland bridge length-bomb DoS** (found by the fuzz gate's very
  first run): the bridge's input buffer grew without bound while the
  decoder waited on an `Incomplete` message that could never complete
  — a client declaring a ~2 GiB string/array length and then
  streaming slowly wedged unbounded memory growth into the server.
  Fix mirrors the X11 bridge doctrine: `wire::max_message_bytes()` is
  one LDP `large_messages` frame (64 MiB — a foreign message larger
  than an LDP frame can never be forwarded whole), enforced twice in
  `dispatch.rs::feed` — a per-message guard (an `Incomplete` bound
  past the ceiling is fatal immediately) and a total-buffer guard
  (defense in depth against future schema or decoder drift).
  Regression suite: `crates/ldp-wayland-bridge/tests/length_bomb.rs`
  (the huge string claim is fatal not buffered, the ceiling is pinned
  to one large frame with the codec's honesty about its bound
  asserted, the total-buffer guard fires under a sub-ceiling claim
  with endless streaming, and ordinary partial delivery still
  completes and dispatches).
- **Compositor post-mortem outbox leak** (found by the crash corpus's
  after-commit cut point): `Scene::drop_client` cleared the crashed
  client's routes, pools, buffers, and output binds — but not its
  pending buffer releases. The next flip then landed a `release`
  event for the dead client, re-creating its outbox queue post-mortem
  with an eventfd descriptor inside (one leaked fd and queue entry
  per mid-frame client crash). Fix: `drop_client` now also retains
  `pending_releases` per client; the corpus is the regression test.

### Exit criteria (met)
- Fuzzers run clean for the CI budget — no panics (counting hook
  across all threads), no OOM (bounded corpora and mutants), no leaks
  (transport and dispatch targets assert process-wide FD stability).
- Stress gate stable — the full 15-minute run: 32 clients, mixed
  workloads, 900.6 s, all stability verdicts green (numbers committed
  in `docs/benchmarks.md`).
- Benchmark report committed to `docs/benchmarks.md` — release-build
  performance and security tables plus the stress and fuzz numbers,
  with the JSON schema documented for trend tooling.

1,576 workspace tests green; docs synced (CHANGELOG, roadmap Phase 19
delivered, README status, `fuzz/README.md`, `tests/README.md`).

## [0.1.0-phase18] — 2026-09-27 — Tools & examples

### Added
- `tools/ldp-tools` v0.1.0 — the operator toolchain library: one
  crate, six binaries, every tool's logic a library function the
  integration suites drive the same way the binaries do (the
  `src/bin/*.rs` adapters are argv parsing and exit codes only —
  0 success, 1 reportable failure, 2 usage):
  - `session`: the `ToolSession` driver — the Phase 10 testbench's
    `TestClient` promoted to a public library shape. Connect with the
    introspection option, registry bootstrap with global replay,
    gated binds (an unadvertised interface refuses locally instead of
    letting the protocol kill the connection), shm pools with a
    writable client handle, buffer factories, the full frame cycle
    (frame → attach → damage → commit → the committed confirmation),
    decoded presentation feedback (`presented`/`frame_dropped` with
    flags and refresh), and `registry.introspect` schema streaming.
  - `error`: the `ToolError` surface — client, IO, and
    honest-degradation failures under one type, with `From` bridges
    from `ClientError`, `LdpError`, `io::Error`, and the `sys` seam
    so `?` works at every layer boundary.
  - `socket`: discovery convention (`--socket NAME` with the
    conventional `@` sigil stripped, then `LDP_SOCKET`; no default
    name — the error names the convention instead of guessing).
  - `value_fmt`: the tracer's stable text rendering of wire values
    (quoted strings, tagged objects/descriptors, bitsets as hex
    words, `?` for unknown kinds — the wire enums are
    `#[non_exhaustive]` and the tracer never invents values).
  - `sys`: the audited syscall layer — every `unsafe` in the crate
    lives here with a SAFETY comment (memfd creation and fence
    eventfd reads); `filled_memfd` rewinds after filling so
    duplicated descriptors all start at byte 0.
  - `audit_log`: the JSONL audit-log reader/writer over the real
    Phase 16 chain types — parse in the exact write order, the
    structural seq-set check (deletion and duplication are
    `NonContiguous`; *reordering* passes the structural gate because
    the hash chain catches it — every digest links to its
    predecessor), verification verdicts, and head checkpoints.
  - The six tools: `ldp-info` (compiled protocol summary; live
    session report with the globals/registry cross-check, the output
    cascade, and the served-schema introspection sweep; DRM/KMS +
    GPU probe — real nodes or an honest "unavailable", never mock
    data), `ldp-debug` (live event tracer with optional extra binds,
    generated presentation traffic, class/filters, per-class census
    and deadline hit-rate; scheduler-recording replay with the same
    statistics), `ldp-validate` (spec-set compilation through the
    real `ldpc` + the live served-schema cross-check, one verdict
    per advertised global), `ldp-profiler` (live frame-pipeline
    measurement through the full choreography — wall latency,
    pipeline pacing, deadline hit-rate — and the same statistics over
    recordings), `ldp-audit` (log loading, record listing, chain
    verification, operator checkpoint comparison — the only layer
    that catches a *truncated* tail, which stays internally
    consistent by construction), and `ldp-input-debug` (evdev dump
    analysis: raw census → SYN_REPORT framing with
    dropped/protocol-A surfacing → the real Phase 11 normalizer over
    an inferred device spec, with class hints).
- `examples/hello-ldp` v0.1.0 — the smallest complete client:
  connect, bootstrap, a 64×64 two-buffer surface, one committed
  frame, the presentation verdict. The scripted scenario asserts the
  fill is pixel-exact in scanout (and only the 64×64 quad — the
  desktop behind stays untouched) plus the dead-socket fast-fail.
- `examples/pointer-paint` v0.1.0 — the input-driven client: an
  evdev trace through the real Phase 11 normalizer, every motion a
  painted pixel in a committed surface. The scripted scenarios
  assert the stroke lands pixel-exact (16-point closed square; a
  two-packet trace paints its points and nothing else) and the
  interior stays background.
- `examples/clip-client` v0.1.0 — the clipboard client: live
  availability report (the Phase 10 slice advertises no data family —
  the client says so and exits cleanly) plus the full in-process
  offer/accept/receive negotiation through the real Phase 13 manager
  — two synthetic clients, serial-bound selection, the routed event
  cascade, MIME narrowing, the `ClipboardRead` permission gate, and a
  real pipe carrying the payload byte-exact (EOF only after every
  write end closes — the pipe contract demonstrated end to end).
- `tests/tools_live.rs` — the Phase 18 exit criterion as CI: every
  tool runs against the Phase 10 compositor as *compiled binaries*
  (`CARGO_BIN_EXE_*`) over the real abstract socket of an in-process
  compositor, each with its own fresh instance; a seventh scenario
  walks all six tools against a dead socket and requires the
  fast-fail with the failure named.

### Fixed
- **The compositor's abstract-socket bind doubled the namespace
  marker.** `Compositor::headless` prefixed the configured name with
  `\0` while `UnixAddr::fill_sockaddr` already writes the abstract
  NUL — the live name was `\0<name>`, so no external connector
  (every tool, every future client) could ever dial in; the Phase 10
  suites never noticed because they shared the same doubled address
  object. Found the moment `ldp-info --live` first tried a string
  round-trip; the bind name is now exactly the configured name.
- **`ToolSession::setup_surface` under-sized the pool** — one
  buffer's bytes for an N-buffer cycle, so every ping-pong attach
  past the first buffer was an `InvalidBuffer` span overflow at the
  server. The pool now spans every buffer.
- `ldp-profiler`'s replay wall-latency match arm compared a
  variable with itself (`*ts <= *ts`, always true — a shadowing
  accident); it now compares the commit's arrival against the
  presentation timestamp it pairs with.
- The input-debug device-class inference overwrote the
  has-rel-motion flag on every REL event (a trailing wheel event
  cleared it — "mouse=false"); the flag accumulates.
- The audit-log parser never skipped the JSON object's opening
  brace (every line failed as `Malformed` at line 1), and the
  malformed-line test fixture edited a field present on both lines
  (misattributing the damage); the parser handles the braces and the
  fixture breaks line 2's unique seq.
- README's status list had the Phase 17 entry spliced into the
  middle of Phase 16's (16's title line was lost in the Phase 17
  edit); the entries are back in order with their test counts.

### Workspace
- `examples/*` joined the workspace members (the three example
  crates build under the same lints and gates as everything else).
- 1,537 workspace tests green (+71 over Phase 17): 57 in `ldp-tools`
  (unit: args/session shapes, error bridges, audit-log tamper
  corpora, evdev analysis), 7 integration binaries-over-socket
  scenarios, and 7 example scenarios (2 hello, 2 paint, 3 clip).

## [0.1.0-phase17] — 2026-09-27 — Compatibility bridges

### Added
- `crates/ldp-x11-bridge` v0.1.0 — the X11 compatibility server subset
  (pure `ldp-core`/`ldp-protocol` runtime dependencies, `ldp-security`
  dev-dependency for token conformance, `#![forbid(unsafe_code)]`):
  - `wire`: byte-order negotiation, request framing with the
    BIG-REQUESTS escape, the 32-byte reply/error/event envelopes.
  - `setup`: the handshake both directions; the one-screen
    one-TrueColor-visual setup reply (the exact shape of LDP's
    XRGB8888).
  - `events`: every core event the subset emits (input with crossing
    details, exposure, structure, property, selection, ClientMessage).
  - `window`: the window tree — geometry, stacking, exact visibility
    regions, before/after structural diffs, gravity-retained backing
    stores.
  - `render`/`shape`: the software rasterizer (all 16 GX functions,
    plane masks, clip lists, damage) with arc/polygon rasterization.
  - `gc`/`property`/`atoms`: GC state with the value-list codec
    (4-byte-slotted fields), the ICCCM property store, the atom
    registry.
  - `dispatch`/`ops`/`draw_ops`: the multi-client connection state
    machine and the full core opcode table — inert belt answered
    honestly (BadImplementation for the out-of-subset requests),
    colormaps (TrueColor read-only), cursors, the MIT-SHM minors over
    the `ShmHost` seam.
  - `driver`: the rootful LDP proxy — bridge-token gate through the
    `TokenCheck` seam, pool export through `DriverHost`, frame-tick
    commits with exact damage, resize, close via the ICCCM
    WM_DELETE_WINDOW dance, and the evdev→X keycode translation.
  - EC suites: the xeyes and xclock lifecycles byte-for-byte with
    pixel assertions; BIG-REQ + SHM transfer corpus including the
    out-of-bounds paths; the full error-code corpus; the driver's
    bootstrap message stream, resize burst, input routing, and token
    conformance (real Phase 16 grant machinery, 32-case forgery
    corpus, cross-app denial, revocation).
- `crates/ldp-wayland-bridge` v0.1.0 — the Wayland compatibility
  compositor subset (same dependency doctrine):
  - `wire`: the 24-bit object id + 8-bit opcode header, NUL-padded
    strings, length-prefixed arrays, ancillary fd slots, and the
    checked incremental decoder.
  - `protocol`: the pinned interface tables — wl_display, registry,
    callback, compositor, shm/pool/buffer, surface, region, the seat
    family (frame-grouping dialect), and the whole xdg-shell family.
  - `state`: surface double-buffering, the xdg role machines
    (configure/ack handshake, unconfigured-buffer rule), positioner
    vocabulary that translates one-to-one onto LDP's own placement
    types, buffers and formats.
  - `dispatch`: the per-client connection state machine — registry
    replay, bind validation, id lifecycles with delete_id
    confirmation, the full request routing, fatal-error discipline,
    and the input/shell injection seams (serials, keymap send-once,
    pointer frame groups).
  - `driver`: the LDP proxy — token gate, per-committed-surface
    export (surface + pool + buffer + toplevel), commit-time pool
    reads (the implicit→explicit sync barrier), configure/close
    translation, input routing onto the foreign client's devices, and
    the total positioner→popup argument mapping.
  - EC suites: the weston-terminal lifecycle (connect, map, keymap,
    type with strictly-advancing serials, resize through the
    ack/recommit dance, close); the wire golden bytes and malformed
    corpus; the popup lifecycle with the full constraint translation
    table; the serial discipline; the driver's export stream,
    configure/close translation, input routing, and token conformance;
    and the core architectural lint (no Wayland/X11 vocabulary in any
    core crate source or manifest, no core→bridge dependencies).

## [0.1.0-phase16] — 2026-09-26 — Security, a11y, power, session

### Added
- `crates/ldp-security` v0.1.0 — the capability-security layer (pure
  `ldp-core` runtime dependency, `ldp-protocol` dev-dependency for the
  conformance corpus, `#![forbid(unsafe_code)]`):
  - `sha256`: SHA-256 in pure Rust (FIPS 180-4) — incremental and
    one-shot, NIST vectors plus boundary cases cross-verified against
    Python's hashlib (55/56/57/63/64/65-byte messages at the padding
    edges), the audit-chain and manifest-hash primitive.
  - `manifest`: app manifests — app_id charset validation at the byte
    level, requested `ScopeSet`, sandbox flavor, the canonical byte
    form, and the 32-byte manifest hash every audit record quotes.
  - `matrix`: the threat-model §3 permission table as executable
    policy — 18 `Operation` rows over five requirement classes
    (Free / Manifest / ManifestAndPrompt / Prompt / Impossible),
    `decide()` answering Allow / Prompt / Deny from (manifest,
    effective) scope sets, and `manifest_baselines()` deriving the
    connect-time baseline (manifest-class scopes only; prompt-class
    and prompt-only scopes never baseline).
  - `grant`: the broker's grant table — token minting through the
    `TokenSeed` entropy seam (deterministic splitmix64 for corpora),
    collision re-draw armor, submission validation that walks the
    whole table with constant-time byte compares: forgery and
    cross-app replay deny with `invalid_token` (the audit trail still
    learns the stolen scope), expiry is boundary-exact (`now ==
    expires` valid, one nanosecond later not), revocation kills
    resubmission, same-app resubmission (reconnect) is legal and
    idempotent; `effective_scopes` unions the connection's live
    submitted tokens.
  - `broker`: the session broker — the only component allowed to make
    grant decisions. Escalation: manifest gate (prompt-only scopes
    exempt — the administrator flow), sliding-window rate limiter
    (audited denials), the `PromptDecider` prompt seam for the
    sensitive scopes, TTL'd or session-bound minting. Manifest
    registry, revocation with live-reporting, capture/inject/bridge
    audit records, `purge_app`.
  - `chain`: the hash-chained audit log — canonical per-record bytes,
    SHA-256 chaining from the genesis/window anchor, bounded ring
    (evicting sets the anchor to the evicted digest so the retained
    window re-verifies), `verify()` reporting the first break,
    five-class subscription filtering (bridge folds into captures —
    documented doctrine), JSONL lines with hand-rolled escaping, and
    `from_records` — the persisted-store reader path (the Phase 18
    `ldp-audit` tool) that also underlies the tamper corpus.
- `crates/ldp-accessibility` v0.1.0 — the accessibility layer (pure
  `ldp-core`, `#![forbid(unsafe_code)]`):
  - `settings`: the feature-toggle model over the exact `a11y_settings`
    wire bits (bit 2 unassigned by spec), Q8 text scale and cursor size
    ranges, snapshot diffing, and the `SettingsBus` broadcast with
    coalescing bounded queues (late binders see the current snapshot).
  - `bus`: the provider/subscriber event bus — `a11y_control`-gated
    subscriptions (deny-by-default), global-sequence stamping at push,
    dispatch as a stable merge (per-provider FIFO preserved exactly),
    push-time focus attribution (later focus changes never rewrite
    earlier announcements), and bounded per-provider queues that drop
    the oldest and count (a flooding provider cannot block the bus).
  - `magnifier`: lens/content geometry — Q8 scale (1x–16x, 0 = the
    wire's keep sentinel), follow selectivity (unfollowed sources
    never move the lens), lens clamped inside the output, zoom steps
    (×4/3 / ×3/4) with clamping.
- `crates/ldp-power` v0.1.0 — the power layer (pure `ldp-core`,
  `ldp-compositor` dev-dependency for the re-anchoring criterion,
  `#![forbid(unsafe_code)]`):
  - `idle`: the active→dimmed→off→suspend ladder with logind delay
    semantics — `blur`/`display`/`idle` inhibitors hold their
    transitions, release fires overdue ones at the next tick, activity
    resets with wake events, long gaps cascade; `set_idle_timeout`
    retargets the ladder live.
  - `backlight`: the flicker-free ramp — 16 steps at 20 ms, monotone
    convergence landing exactly on target, mid-ramp retargets continue
    from the current level.
  - `suspend`: the sleep/wake pair with the wire's sleep kinds,
    suspended-duration reports, and `ReAnchor::grid_align` — the first
    phase-preserving vblank boundary after the wake (no phase only
    when the pre-suspend anchor is unknown or the wake precedes it).
- `crates/ldp-session` v0.1.0 — the session layer (pure `ldp-core`,
  `ldp-power` dev-dependency for the inhibit-bit conformance,
  `#![forbid(unsafe_code)]`):
  - `dbus`: the D-Bus wire codec from the specification — message
    framing, the header `a(yv)` with fields 1–9, basics + variant +
    array + struct + dict marshaling with the full alignment
    discipline, and bounds-checked parsing (typed errors, never
    panics). Golden bytes pin the canonical Hello and TakeControl
    forms.
  - `conn`: the connection state machine over the `DbusTransport`
    seam — serial allocation, reply matching, signal routing, the
    EXTERNAL auth preamble.
  - `logind`: the session machine — TakeControl, TakeDevice (fd +
    inactive; the lease table tracks fds across pauses), PauseDevice
    (pause/force/gone; ack owed only for pause), ResumeDevice,
    Lock/Unlock, PrepareForSleep; error replies and malformed signals
    surface typed without derailing the machine.
  - `vt`: VT-switch choreography — pause → release DRM master (+ ack
    for pause-reason) → inactive → resume → reacquire device →
    active; stray events ignored by state.
  - `lock`: lock-screen orchestration — Locking (surface-mapping
    window, optional force-blank deadline for broken lockers) → Locked
    (the focus gate admits only lock surfaces) → Unlocked; only the
    lock surface may unlock (other requests refused); the
    administrative Unlock signal tears down from outside.
  - `inhibit`: the cookie registry — per-connection cookies, release
    by cookie or connection teardown, the union effective mask; wire
    bits pinned against the power layer's independent definition.

### Exit criteria
- Permission matrix: the exhaustive 18-row × 3-state walk against a
  hand-transcribed literal table, plus prompt-class doctrine agreement
  with `ldp-core`'s `requires_prompt`, wire-value conformance for every
  local enum/bitset against the compiled schema, and the
  every-`ldp.security`-operation typed-handle coverage table.
- Token forgery/replay: 2,000-mint distinctness corpus, 2,000-mutation
  forgery corpus (single-word XOR masks plus structural probes — every
  one denies scope-attribution-free), cross-app replay with stolen-scope
  attribution, revoked resubmission, expiry lattice (boundary-exact
  per token), broker TTL landing on the same boundary, revocation and
  purge audit trails in order.
- Audit chain: verification through 1,000 appends with exact counts;
  the tamper corpus (every scalar and string field of every record,
  adjacent swaps, tail truncations, forged appends, digest
  substitutions — all fail); ring windows staying verifiable across
  evictions; the full broker session landing in the chain in order
  with exact filtering; JSONL structural round-trips.
- A11y event ordering: the 5,000-event LCG corpus across 3 providers
  with interleaved dispatch points — global-seq totality, per-provider
  FIFO against an independently maintained push log, no duplication,
  push-time attribution equality, delivered+dropped conservation; the
  scope-gate matrix; identical multi-subscriber fan-out; magnifier
  bounds/zoom/follow property corpora.
- Suspend/resume re-anchoring: at 144/90/60/40 Hz × sleep depths —
  the re-anchor boundary sits exactly on the pre-suspend grid and
  strictly after the wake; post-resume predictions from the real
  `FrameClock` land exactly one nominal interval past the re-anchor,
  chain-spaced at exactly nominal, never in the past; the
  stale-timeline teeth check proves re-anchoring changes the grid.
- D-Bus/session: golden Hello/TakeControl bytes, every-type
  round-trips, multi-value bodies, the malformed corpus (typed errors
  at the right stage, full-prefix truncation sweep, path validation);
  the scripted full session lifecycle end-to-end in marshaled bytes
  (TakeControl → TakeDevice → VT away/back with the ack → Lock/Unlock
  → sleep); the lock focus-gate matrix; the inhibitor registry
  semantics; the cross-crate inhibit-bit conformance and the
  registry→idle-machine end-to-end drive.

1,328 workspace tests green (unit + integration + drift gates);
fmt, clippy `-D warnings`, rustdoc `-D warnings`, spec lint, and the
`ldpc` drift gate all pass.

## [0.1.0-phase15] — 2026-09-26 — VRR

### Added
- `crates/ldp-vrr` v0.1.0 — the adaptive-sync policy engine (pure
  `ldp-core` + `ldp-compositor` runtime dependencies, `ldp-display`
  dev-dependency for the panel-model integration suites,
  `#![forbid(unsafe_code)]`):
  - `caps`: the output's adaptive-sync support — the panel window
    `[min, max]` as validated periods (zero/inverted rejected,
    millihertz wire conversions with the rate↔period inversion
    handled), the `vrr_caps` bitset (seamless, fixed_rate) with
    round-tripping wire bits, and `OutputVrrSupport` enforcing the
    kernel's own structural invariant: the nominal mode period must
    sit inside the window, otherwise the mode cannot run VRR.
  - `policy`: the decision table — one pure function over
    policy (off / deadline / always) × support × adaptive demand ×
    battery saver, each row carrying a machine-checkable rationale
    (`PolicyOff` / `Unsupported` / `NoDemand` / `BatterySaver` /
    `DeadlineWindow` / `AlwaysOn`). Deadline installs the widening
    `max − nominal` (exactly the stretch a late commit may consume);
    always installs the full `max − min` span (commit-at-ready); off
    installs zero. `vrr_policy` wire values 1/2/3 round-trip.
  - `window`: the window arithmetic — the scheduler-facing bounds,
    the deadline-widening math (saturating on degenerate panels), and
    `select_flip`, the **flip-slip avoidance** rule: a flip lands at
    `clamp(max(ready, last + min), last + min, last + max)` — never
    closer than the minimum refresh interval to the previous scanout,
    never past the stretch ceiling; only a readiness past the whole
    window misses.
  - `refresh`: the `RefreshSelector` — the per-output latency
    optimizer answering "content is ready at `t`" with a ruling:
    `FlipAt` (a legal window slot, sequencing off the one outstanding
    flip like the KMS queue), `Defer` (the fixed-rate fallback:
    below-minimum-rate frames under deadline policy deferred onto the
    nominal grid with a retry hint — the path that emits the reserved
    `throttled` drop reason), or `Late` (no fixed-rate fallback: the
    catch-up flip at readiness). VRR-off answers nominal-grid rulings
    with the mock timeline's arithmetic.
  - `tearing`: the tearing gate — a three-way conjunction (immediate
    mode + async-flip capability + session permission) that does not
    read VRR state at all; the rationale ladder
    (NotRequested / Unsupported / Forbidden / OptIn) is the audit
    trail. `mode_is_tear_free` pins the structural tear-freedom of
    vsync/adaptive presentations.
  - `engine`: the `VrrEngine` — the stateful per-output composition.
    Policy/battery/demand changes re-run the table and, when the
    effective KMS state changes, emit `WindowApplied` (program
    `VRR_ENABLED` and the window through the next atomic commit; the
    initial state is queued at construction); deferrals emit
    `Throttled` with the retry grid point. `scheduler_config()`
    returns the base `SchedulerConfig` with `vrr_window_ns` installed
    from the live decision, so the deadline scheduler and the panel
    agree on the same window. Timeline inputs are timestamped and
    asserted non-decreasing, mirroring the scheduler's driver
    contract; `commit_ready` drives the selector per (surface,
    frame).
- `ldp-display`: `CommitFlags::PAGE_FLIP_ASYNC` (the kernel's
  `0x02` async-flip bit — the tearing opt-in path on real hardware)
  with `is_async_flip()`, plus `BitOr`/`BitOrAssign` for flag
  composition. The ldp-vrr tearing gate is the only authority on
  whether the flag may be set.
- Integration suites (`crates/ldp-vrr/tests/`):
  - `policy_table.rs` (exit criterion 1): the exhaustive 36-row walk
    of the decision table against a hand-derived literal table —
    enablement, window presence, widening to the nanosecond, and
    rationale for every combination; the wire vocabularies
    (`vrr_policy` values, `vrr_caps` bits, millihertz windows)
    round-trip; the rationale set is proven exhaustively reachable.
  - `golden_timelines.rs` (exit criterion 2): selector timelines at
    144/90/60/40 Hz cadences (floor clamping, at-ready flips,
    fixed-rate deferrals with grid retry hints, catch-up without the
    fallback) plus the flip-slip invariants over a 64-case randomized
    LCG corpus; scheduler+window goldens — the window-late save (a
    commit past the plain deadline but inside `max − nominal` still
    presents, tear-free, at its own stretched flip), the
    one-nanosecond widening boundary, always-policy full-span
    widening, off-policy byte-identical pass-through.
  - `panel_integration.rs` (exit criterion 2, the mock-KMS loop):
    `VRR_ENABLED` programmed from the engine's emission, engine
    rulings matching the device's flip completions to the nanosecond
    at a 90 Hz cadence, the seamless toggle round-trip, and the full
    loop where the scheduler's `presented` feedback lands at the panel
    flip with the measured VRR interval.
  - `tearing_isolation.rs` (exit criterion 3): the gate matrix
    identical under both VRR states, the composition-level blindness
    proof (every gate input against every policy-table outcome), a
    2,000-case randomized corpus, the scheduler-level isolation (the
    immediate path's event vector byte-identical with and without the
    window; adaptive surfaces on a windowed output never torn; a
    mixed output tearing exactly its immediate surface), and the KMS
    seam (the async-flip flag riding exactly on the allowance, an
    async-flagged flip committing through the mock device).
- 70 new tests (37 unit + 33 integration); workspace total 1,198.
- `docs/architecture.md` §13 rewritten as-built; `docs/roadmap.md`
  Phase 15 marked delivered; README status updated (Phase 15 of 20).

## [0.1.0-phase14] — 2026-09-26 — Color & HDR

### Added
- `crates/ldp-color` v0.1.0 — the canonical color math (pure
  `ldp-core` dependency, `#![forbid(unsafe_code)]`):
  - `matrix`: row-major 3×3 algebra (apply/mul/transpose/invert via the
    adjugate with singularity rejection) and the chromaticity
    derivation — primary direction vectors as columns, Cramer-scaled to
    the white point — producing the RGB↔XYZ matrices for BT.709/sRGB,
    DCI-P3 and BT.2020, the RGB↔RGB conversions (exact identity fast
    path, published 4-dp ITU anchors) and the luma coefficient rows;
    plus the Bradford cone-response matrix and its D65↔D50 adaptation
    (the full von Kries sandwich `B⁻¹·diag·B` — the famous published
    matrix is the D50→D65 direction, hand-verified by its exact
    D50→D65 white mapping, and the module's test pins that).
  - `transfer`: the six wire transfer curves as f32 encode/decode
    pairs — bit-identical to the renderer's Phase 8 hook (constants,
    branch order, clamps; a cross-consistency integration suite pins
    the two equal forever) — and the f64 PQ reference
    (`pq_decode_f64`/`pq_encode_f64`, constants in the spec's rational
    spellings) that every integer table and the BT.2390 EETF evaluate
    through. Documented boundary: PQ's inverse EOTF maps luminance 0
    to the black signal `C1^M2 ≈ 7.309559e-7` (the curve's one
    non-bijective point) — a published constant, pinned in the KAVT
    suite.
  - `ramp`: the exact integer PQ ramps (8..=16 bits, 10/12-bit the
    v1 formats): f64-built decode tables cast once, nearest-code
    binary-search encode with the ties-to-lower rule, and the
    zero-drift property `encode(decode(code)) == code` for every code
    — the architecture's "PQ encodes via the exact integer ramps"
    rule. The f32 curve pair's intrinsic noise (the C2−C3·vp
    cancellation near the top, ~2e-5 amplified) is documented: the
    table is the authority, the curve the approximation.
  - `gamut`: luma-preserving desaturation on the constant-luma line
    through each color's own achromatic point. Two modes: hard `Clip`
    (in-gamut colors bit-identical — the identity fast-path doctrine;
    out-of-gamut lands exactly on the analytic boundary point) and
    `SoftKnee` (chroma-ratio compression through a smoothstep
    shoulder: untouched below the knee, continuous at the knee, the
    gamut edge and the ceiling, monotone, always in-gamut — the ICC
    perceptual-intent trade documented). Properties: exact luma
    preservation, chroma monotonicity along luma-neutral rays,
    super-white/black collapse.
  - `tone`: the BT.2390-structured EETF — PQ-domain normalization of
    master and display bounds, knee at 75% of the display range,
    luminance-exact identity below the knee, master peak landing
    exactly on display peak, and a provably-monotone rolloff: a C¹
    cubic Hermite (slope 1 at the knee, 0 at the peak) when the
    compression ratio permits (Fritsch–Carlson `r ≥ 1/3`), a
    smoothstep branch otherwise (beyond 3:1 in PQ-normalized terms any
    slope-1-at-knee cubic is non-monotone — the derivation is in the
    branch comments). `map_luminance` implements the BT.2390 input
    clip with the documented sub-mastering-black pass-through; the
    per-pixel `luminance_scale` is bounded and never infinite.
    `HdrToHdr` is the HDR→HDR peak-clip path. The 1000→100-nit
    canonical case pins the 203-nit reference white at ≈87.6 nits.
  - `icc` + `icc_parse` + `icc_trc`: ICC v2/v4 matrix-TRC display
    profile import — full header validation (size word, version 2..=4,
    `mntr`/`RGB `/`XYZ `/`acsp`, the D50 header illuminant), tag-table
    validation (bounds, duplicates, data-after-table), typed payload
    readers (XYZType, curveType identity/gamma/table with
    strict-monotonicity validation, parametricCurve types 0–4 with
    the sRGB-proven branch orientation — the power branch covers
    X ≥ d — and full piecewise inverses, textDescriptionType/`text`/
    `mluc` records, `sf32` chad), the f64-exact model, the
    `chad`-exact (v4) or Bradford (v2) D50→D65 de-adaptation, and the
    resolution to the closest parametric description (primaries by
    max chromaticity distance over primaries + white; transfer by
    curve error over the SDR candidates — PQ/HLG excluded as
    absolute/scene-referred, documented).
  - `icc_write`: the canonical deterministic writer — fixed tag order,
    4-aligned data, zeroed date/ID, canonical version words, v2
    ASCII/`text` vs v4 `mluc`/`paraType` selection with typed
    rejections for unrepresentable models. The round-trip engine:
    `parse(write(m)) == m` and `write(parse(write(m)))` byte-identical
    for wire-grid models (s15Fixed16/u8Fixed8 quantization rules
    documented — the writer rounds, never truncates).
  - `pipeline`: `ColorSpace` (parametric or ICC-imported: decode →
    linear → XYZ, linear↔linear conversions, luma) and
    `OutputTransform`, the per-output tail — tone-map scale (EETF for
    SDR targets with HDR mastering, peak clip for PQ targets) →
    reference-white normalization → gamut map → transfer encode, over
    absolute-nits inputs with the SDR-anchoring seam owned by
    `ldp-hdr` (documented, never a hidden constant). Studio-range
    targets are typed errors (v1 outputs are full-range).
- `crates/ldp-hdr` v0.1.0 — the HDR policy layer (depends on
  `ldp-core` + `ldp-color` only, `#![forbid(unsafe_code)]`):
  - `metadata`: HDR10 static-metadata validation and normalization —
    the CTA 0-unknown sentinels preserved, mastering bounds ordered
    and under the PQ ceiling, FALL normalized to CLL, the InfoFrame
    integer codings (1e-4 cd/m² min, whole-nits max/CLL/FALL) with
    the documented u16 saturation window.
  - `infoframe`: the CTA-861.3 HDMI HDR Static Metadata InfoFrame —
    the full 30-byte packet (type 0x87, version 1, length 26, EOTF +
    descriptor nibbles, reserved byte, eight 0.16-fixed chromaticity
    codes, the four luminance fields, checksum zeroing the packet),
    hand-computed BT.2020/D65 chromaticity byte vectors, parse-back
    with every header field validated, and typed corruption rejections.
  - `adaptation`: the SDR-in-HDR canvas (`LuminanceAdapter`, BT.2408
    203-nit default) — monotone in input and canvas, exact identity
    anchors, invertible in-domain — and BT.2100's HLG OOTF system
    gamma `1.2 + 0.42·log₁₀(Lw/1000)` with the published anchors
    (1000→1.2, 2000→1.3264, 400→1.0329, 100→0.78) and the
    peak-dependent mid-tone ordering.
  - `policy`: the output-mode decision table (pure `decide`:
    no-HDR→SDR, PQ content→PQ-else-HLG, HLG content→HLG-else-PQ,
    all-SDR→SDR; PQ wins over HLG as the interchange EOTF) and the
    `ModeController` dwell hysteresis — a target mode must hold for
    `dwell` consecutive evaluations before the output switches, so
    lone HDR popups cannot flap the panel mode; deterministic by
    construction and fuzz-verified (no switch inside a dwell window
    over 2000 randomized steps).
- Exit criteria (tests/): `kavt.rs` — every formula against an
  independently transcribed f64 reference (ST 2084 with the spec's
  printed decimal constants, BT.2100's a/b/c via exp/ln, the sRGB
  closed forms) plus the published anchors: PQ 100/203/1000/4000
  nits, the black signal, the 10-bit code spots (512→≈92.2 nits,
  917→≈3776 nits), sRGB 0.5↔0.7353570/0.2140411, HLG 0.75→0.264898,
  the full-precision published matrices and both luma rows, and the
  BT.2390 structural anchors (knee luminance, identity bracket,
  203-nit window). `icc_roundtrip.rs` — three fixtures (sRGB-class
  v2 table, P3-class v4 parametric+chad, gamma-2.2 v2) built on the
  wire grids through the Bradford-adapted standard primaries: exact
  model round-trips, byte-identical canonical re-serialization,
  resolution fitting the right descriptions, native matrices
  recovering the standard primaries, corruption rejections at every
  truncation point and field, and the writer's unrepresentable-model
  errors (including v4 mluc accepting non-ASCII, round-tripped).
  `renderer_consistency.rs` — the Phase 8 hook and the canonical
  curves/matrices pinned equal. `pipeline_props.rs` — randomized
  (LCG) output configurations: monotone/bounded/deterministic tails
  with per-target endpoints, white preservation across every space
  conversion, and the ramp-composed HDR path.
  `ldp-hdr/tests/properties.rs` — the adaptation-monotonicity EC over
  randomized corpora, the full SDR-ramp→PQ-panel chain (anchored
  exactly at the canvas, brighter canvases lift), policy determinism
  + no-flap over 2000 randomized steps, the metadata→InfoFrame
  plumbing round-trip, and one coherent end-to-end configuration
  story.

### Changed
- `docs/architecture.md` §12: the as-built refinement (the
  cross-consistency pin, the Fritsch–Carlson branch rule, the
  chad-exact de-adaptation, the canvas seam, the InfoFrame byte map).
- `docs/roadmap.md`: Phase 14 marked delivered; README status updated
  (Phase 14 of 20).

## [0.1.0-phase13] — 2026-09-25 — Clipboard & DnD

### Added
- `crates/ldp-clipboard` v0.1.0 — the data exchange layer (pure
  policy over ldp-core + the compiled schema, plus one audited libc
  seam):
  - `mime`: RFC 6838-subset validation before any storage (the
    validation-before-allocation doctrine), canonical form
    (lowercased tokens, sorted parameters, quoted values only when
    needed), `text/plain` charset folding — the ASCII-compatible
    family (`utf-8`/`utf8`/`us-ascii`/`ansi_x3.4-1968`) compares
    equal, other encodings stay distinct — and preference-ordered
    negotiation returning the offered string a receiver must send.
  - `source`: the `data_source` machine — bounded offer-list
    accumulation (duplicates are no-ops, floods are typed errors),
    the attach/cancel/finish lifecycle, send accounting, and target /
    action bookkeeping for the drag-side UI.
  - `offer`: the `data_offer` machine — accept/receive validation
    (offered-MIME check with folding; one receive per MIME per offer),
    drag-only gating of `set_actions`/`finish`, and a
    check-then-commit split so manager-level admission can interleave
    without leaving traces on rejection.
  - `device`: the per-seat slot model — clipboard and primary as one
    mechanism with two names, serial-bound setting (the popup-grab
    precedent for the integrator-supplied record), owner-only null
    clears, and teardown cascades (source gone, client gone).
  - `dnd`: the drag FSM and the locked action-negotiation semantics —
    receiver narrows via `set_actions`; the server re-announces to
    the source; at drop the compositor picks copy > move > ask from
    the narrowed set and announces the singleton (the source's last
    `actions` event is the negotiated action — `dnd_finished` carries
    no argument); a declined (empty) set cancels the drag; leave
    resets narrowing so no stale narrowing leaks between receivers;
    `ask` resolves through the integrator's user prompt.
  - `transfer`: the stateful page-bounded pump (one 4096-byte page
    per pump; a would-blocking sink retains its pending tail — the
    fuzz-proven failure mode of a stateless step), the transfer
    registry enforcing `Limits::client_fds` at admission with
    exactly-once release, terminal states carrying the authoritative
    byte count, and cancel-by-client/source/offer sweeps.
  - `pipe`: the audited libc seam — `pipe2(O_CLOEXEC|O_NONBLOCK)`,
    blocking-mode switching, raw read/write with EINTR/EAGAIN
    handling, std-io bridges so the blanket pump traits apply; every
    `unsafe` block carries a SAFETY note (the `ldp-input` sys
    precedent; every other module is `#![forbid(unsafe_code)]`).
  - `permission`: deny-by-default gates — clipboard/primary *read*
    needs the `clipboard_read` scope (manifest baseline or escalated
    token), *setting* the selection needs no scope (ownership is not
    a read), DnD drop receives are user-intent authorized; every
    decision renders a JSONL audit line.
  - `manager` + `drag`: the cross-client coordinator — every protocol
    request evaluates to a routed event batch (client, object, typed
    event, optional FD); selection publication to every non-owner
    device client; one offer per drag reused across enter/leave; the
    shared cancel funnel; gate → validate → admit → `send` receive
    ordering (scope checks before argument validation, architecture
    §16.3); object ids supplied by the integrator's store.
  - `event`: the typed `ldp.data` vocabulary — all 16 events,
    encode→decode→strict-signature round-trips against the compiled
    registry, FD-count agreement for `send`.
- Integration suites (the Phase 13 exit criteria):
  - `conformance.rs` — schema coverage in both directions; golden
    lifecycles (selection publish/accept/receive, eviction,
    owner-destroy nulls, the full drag happy path, offer reuse,
    primary as a unified slot); error conformance (bad serial,
    foreign/unknown sources, late offers, unoffered/duplicate
    receives, gate denials, FD-budget exhaustion).
  - `transfer_fuzz.rs` — 2,000-case fuzz over random MIME/size/pattern
    with adversarial halves (short reads/writes, 30% stall rates,
    mid-stream failures): bounded step budget (never deadlocks),
    byte-exactness via streaming checksums, plus the 5,000-step
    registry-ledger fuzz returning to exactly zero.
  - `stream_100mb.rs` — the 100 MB stream through real blocking pipes
    under default limits: on-the-fly pattern source, reader thread
    with in-situ pattern verification, streaming checksums on both
    sides, one-page working set, FD-table leak assertion; four
    concurrent 8 MB streams sharing a tightened budget round-robin.
  - `dnd_fsm.rs` — scripted cancel paths, surface-death semantics,
    ask resolution, wrong-client rejection, offer reuse across
    receivers, seeded determinism, and the 30k-step two-seat
    transition fuzz with invariants after every step.
  - `permission.rs` — the full gate matrix through the manager:
    deny-by-default with no trace (denials leave no transfer, no
    send, and do not consume the receive slot), manifest and token
    bases, wrong-scope tokens, primary gated identically, unscoped
    setting, drop receives without any scope, and client-teardown
    cleanup.
- Workspace test count: 1,048 (was 955).

## [0.1.0-phase12] — 2026-09-25 — Shell

### Added
- `crates/ldp-shell` v0.1.0 — the shell layer (pure policy over
  ldp-core + the compiled schema; no clock, no I/O, no unsafe):
  - `serial`: the configure serial clock — wrapping-monotonic with
    reservation skipping (a live proposal's serial is never reissued),
    wrapping-order `Serial::after` semantics, and `resume_from` for
    crash-recovery watermarking (post-restart serials can never
    collide with pre-crash acks still in flight).
  - `toplevel`: the state machine — six spec state flags as intents
    (minimize clears activation and vice versa), bounded title/app-id
    storage, and the exact two-phase commit: one live proposal, a new
    `configure` supersedes the previous one (its serial dies), an ack
    must reference the live serial (stale acks are typed errors), the
    next commit realizes exactly the acked proposal. Geometry derives
    from policy inputs (fullscreen = whole output with zero insets;
    maximized = workspace area minus SSD insets; both clamped to the
    client's size hints, which saturate on contradictory pairs so the
    hint pair is consistent by construction).
  - `popup`: anchor/gravity placement per the LDP spec text (gravity
    is the direction the popup *grows* from the anchor point — the
    edge opposite the growth vector sits at the anchor) and the
    constraint pipeline: slide (toward the usable area, capped at the
    anchor rect's opposite edge — never detaches), flip (anchor +
    gravity mirrored on the still-offending axis, accepted only when
    that axis then fits), resize (extent clamp toward the overflowing
    edge; left/top overflow cannot be resized away and stands
    honestly). Reposition, client/server dismissal, and explicit
    pointer grabs with press-serial validation.
  - `dialog`: transient windows with modality (`gates` only the
    parent's tree, cleared on close), workspace-clamped size
    proposals, and the centering policy (over the parent's content
    area, clamped inside the workspace).
  - `ssd`: decoration metrics in logical units with per-output
    ceil-scaling (never under-cover, < 1 px error) — SSD insets are
    border + title bar, CSD insets are the system hit-zone
    reservation; frame/content geometry round-trips exactly; the
    frame margin is the larger of shadow and hit zone.
  - `spaces`: macOS-style spaces — per-seat active space, sticky-or-
    assigned visibility, clamped moves, and count-shrink reflow that
    reports every moved window (deterministically sorted).
  - `stack`: the window-stack policy layer — per-space back-to-front
    order, dialogs filed above their parents, modal gating on focus,
    MRU activation with attribution (the cause of every focus change
    is recorded in a bounded log for a11y/audit), focus fall-back on
    dialog close, parent-death dialog cleanup, and deterministic
    BTreeMap-ordered reflow.
  - `event`: the typed `ShellEvent` vocabulary (all 8 module events)
    encoded against the compiled registry — opcode resolution,
    bitset states, nullable output refs.
- Integration suites (`crates/ldp-shell/tests/`): the schema
  conformance oracle (every request and event of `spec/shell.toml`
  has a typed handle — verified in *both* directions against the
  compiled schema, so neither the spec nor the crate can drift);
  golden lifecycles (toplevel map→configure→ack→commit→maximize→
  fullscreen→close; popup placement→reposition→dismiss; dialog
  open→ack→close; spaces moves and reflow) whose every emitted
  message passes encode → decode → strict signature check and
  byte-stable re-encoding; the 20k-step configure fuzz (seeded
  xorshift64*, adversarial ack/commit/supersede sequences across a
  six-window fleet, invariants asserted after every step, the
  wedge property — a fresh propose always succeeds and a live ack
  always honors); popup-solver property suites against an independent
  reference model (containment, slide attachment, permission
  honesty, purity, no-op on fitting placements); the mixed-DPI SSD
  suite (scales 1×–3×, output migration re-proposes with fresh
  serials, fuzzed Q8.8 scale table); and the spaces × stacking ×
  focus integration suite (sticky visibility, workspace-switch focus
  restoration, modal gating, 30k-step focus churn with invariant
  checks).

### Fixed
- None (new crate); the fuzz driver itself caught and forced two
  design hardenings before release: contradictory size hints now
  saturate instead of storing an inconsistent pair, and the
  testbench PRNG is xorshift64* rather than an LCG (an LCG's bit 0
  alternates, which froze a `flip()` driver at one value under a
  regular call pattern — the conformance suite caught it).

## [0.1.0-phase11] — 2026-09-25 — Input & seats

### Added
- `crates/ldp-input` v0.1.0 — the input backend:
  - `evdev` wire codec: 24-byte `input_event` records (LP64 timeval
    folding, negative-second clamping), partial-record-tolerant stream
    decoder, `SYN_REPORT` framing with `SYN_DROPPED` resync and
    protocol-A flagging.
  - `codes`: the documented evdev ABI constant subset (types, SYN/REL/
    KEY/BTN/ABS/MSC/LED codes, bus types), pinned by tests.
  - `device`: `DeviceSpec`/`AbsInfo`/`DeviceId` with hint-first class
    classification, capability bit queries, `LedState` writeback image.
  - `mt`: the kernel protocol-B slot machine — per-slot state, tracking
    IDs with reassignment protection, drop resets, full-state diffs.
  - `normalizer`: device units die here — normalized touch contacts
    (0..1 + mm via axis resolution), fuzz dead-zoned tablet pressure,
    tilt hundredths-of-degree → radians, wheel detents + hi-res counts,
    key/button split at the `BTN_MISC` boundary, kernel autorepeat
    filtered (repeat is ours), unknown events surfaced as `Other`.
  - `accel`: the two-tier smooth curve (identity below threshold,
    smoothstep to the plateau, C¹ at both joins), latency-first
    velocity (one event of history), float-delta form for touchpads;
    property tests: monotone, bounded, continuous, zero-slope at the
    tier join, direction-preserving, golden curve points.
  - `gesture`: swipe/pinch/hold state machines over mm-accurate finger
    tracking (uniform ending semantics: lift = clean, add = interrupt,
    drop = cancel), two-finger scroll emulation with spread-threshold
    pinch conversion, single-finger pointer strokes.
  - `repeat`: server-side key repeat schedules (press-anchored ticks,
    prefix-stable windows, bounded coalescing for wake-point driving).
  - `xkb`: `dlopen("libxkbcommon.so.0")` with the full symbol table,
    safe context/keymap/state wrappers (evdev keycodes +8), v1
    serialization (caller-owned strings freed), modifier snapshots,
    keysym queries, sealed-memfd keymap descriptors
    (`MFD_ALLOW_SEALING`, SHRINK|GROW|WRITE, byte-exact read-back);
    state-component constants verified against the 1.7.0 header.
  - `backend`: `/dev/input` enumeration (absence = the honest empty
    list), `O_RDONLY|O_NONBLOCK|O_CLOEXEC` opens, the `EVIOCG*` probe
    (version/id/name/bits/absinfo) with typed `NotEvdev` for
    non-evdev descriptors — exercised against a memfd in CI.
- `crates/ldp-seat` v0.1.0 — the seat layer:
  - `event`: the typed `SeatEvent` vocabulary (all 45 `ldp.input`
    events, spec-exact wire values) with schema-registry opcode
    resolution and argument building — encode-time consistency.
  - `seat`: `SeatManager` — udev-tag device assignment (auto-creating
    seats, duplicate-device refusal), capability masks recomputed per
    change.
  - `focus`: subpixel input regions (`[x, x+w)` semantics), top-first
    hit testing, the scene view (`SurfaceRef`/`ClientBinding`), and
    the pointer/touch focus domains.
  - `grab`: implicit button grabs, explicit popup grabs,
    capability-gated keyboard grabs (the token check is the security
    layer's call), dismissal on surface death.
  - `route`: the per-seat router — acceleration, gestures, keymap
    state, repeat, touch/tablet grabs; finger-scroll millimeters →
    radians through a nominal 25 mm wheel radius; batch terminators
    (`frame` events) follow each family's last target; the full
    golden corpus drives byte-literal evdev traces through decoder →
    framer → normalizer → router and asserts the routed batches
    event-by-event, then re-encodes and round-trips the wire form with
    byte-stability checks.
- Integration suites: `golden.rs` (11 scenarios: mouse enter/motion/
  click/crossing/wheel, keyboard keys + modifiers + autorepeat
  filtering, touchscreen contacts, touchpad single-finger + scroll +
  three-finger swipe, tablet proximity/contact/axes, wire round-trip),
  `multi_seat.rs` (tag routing, interleaved-input isolation, focus
  independence, per-seat keymaps, surface-death isolation),
  `grabs.rs` (implicit pinning across boundaries, multi-button spans,
  wheel-follows-grab, surface-gone release, region-restricted
  surfaces, press/release symmetry).

### Notes
- Safety doctrine unchanged: every module `#![forbid(unsafe_code)]`
  except the audited `sys` boundaries (`xkb::sys`, the backend's
  ioctls) — `SAFETY` per call. Timing doctrine: no clock reads; the
  machines run on the timestamps they are fed.
- Tap-to-click is deliberately absent (physical touchpad buttons
  arrive as `Button` events); the tap policy belongs to the shell
  phase, matching libinput's config-policy split.
- Workspace: 860 tests green (was 757), fmt/clippy(-D warnings)/doc/
  spec-lint/ldpc-drift all clean.

All notable changes to the LDP (Lion Display Protocol) project are documented
here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning is [SemVer](https://semver.org/) per crate, with the *protocol* carrying
its own module versions (see `spec/README.md`).

Releases are cut at phase boundaries defined in `docs/roadmap.md`.

## [0.1.0-phase10] — 2026-09-24 — The vertical slice: lion-compositor

### Added
- `binaries/lion-compositor` — the Phase 10 example compositor, the first
  end-to-end assembly of the whole stack: client window → attach/commit →
  surface tree → damage engine → software renderer → double-buffered
  scanout chain → atomic page flips (mock KMS) → presentation feedback
  (`frame_target`/`presented`/`frame_dropped`) → buffer release fences,
  all served over the real protocol by an `ldp-server` on an abstract
  Unix socket with one `CompositorDispatcher` per session (lib + thin CLI
  binary: headless serve, `--selftest`, `--dump` PPM frames, DRM probe,
  auto fallback).
- **Headless time doctrine:** the mock device's injected clock advances
  only at protocol wake points — `Dispatcher::on_wake` pumps after every
  handled message, rendering pending damage, submitting
  `PAGE_FLIP_EVENT|NONBLOCK` flips, and advancing the clock exactly to
  pending flip landings (never speculatively). The served event stream
  is therefore a pure function of the message sequence: byte-reproducible
  and CI-friendly, with presentation timestamps exactly on the vblank grid
  (16,666,666 ns @ 60 Hz).
- **Event routing:** the pump produces outbox entries routed by
  ownership — the waking client's entries emit directly through its
  `DispatchCtx`, everyone else's park in per-client outboxes drained at
  their next wake (cross-client presentation feedback without
  cross-thread socket access).
- **Buffer exchange:** `shm.create_pool` maps received descriptors
  read-only through the audited `sys` layer (mmap/memfd/eventfd, every
  call `SAFETY`-commented); pools grow by re-mapping (resize); buffers
  validate eagerly (format/stride/span); superseded buffers release with
  a *signalled eventfd* fence — the documented headless stand-in for the
  kernel sync-file the DRM path will hand out. Release lands with the
  flip that stops reading the buffer.
- `ldp-server` — three default-no-op `Dispatcher` hooks (backward
  compatible): `on_bind` (globals emitting initial events: the output
  cascade, shm formats), `on_destroy` (central lifecycle → scene
  teardown), `on_wake` (the pump point), plus `DispatchCtx::emit_fd` /
  `ClientSession::send_event_fd` — the server-to-client descriptor path
  `buffer.release` requires.
- `ldp-client` — `Connection::create_object_fd`: the descriptor-carrying
  factory flow (`shm.create_pool`), the client-side counterpart of the
  server's FD-carrying emission.
- `ldp-renderer` — `Renderer::clear_damage`: the background pass
  primitive (damage-scoped clear; uncovered damage falls back to the
  desktop color).
- Integration suites (the Phase 10 exit criteria): `full_session` (the
  EC — complete client cycle incl. buffer exchange and presentation
  events, headless in CI), `render_pixels` (scanout pixel truth:
  subsurface stacking, premultiplied blending, incremental damage,
  detach repaint), `presentation` (vblank-grid timestamps, late-commit
  drops, superseded registrations, release-fence lifecycle),
  `lifecycle` (fatal validation codes, pool growth, destroy repaint +
  release, two-client isolation).

### Fixed
- `ldp-renderer`: the 1:1 word-copy path ignored the layer's destination
  x origin — layers placed away from (0,0) read out of bounds (found by
  the vertical slice's subsurface test; golden suites only placed at the
  origin).
- `ldp-display`: `EdidIdentity::synthesize` overwrote the 13th monitor
  name byte with the descriptor terminator, truncating full-width names
  on round trip (found by the output cascade test).

### Notes
- Scope honesty: DRM mode probes and reports real nodes (discovery +
  backend open + topology) but does not deliver pixels — GEM dumb-buffer
  or EGL scanout plumbing arrives with the GPU phases; the EC is the
  headless full session per the roadmap. Root surfaces map at the layout
  origin (the positioning shell is Phase 12).
- Workspace: 757 tests green (was 730); fmt, clippy `-D warnings`,
  rustdoc `-D warnings`, spec lint, and the ldpc drift gate all pass.

## [0.1.0-phase9] — 2026-09-24 — Display & GPU: KMS atomic backend + fence model

### Added
- `crates/ldp-display` + `crates/ldp-gpu` — the display and GPU halves of
  the hardware path (roadmap Phase 9, `docs/architecture.md` §9/§11).
  29 source files + 3 integration suites, both crates `ldp-core` + `libc`
  only (libc solely for the audited dlopen layers — the transport
  precedent: every module except `drm/sys`, `drm_sys`, `egl/sys` is
  `#![forbid(unsafe_code)]`, and each FFI call site carries a `SAFETY`
  comment).
- `ldp-display`: the object-safe `KmsBackend` trait (topology, connector/
  CRTC/plane snapshots, property catalogs, blobs, FB registration, atomic
  commits, event drain) with two interchangeable implementations —
  `MockDevice`, a deterministic in-memory DRM (laptop-dual preset: eDP
  panel + DP + HDMI, VRR-capable CRTC 48-144 Hz, six planes with
  `IN_FORMATS` caps, kernel-grade commit validation, hotplug injection,
  injected-clock timelines), and `DrmBackend` over `dlopen("libdrm.so.2")`
  (31-symbol table, `repr(C)` ABI prefixes pinned by size assertions).
  The typed `RejectReason` taxonomy (20 reasons) mirrors
  `drm_atomic_check_only`'s check order; requests are declarative
  (property-name addressing, backend-minted blobs); `OUT_FENCE_PTR` is
  modeled as a per-CRTC heap-stable slot whose address crosses the seam
  (the returned fence is an owned sync-file descriptor).
- The mock scanout timeline: nominal-grid vblanks; VRR flips land at
  `max(commit, last+min)` clamped into `[last+min, last+max]`, idle
  stretches to max; in-fences hold the queue (completion at
  `max(target, fence_ready)`); chained flips sequence off the pending
  flip. Every timestamp comes from the injected clock — event traces are
  byte-reproducible.
- The periphery: EDID base-block parsing (checksum-validated identity) +
  synthesis; the 68-byte `drmModeModeInfo` blob codec with exact
  clock-derived refresh (59.94 ≠ 60 stays distinguishable);
  `IN_FORMATS` blob codec (kernel `drm_format_modifier_blob` layout);
  sysfs backlight policy over a `BacklightFs` seam (raw-panel floors,
  monotone integer ramps with round-half-away-from-zero); udev hotplug
  monitoring over `dlopen("libudev.so.1")` (DRM-subsystem filter,
  O_NONBLOCK drain) implementing the `HotplugSource` seam; `Reprobe`
  topology diffs.
- `ldp-gpu`: render-node discovery (`drmGetDevices2` behind a two-symbol
  dlopen layer) with a deterministic selection policy (render node of the
  first device that has one; primary-node fallback only when it must,
  because that path needs DRM-Master); `DmaBufDescriptor` with
  kernel-grade validation (plane arity per format, row-minimum strides,
  explicit modifiers only — `INVALID` refused) and the exact
  `EGL_EXT_image_dma_buf_import(_modifiers)` attribute codec, encode and
  decode, all 26 eglext names pinned to their ABI values.
- The fence model (`ldp-gpu::sync`): sync-file descriptors (owned),
  syncobj timeline points, `ldp-display` mock tokens (the two crates'
  fence provenances meet by value — no dependency edge), and AND-merges
  (a merge is ready when its *latest* member is); `MockSyncDriver` runs
  one injected `Mono` clock over all three tables.
- The EGL bootstrap state machine behind the object-safe `EglApi` trait:
  load → display → initialize → capability gate
  (`EGL_EXT_image_dma_buf_import(_modifiers)`), typed failures at every
  step. `RealEgl` over `dlopen("libEGL.so.1")` — on GL-less machines the
  load itself fails typed (the honest headless outcome, CI-tested);
  `MockEgl` imports by *decoding the encoded attribute list* (the codec
  round trip runs on every import) and composites with the reference
  blend math.

### Fixed
- Resumed mid-implementation state: completed the missing `drm/mod.rs`
  translation layer and `udev_sys` monitor, reconciled module wiring,
  and corrected the mock's staged-blob overlay (an inline `MODE_ID`
  payload now validates and applies), the `IN_FORMATS` encoder's record
  layout (contiguous records, then bitmaps — the kernel layout), the
  EDID checksum arithmetic, the backlight ramp interpolation scope, and
  the 1440p144 fixture clock (580,078 kHz over 2720x1481 = 144 Hz under
  the kernel's rounded-vrefresh convention).

### Exit criteria
- Atomic-commit goldens: full pipeline bring-up, deterministic page-flip
  chains (byte-equal traces across fresh devices), the 20-reason
  rejection matrix in kernel check order, all-or-nothing commits,
  TEST_ONLY leaves no residue, out-fence lifecycle, in-fence gating,
  blob/FB lifecycles, deactivation dropping pending flips, hotplug
  reprobe round trip, `&dyn KmsBackend` polymorphism.
- VRR proofs at the device level: first flip at `last+min`, chained
  flips respecting the window, late commits landing at commit time,
  idle stretch to max, fixed-sync grid discipline, two-CRTC interleave
  in timeline order, reproducible VRR traces.
- The real layer, honestly: libdrm + libudev load and resolve here;
  `DrmBackend::open("/dev/dri/card0")` fails typed ENOENT (headless);
  `drmGetDevices2` walks clean; the udev monitor bootstraps or fails
  typed. `RealEgl` reports `LibraryLoad` (no libEGL in this sandbox) —
  the documented unavailable path.
- Bit-comparable harness (`ldp-gpu` dev-dep `ldp-renderer`): the same
  layer list through the reference `SoftwareRenderer` and the mock EGL
  path (DMA-BUF descriptors → attribute round trip → fenced draw gated
  on the mock sync driver → readback) agrees **byte-for-byte** on the
  representable subset — single opaque layers, partial placements,
  XRGB-over-ARGB, two- and three-layer stacks with partial opacity,
  XRGB targets — because the mock's blend is the reference blend
  (`a2 = mul255(sa, q)`, `inv = 255 - a2`, sums clamped to the result
  alpha) reached through a different plumbing path. Padded-stride
  imports read the pattern through the descriptor's own geometry.
- Workspace: 730 tests green (was 611), fmt · clippy `-D warnings` ·
  test · doc `-D warnings` · spec lint · ldpc drift — all passing from
  a fresh extract of the delivered zip.

## [0.1.0-phase8] — 2026-09-24 — Renderer: contract + software backend

### Added
- `crates/ldp-renderer` + `ldp-core` planar-layout amendment — the
  `Renderer` contract and the reference software compositor
  (`docs/architecture.md` §9.1, roadmap Phase 8). Eleven source modules +
  testkit, `#![forbid(unsafe_code)]`, exactly one dependency (`ldp-core` —
  no wire code, no I/O, no clock reads; rendering is a pure function of
  (output, damage, layers), which is what keeps the golden frames
  byte-stable). Module split: `lib.rs` (the trait), `errors.rs`, `layer.rs`
  (OutputDesc / SurfaceLayer / scanout eligibility), `view.rs` (the
  bounds-gated BufferView), `spans.rs` (per-row merged damage intervals),
  `mapping.rs` (Direct/Integer/Scaled pixel inverses), `sample.rs` +
  `yuv.rs` (format-complete sampling), `transfer.rs` + `pipeline.rs` (the
  color pipeline), `composite.rs` + `software.rs` (the span kernels and
  the backend).
- The `Renderer` trait: `begin_frame(output, damage)` →
  `submit(back-to-front layers)` → `end_frame() → CompletedFrame`; layers
  carry buffer + destination + transform + color description + opacity.
  `SoftwareRenderer` keeps a premultiplied-alpha u32 framebuffer with
  `readout()`/`clear()`.
- Four resolved pixel paths per layer: **copy** (1:1 untransformed opaque
  X-family word-copy with forced alpha), **opaque** (write-only sampling),
  **alpha** (integer premultiplied `over` in the target's encoded space —
  fixed-point `mul255` round-half-up, bit-stable, no libm), and
  **pipeline** (scene-linear decode/blend/encode for cross-description
  composites). Framebuffer blending policy locked: same-description
  layers blend encoded (pixman/GL-fast-path equivalence),
  cross-description blend linear.
- Format-complete sampling: all eleven v1 fourccs; DRM channel-order
  convention (u32 field order, little-endian memory — `wl_shm`
  byte-compatibility for the bridges); full and studio ranges with
  integer chroma pivots (128/512, P010 studio excursion 64..940 /
  512±448); YCbCr matrices *derived from the primaries' chromaticities*
  through the white-point solve (BT.709 anchors 0.2126/0.0722);
  RGB565 bit replication.
- All eight buffer transforms with exact integer 1:1 inverses
  (cross-checked against the core's forward `transform_point` per pixel)
  plus the continuous pixel-center inverse with nearest sampling for
  scaled layers (integer and continuous mappings agree exactly at
  1:1 centers — pinned by test).
- Damage-clipped span compositing: per row, destination ∩ damage rects,
  sorted and merged, so overlapping damage rectangles never double-blend
  and undamaged framebuffer content survives partial layer re-submission.
- The color pipeline (Phase 14's hook): range decode → transfer decode
  (sRGB, gamma 2.2/2.8, PQ/ST 2084 with absolute-nits output, HLG with
  scene-referred OETF^-1) → XYZ-derived 3×3 primaries conversion →
  reference-white anchor (PQ content at its 203-nit reference white lands
  on the SDR output's reference white; symmetric for SDR→HDR) → clamp;
  identical descriptions resolve to the identity fast path.
  `decode_transfer`/`encode_transfer`/`primaries_matrix`/
  `ycbcr_coefficients` are public deterministic math for `ldp-color`.
- `scanout_candidate`: the scheduler's direct-scanout pre-filter
  (fullscreen, untransformed, output-sized, opaque region covering the
  destination, matching format and color) with a typed rejection reason.
- `testkit`: the canonical pattern rule, per-format pattern buffers
  (premultiplied storage for ARGB/ABGR, forward YUV encoders matching the
  decode conventions), geometry/render helpers — shared fixtures, while
  every golden suite derives its expectations independently.

### Changed
- `ldp-core` buffer geometry: **fixed planar layouts** — the Phase 1
  layout math applied luma subsampling to every plane and gave planar
  chroma planes luma-shaped blocks (NV12 chroma rows were half-width;
  P010 luma was 1 byte per sample). New `FourCC::plane_block(plane)`
  carries per-plane `(bytes_per_block, sub_w, sub_h)`; validation,
  `simple_layout` and `spanned_bytes` use it. NV12 64×64 now has 64-byte
  chroma rows at offset 4096; P010 2-byte luma samples; YUV420's separate
  U/V planes have ceil(w/2)·1-byte rows. New pinning tests for absolute
  strides/offsets and spanned coverage (ldp-core 98 tests).

### Fixed
- PQ constants (m1 had a spurious /4; decode used the m2 exponent in the
  wrong direction) and the HLG OETF's linear branch (√(3E), not
  √(12E)·3) — both caught by definitional anchors (PQ 0.508 ↔ 100 nits,
  HLG 0.5 ↔ 1/12).
- The 3×3 column-basis solve was transposed (Cramer numerators built for
  the row-major reading) — replaced with scalar-triple-product Cramer,
  correct by construction; BT.709 Kr/Kb now match the standard.
- Full-range chroma pivot moved from `v − 0.5` to the integer 128 (512
  for P010): neutral chroma decodes to exactly zero, keeping gray/black/
  white anchors byte-exact.
- Direct-scanout eligibility computed `opaque − dest` instead of the
  holes `dest − opaque`, and missed the output-sized buffer requirement
  (new `Scaled` rejection) — caught by the scanout suite.
- The copy path never reported its pixels in `RenderStats` (the 4K EC's
  work accounting was zero) — caught by the perf gate.

### Tests (EC)
- `tests/golden_formats.rs` (5): all eleven formats against
  independently-written straight-line decoders with their own addressing
  and hard-coded BT.709 constants; RGB family byte-exact, YUV ±1 LSB;
  definitional anchors (gray/black/white byte-exact in every YUV format,
  pure red from the definitions guarding against U/V swap, dirty X byte
  ignored).
- `tests/golden_blend.rs` (10): hand-computed premultiplied `over`
  (191/32/16 word), opacity folding (mul255 arithmetic), transparent
  pass-through, layer order non-commutativity, exact damage clipping
  with sentinel preservation, overlapping damage blending once, stats
  accounting, X-output alpha forcing, run-to-run bit stability.
- `tests/golden_transform.rs` (10): all eight transforms against the
  forward-constructed reference (core `transform_point` + offsets),
  hand-placed corners, composition identities (R90∘R90=R180 via
  intermediate framebuffers), integer up/down-scale footprints,
  transform+scale composition, edge clipping, odd-scale rounding edges,
  NV12 chroma-grid consistency under rotation.
- `tests/golden_color.rs` (8): studio-range expansion, BT.2020 gray
  invariance + saturation movement, PQ reference-white anchoring (203
  nits → SDR white, 20 nits proportional), cross-description linear
  blending vs an f64 model, transfer-mismatch linear path, pipeline
  damage clipping, linear opacity blending.
- `tests/scanout.rs` (4) and `tests/perf_4k.rs` (2, release-only): the
  eligibility matrix, and the **4K single-surface EC — ~3.5–3.9 ms
  median (7 runs, warmup) against the < 8 ms gate on the 2-core CI
  box**, with per-run work accounting (8,294,400 opaque pixels) and
  half-damage scaling checks.
- 39 in-crate unit tests (transfer anchors and round trips, matrix
  identities, coefficient standards, span merging, view bounds, mapping
  agreement, protocol errors).

## [0.1.0-phase7] — 2026-09-24 — Deadline frame scheduler

### Added
- `crates/ldp-compositor` (scheduler half) + `ldp-core` timing additions —
  the deadline frame scheduler (`docs/architecture.md` §10.3, roadmap
  Phase 7). Six new modules, `#![forbid(unsafe_code)]`, still exactly
  one dependency (`ldp-core` — no wire code, no I/O, no clock reads; the
  scheduler is a pure state machine over timestamped inputs, which is
  what makes it replayable). Module split: `predictor.rs` (the
  `FrameClock`), `sched_types.rs` (the vocabulary: events, policy,
  registration bookkeeping, the recorded input stream), `scheduler.rs`
  (the machine), `coalesce.rs` (the class queue), `replay.rs` +
  `replay_codec.rs` (harness + binary format).
- `predictor.rs` — the per-output `FrameClock`: strict next-vblank
  lookup, n-ahead extrapolation through the PLL's *effective step*
  (nominal period + integrated correction, so deep targets stay exact
  at lock under period drift — the plain period would accumulate the
  drift per extrapolated step), and measured-refresh bookkeeping that
  accepts only single-period samples (stalls, duplicates and missed
  vblanks never poison the `presented` feedback; invalid samples never
  clobber the last valid one).
- `scheduler.rs` — `FrameScheduler`: the frame contract as specified in
  `ldp.core`. One live registration per surface
  (`frame_request` → `SchedEvent::FrameTarget` carrying the
  `FrameDeadline` contract `target_vblank − submit_cost −
  flip_latency` for the `depth + extra_lead`-th vblank ahead, with the
  minimum-lead retarget walk); a commit strictly before the deadline
  (before `deadline + vrr_window` under adaptive mode) satisfies and
  binds it to that vblank; **every registration terminates in exactly
  one event** — `presented` (measured refresh, vblank/torn flags) or
  `frame_dropped` with reason `deadline_missed` / `superseded` (a newer
  registration replaced it — the §10.4 presentation coalescing rule) /
  `surface_hidden` (unmap/full occlusion: at hide, at commit, or at
  expiry while hidden) / `output_off` (park). `throttled` is reserved
  for the Phase 15 VRR policy engine and never emitted. Late content
  still becomes live state silently. The **escalation ladder** widens
  deadlines for systematically slow clients
  (`extra_lead = min(miss_streak / escalate_after, max_extra_lead)`)
  with hysteretic de-escalation (`deescalate_hits` consecutive
  presentations per step) so clients between two pipeline depths settle
  instead of oscillating. Immediate mode = commit-at-ready with torn
  presentation flags (the KMS immediate-flip path arrives in Phase 9).
  Pre-anchor and parked timelines defer frame-target replies until the
  timeline goes live — frame callbacks queue through DPMS-off. Inputs
  are asserted non-decreasing; per-surface iteration is ordered —
  emissions are a pure function of the input stream.
- `coalesce.rs` — `CoalescingQueue`, the §10.4 class-aware emission
  path: input/data never drop (explicit `QueueFull` backpressure),
  presentation/configuration coalesce to the latest per key (the
  surviving value keeps the latest arrival position, so cross-class
  ordering stays arrival order), per-frame terminal signals are unkeyed
  and never replaced, and a coalescing class at capacity evicts its own
  oldest item. Tombstone slots are compacted; `SchedEvent` implements
  `Coalescible` (frame_target/presented keyed per surface+kind,
  frame_dropped unkeyed).
- `replay.rs` — the determinism replay harness: `SchedInput` (the
  driver vocabulary), `run()` (the pure session runner), `record()` /
  `replay()` with divergence reporting, and a self-describing
  **checksummed binary recording format** (magic + version + config +
  inputs + outputs + FNV-1a; corruption, truncation, bad magic/version,
  out-of-order inputs and zero refresh are all explicit errors).
  Recordings are canonical byte-for-byte; `ldp-debug` (Phase 18) will
  consume this format.
- `ldp-core`: `FrameDropReason` (the `ldp.core.frame_drop_reason` wire
  enum, previously missing from the core mirror) and
  `VblankPredictor::{last_flip, correction_ns, effective_step_ns}` —
  the correction is now documented as the drift term it actually is.

### Tests (exit criteria)
- **Golden timeline conformance** (`tests/scheduler_golden.rs`, 15
  scenarios): hand-built input streams pin exact emission vectors to
  the nanosecond — steady state, miss-and-retarget, the full escalation
  ladder (1→2→3→3 vblanks of lead, walking back down), supersede,
  hide/unhide, park/resume with deferred replies, unanchored start,
  depth-2 pipeline, immediate/torn, adaptive window absorption and its
  bounded expiry, minimum-lead retarget, multi-surface emission order,
  jittered flips within the arrival slack, silent unregistered content.
- **Deadline hit-rate properties under jitter**
  (`tests/scheduler_property.rs`, 8 properties × 3 seeds): a 97%-budget
  client on a ±250 µs-jittered panel holds ≥95%; a ~56.8 Hz panel
  against a 60 Hz mode locks (post-lock budgets track the true period,
  misses vanish); a systematically slow client converges via the ladder
  (≥75% hits, only deadline-miss drops); a 200 ms stall re-anchors
  without a miss storm; immediate mode presents everything torn;
  adaptive eliminates misses where the vsync control cannot; three
  clients on one output never starve each other; `presented` refresh
  tracks the panel, not the mode.
- **Determinism replay** (`tests/scheduler_replay.rs`, 6 tests): every
  property scenario records, decodes losslessly, re-encodes
  canonically, and replays to the identical event vector; same seed →
  same bytes, different seeds differ; tampered outputs flag the
  divergence index; recording sizes are bounded.
- **Coalescing classes** (`tests/coalesce.rs`, 4 tests): a full
  slow-client backlog collapses to the latest target + latest presented
  per surface while every drop survives in order; input survives and
  never reorders under backpressure (retry-until-delivered); data never
  drops; coalesced events keep the latest position.
- Unit and contract tests: 60 in-crate (predictor grid semantics, PLL
  drift-lock and interval bookkeeping, queue policies, codec
  round-trips and corruption detection) plus 16 drain-level scheduler
  contract tests and 13 queue-policy tests as integration suites.
  Workspace total: **536 tests** (was 455), all green alongside fmt,
  clippy `-D warnings`, rustdoc `-D warnings`, spec lint, and the
  `ldpc` drift gate.

### Changed
- `docs/architecture.md` §10.3/§10.4 rewritten to the implemented
  semantics (the frame contract, escalation ladder, deferred replies,
  the adaptive window's scope vs. Phase 15, and the coalescing
  queue's exact replacement rules); `ldp-compositor` crate docs and
  re-exports extended (scheduler, coalesce, predictor, replay).

## [0.1.0-phase6] — 2026-09-23 — Compositor core

### Added
- `crates/ldp-compositor` v0.1.0 — the scene graph and damage engine
  (`docs/architecture.md` §7 and §10.1–10.2, roadmap Phase 6); 9 modules,
  `#![forbid(unsafe_code)]`, one dependency (`ldp-core` — no wire code,
  no I/O; the engine is the same library whether driven by the protocol
  dispatcher or by tests). `SurfaceTree` is the authoring structure:
  roots with output positions, nested subsurface roles, per-parent
  back-to-front stacking lists with validated `place_above`/
  `place_below` (cross-parent restacks rejected; each restack records
  the crossed range), and the **atomic commit cascade** — roots and
  desync subsurfaces apply their pending state immediately and then
  flush every sync-mode descendant carrying deferred state or a pending
  position, recursively; sync subsurface commits only *stash* (merged
  over an earlier stash, later requests winning). Pending state
  accumulates the request-shaped setters of `ldp.core.surface`
  (attach/damage/damage_buffer/set_transform/set_buffer_scale/
  set_input_region/set_opaque_region/set_color/set_hdr_metadata/
  set_presentation_mode) and applies atomically with the damage
  accounting: explicit damage clipped to the new bounds; a *different*
  buffer with no explicit damage damages the whole surface; a detach
  contributes no content damage (the unmap is a coverage change);
  `damage_buffer` converts against the *pending* geometry
  (buffer-to-surface transform, then inverse scale with outward
  rounding).
- The **damage engine** (`DamageEngine::compute`) with the normative
  per-pixel **fold model**: a composited pixel is a bottom-to-top fold
  where translucent surfaces contribute and an opaque surface wipes
  everything below; a pixel must be repainted iff its fold value
  changed. Four exact rule families realize it: **content** (R1 —
  accumulated surface damage, translated, clipped to bounds, minus the
  current occlusion; the same region is the per-surface *presentation*
  damage), **coverage** (R2 — old-only cells through the old occlusion,
  new-only through the current one, and when the *offset* changed the
  interior too, because every interior cell now shows a shifted content
  cell), **opaque flips** (R3 — the old ∆ new opaque footprint, only
  where a surface below can be revealed or hidden), and **restack
  pairs** (R4 — subtree-footprint intersections of the mover and the
  crossed siblings, flushed when the front-to-back walk enters the
  front-most surviving participant's subtree so the mask is exactly the
  opaque set above the whole crossed range). Removed subtrees are the
  coverage rule with an empty new side. The engine computes the three
  architecture damage classes — repaint, scanout (fully-visible,
  untransformed, fully-opaque root surfaces entirely on the output),
  and per-surface presentation/visible regions — all clipped to the
  output bounds, and updates each surface's pass records (mapped
  bounds, occlusion, opaque footprint) for the next frame's old side.
  The **front-to-back flatten** (`occlusion`) supplies per-node output
  geometry, subtree extents (a restack moves whole subtrees — an
  unmapped parent's subsurfaces carry the pixels), and the occlusion
  walk (visible = mapped bounds minus the opaque of everything above).
- **Corpus conformance (exit criterion)**: a randomized scene-graph
  corpus (5 seeds × 80 frames; create/commit/damage/move/restack/
  mode-flip/destroy plus root-commit sweeps) drives the engine and an
  **independent per-pixel reference implementation of the fold model**
  (per-surface version grids; per-cell contribution lists carrying the
  *local cell identity* — a moved surface changes every interior pixel
  — truncated at the topmost opaque). Exact cell-set equality is
  asserted every frame for both repaint and presentation damage. The
  corpus proved and refined the rule set: opaque flips need a surface
  below; restack pairs need subtree footprints and a first-visited
  flush point; moves need the full old ∪ new footprint minus
  both-frames occlusion; geometry-only changes need only the symmetric
  difference.
- `FocusStack` (MRU activation order + keyboard focus pointer + Alt-Tab
  cycling) with by-construction invariants: no duplicates, the focus is
  always tracked or `None`, removals reassign focus to the next MRU
  entry. `FrameSnapshot` (`SurfaceTree::snapshot`): the immutable
  per-frame capture for the render path — `Arc`-shared nodes (structural
  sharing keeps captures cheap and old snapshots alive across edits),
  a monotonic `FrameId`, a by-id index, and the back-to-front draw
  list (parents before children — children render above parents —
  verified to be the exact reverse of the damage engine's front-to-back
  flatten). `FrameSnapshot::check` re-verifies the structural
  invariants; the randomized mutation-storm test holds three snapshots
  live across 120 rounds of mutations asserting zero drift.

### Changed
- README status/layout/deps; `docs/architecture.md` §10.1–10.2 updated
  to the implemented semantics (fold model, four rule families,
  per-surface pass records, subtree-scoped restack damage).
- Workspace: 6 runtime crates + 1 tool crate; 455 workspace tests
  green (ldp-compositor contributes 77: 48 unit + 29 integration).

## [0.1.0-phase5] — 2026-09-23 — Client library

### Added
- `crates/ldp-client` v0.1.0 — the client half of the protocol stack
  (`docs/architecture.md` §5, roadmap Phase 5); 13 modules,
  `#![forbid(unsafe_code)]`, no new runtime dependencies (std plus the
  three LDP layer crates). `Connection` owns one `AF_UNIX` stream, its
  framing pair, the proxy map, the ID allocator, and the class-lane
  queue: `connect`/`connect_with` perform the `hello`/`welcome`
  handshake (bounded by `handshake_timeout`; nothing may precede
  `welcome` — an early event is an `invalid_state` violation), expose
  the typed `Welcome`, and implement the blocking driving model v1
  (`pump_one` reads one message, `dispatch_pending` /
  `dispatch_budget` drain dispatchable events, `roundtrip` is the sync
  barrier, `ping` the keepalive; event loops flip to nonblocking after
  the handshake and poll `raw_fd` themselves). The inbound pipeline
  mirrors the server's §9 order with the direction flipped: framing →
  structural decode → **proxy resolution before the signature check**
  (stage 3 runs against the target proxy's interface and pinned
  version — the client-side type-confusion defense; an event for an
  untracked object is a fatal `UnexpectedEvent`), then classification
  into a dispatch lane. `ProxyMap` tracks the client's mirror of the
  server's object store: bootstrap object pre-installed,
  `Live`/`PendingDestroy` states covering the destroy window (events
  still resolve until the `destroyed` confirmation), removal on
  `destroyed`/`revoked`, and **automatic proxies for server-announced
  objects** — every event `new_id` argument binds a proxy at its `of`
  interface (short names resolve within the declaring module). The
  `IdAllocator` hands out the client ID half monotonically from 2
  (no free list: reuse is the classic double-allocation bug the
  generation discipline exists to guard; exhaustion at 2^31 - 2 is a
  hard error, not a wrap). `create_object` is the typed factory flow
  (`compositor.create_surface`, `seat.get_pointer`, …): allocate the
  `new_id`, splice it at the schema-declared argument position, send,
  register the proxy at the interface's schema maximum. `registry`/
  `bind`/`bind_version` bootstrap the session (a registry object is
  materialized automatically when none exists). Errors:
  `ClientError` (layer, disconnect, server-fatal, misuse families)
  with `DisconnectKind` classification mirroring the server's
  (clean between messages, crash mid-message, transport, local drop
  after a fatal `connection.error` — which is forwarded to the handler
  *before* the connection dies).
- The **class-lane scheduler** — the Phase-5 engine behind the
  architecture's latency guarantee ("a burst of `frame_target` events
  can never delay pointer motion beyond one dispatch cycle"):
  `EventClass` (Input / Data / Control / Configuration / Presentation)
  with `class_of` encoding the v1 surface (input module minus its
  device-configuration events; data module; the presentation set
  `frame_target`/`presented`/`frame_dropped`/buffer `release`; shell/
  color/session/a11y as configuration; **unknown interfaces classify
  as Control** — the conservative barrier that cannot break ordering
  invariants it might depend on). `EventQueue` keeps one FIFO lane per
  class; dispatch order is input, data, barrier-ready control,
  configuration, presentation; a control event is a **barrier** that
  waits until no earlier-arrived event remains queued in any lane —
  exactly the `sync_done` contract ("everything the server queued
  before my sync has been delivered") and the safest reading of the
  lifecycle events. Progress is structural (the globally earliest
  event is always dispatchable — the scheduler can never wedge);
  bounded batches change throughput, never the dispatch set. Events
  own their ancillary FD table; untaken FDs close on drop.
- **Reconnect as session rebuild** (`reconnect`): an LDP connection is
  stateful end to end — every proxy, pinned version, and binding dies
  with the socket — so `Reconnector` runs the loop connect → handshake
  → application `Rebuild` callback ("socket works" to "session works",
  where state restoration belongs) → retry on failure, under a
  `ReconnectPolicy` of bounded exponential backoff with deterministic
  jitter (seeded LCG — no `rand` dependency) and a 60 s per-step sleep
  cap; `connect_with_retry` is the one-shot helper. Handler
  re-entrancy is a compile error by construction: handlers receive
  `Event` references only, never a connection handle.
- Integration suites against a live `ldp-server` (scripted
  `FactoryDispatcher` harness): `full_session.rs` — the complete
  lifecycle (connect, handshake, `get_registry`, global replay with
  the barrier ordering asserted, bind, factory objects, sync
  round-trips, ping, destroy with cookie correlation, clean
  disconnect) plus the failure paths: `large_messages` negotiation
  round-trips 1 MiB frames, the introspection option gates
  `registry.introspect`, server errors are fatal and reported with
  sticky state, handler errors abort dispatch and propagate, a
  whole session survives disconnect via `Reconnector` rebuild, and
  exhaustion is bounded. `class_lanes.rs` — the exit-criterion
  property tests: one input event jumps a 100,000-event presentation
  backlog; randomized flood property (input latency holds across
  thousands of interleavings); **live** presentation floods from a
  real server cannot starve pointer motion; barriers never wedge
  behind lanes; budgets never change the dispatch set.

### Changed
- `README.md` status, layout, and dependency notes; `docs/architecture.md`
  §5 client-half bullets now match the implementation (single
  `EventHandler` receiving classified events, five lanes including the
  data lane, reconnect as application-driven session rebuild).
- Workspace: 5 runtime crates (`ldp-core`, `ldp-protocol`,
  `ldp-transport`, `ldp-server`, `ldp-client`) + 1 tool crate (`ldpc`);
  378 workspace tests green.

## [0.1.0-phase4] — 2026-09-23 — Server core

### Added
- `crates/ldp-server` v0.1.0 — the connection/session engine
  (`docs/architecture.md` §4–5, roadmap Phase 4). `Server` performs
  accept-with-credentials, assigns audit client IDs, enforces the
  `max_clients` ceiling (refusal closes the socket and audits before any
  protocol byte is read), and drives one session thread per connection
  with panic isolation and a live-count that releases only after the
  session's socket is closed. `ClientSession` runs the full §9 pipeline
  per message — framing (transport stage 1), structural decode (stage 2),
  **object resolution before the signature check** (stage 3 runs against
  the target object's interface and pinned version: the type-confusion
  defense), then dispatch. Handshake state machine (`hello` must be the
  first message; a second is `invalid_state`), option granting as the
  intersection with server policy (`large_messages` rebuilds the framing
  pair at 64 MiB), `welcome` with release/caps/client-id/sandbox verdict,
  and fatal-error emission (`connection.error` delivered best-effort,
  then close). The generational `ObjectStore` keys on the full wire ID
  (client and server ranges are disjoint slots), advances generations on
  slot reuse, and offers `StoreHandle` (id, generation) for server-side
  stored references — the misdelivery protection. Built-in semantics for
  `ldp.core.connection` (sync/ping round-trips, the central
  destroy/destroyed lifecycle with double-destroy and bootstrap-object
  rules, `get_registry` bootstrap with global replay) and
  `ldp.core.registry` (bind with version pinning and advertisement
  checks, introspection served as compact JSON — one `schema` event per
  interface for whole-protocol queries so every payload fits the string
  limit; conformance-checked across all 33 interfaces). The `Dispatcher`
  trait + `DispatchCtx` seam: every non-core interface is a dispatcher;
  contexts expose event emission, object creation/revocation, and FD
  ownership transfer (`take_fd`/`fd_released` with the `client_fds`
  ceiling), untaken FDs close on return. Audit hooks (`AuditSink`:
  connected/handshake/bind/destroy/revoke/denied/fatal/unhandled/
  disconnected/rejected) with `NullAudit` and a bounded `AuditRecorder`.
  `#![forbid(unsafe_code)]`, zero new runtime dependencies.
- Registry bootstrap amendment (pre-release spec change, the gap Phase 4
  implementation exposed): `connection.get_registry(version, new_id)`
  (request opcode 5) materializes the per-connection registry — the
  mechanism that bootstraps binding itself; the registry is also
  implicitly advertised as a global so further registries bind like any
  other interface. `registry.introspect`/`schema` doc strings updated to
  the per-interface streaming contract. Regenerated committed artifacts
  (tables, blob, reference docs); v1 inventory now 90 requests.
- Protocol doc sync: §6 handshake diagram and §13 example session now
  match the normative TOML argument spellings; §3.1/§3.2 rewritten to
  state the two-layer generation discipline precisely (wire references
  resolve to current occupants; stored references are
  generation-checked).

### Fixed
- `ldp-transport` `BackpressureConfig::from_limits`: the writer's queue
  ceiling is now `max(event_queue_bytes, message_bytes)` — a legal
  message (already within the per-message ceiling) could previously be
  *unqueueable* whenever the negotiated message ceiling exceeded the
  event-queue bound, deadlocking `large_messages` transfers. One legal
  message is the irreducible unit of parking; the slow-peer bound is
  unchanged under default limits.

### Changed
- Pinned inventories updated for the amendment: 90 requests + 120 events
  across 33 interfaces (ldpc conformance test, drift pins,
  round-trip corpus).

## [0.1.0-phase3] — 2026-09-23 — Transport

### Added
- `crates/ldp-transport` v0.1.0 — the AF_UNIX transport layer
  (`docs/architecture.md` §6). Listener/connect over filesystem and
  abstract-namespace socket addresses, with `SO_PEERCRED` credentials
  read at accept time, before any protocol byte is processed. The
  framed message reader/writer implements `docs/protocol.md` §1–2
  stage 1: one message = one `sendmsg` with `SCM_RIGHTS` FDs riding
  the same call; the header FD count is cross-checked against the
  ancillary array (`fd_mismatch` otherwise); message and FD ceilings
  are enforced *before* any payload allocation; every FD of a
  rejected message is closed (kernel-closed overflow included). The
  writer re-validates messages locally (length, alignment, header
  `fd_count` vs batch size) so a local bug surfaces as the same error
  the peer would report, attaches FD batches at their message's first
  byte with chunk capping so ancillary data never rides the wrong
  message, and bounds its queue by `Limits::event_queue_bytes` plus a
  queued-FD budget — a slow peer parks its own queue, never server
  memory. Backpressure hooks (`WriterHooks`: congestion / drain /
  recovery / overflow) plus a `HookRecorder` for tests.
- FD hygiene primitives: `FdList` (RAII ancillary batch, adoption via
  `fcntl(F_GETFD)` validation), `count_open_fds()` for the leak gate,
  `eventfd` creation, and a bytewise (safe, native-endian) cmsg codec
  walking multiple `SCM_RIGHTS` control messages.
- Exit criteria (roadmap Phase 3) all green: 10,000 FD-passing
  round-trips with the process FD table byte-identical afterwards;
  a malformed-frame corpus (oversize, count mismatch both directions,
  `fd_count` over limit, FDs smuggled mid-payload, truncation +
  disconnect) rejected with the right wire code; a 200-FD bomb (above
  the 64-FD receive buffer) rejected with zero leaks; slow-peer
  backpressure bounded, ordered, and recovered; `ldp-protocol`
  messages with `fd` arguments cross the transport and pass stages
  1–2, with stage 3 rejecting a hand-built mismatched signature.

### Safety
- `unsafe` exists only in `ldp_transport::sys` as individually
  documented syscall wrappers (the 2026 `libc` releases mark the
  syscall functions `unsafe` themselves); every other module is
  `#![forbid(unsafe_code)]`. `libc` is the crate's single runtime
  dependency beyond `ldp-core`, chosen precisely to minimize
  hand-reviewed `unsafe`.

### Design decisions
- Partial-send discipline: on stream sockets a `sendmsg` may accept a
  prefix; SCM_RIGHTS transfers with the *first* accepted byte, so the
  writer carries FD attachments by byte offset and continuation
  chunks carry no control data.
- Chunk policy by mode: nonblocking streams drain while the kernel
  accepts bytes; blocking streams do one `sendmsg` per call (a
  blocking loop would park the thread until the peer — usually behind
  the same caller — reads).
- Blocking-mode `sendmsg` never spins (poll/eventd integration is
  Phase 4's event loop); a framing error poisons the reader — the
  protocol has no resynchronization.
- Send-side "ownership transfer" means *responsibility to close*: the
  kernel duplicates SCM_RIGHTS FDs at send time; the writer closes the
  caller's originals after the carrying chunk is accepted.

## [0.1.0-phase2] — 2026-09-22 — Protocol compiler & wire codec

### Added
- `tools/ldpc` v0.1.0 — the LDP protocol compiler. Parses `spec/*.toml`
  (serde `deny_unknown_fields` everywhere: unknown keys are errors),
  validates the full grammar of `docs/spec-format.md` §2 — including the
  cross-module rules a file-by-file linter cannot see (globally unique
  fully-qualified interface names, import resolution, enum/bitset/interface
  name disjointness) — and generates: Rust schema tables + typed
  `#[non_exhaustive]` enums + bitset const modules + opcode constants
  (`crates/ldp-protocol/src/generated/`, committed), the `LDPS` binary
  introspection blob (the spec, re-serialized), and per-module reference
  docs (`docs/reference/`). CLI: `ldpc check` / `ldpc gen [--check]`.
  Output is byte-stable; CI and `cargo test` fail on drift.
- `crates/ldp-protocol` v0.1.0 — the wire codec. 16-byte envelope
  encode/decode with reserved-flag rejection; tagged argument units
  (8-byte aligned, zero-padded, NUL-terminated strings, 16-byte array
  headers with explicit element tags); whole-message `encode`/`decode`
  implementing validation stages 1–2 (framing, structural) with the
  security property that no allocation proportional to untrusted sizes
  happens before all length fields are bounds-checked; stage-3 signature
  checking (`check_signature`: types, counts, nullability, new_id
  ownership direction, since-gating, strict-mode enum/bitset domains);
  `SchemaRegistry` lookup over the generated statics; `blob` parser for
  the introspection format. `#![forbid(unsafe_code)]`, depends only on
  `ldp-core`.
- Schema-driven conformance: every one of the 209 v1 operations
  (89 requests + 120 events across 33 interfaces) is round-tripped
  through the codec and signature-checked in both validation modes; a
  hand-built malformed corpus (framing, FD, tag, string, float, array,
  padding cases) is rejected at the right stage with the right error
  code.
- Drift gate: `tests/drift.rs` recompiles the spec and byte-compares
  committed generated output; `ldpc gen --check` does the same from CI
  and `./scripts/verify.sh`.
- `.github/workflows/ci.yml` — the concrete CI pipeline (fmt + clippy
  + test + doc + spec lint + drift).

### Changed
- `ldp-core::wire`: `Value::Array` now carries the wire element tag
  (`Array { element, items }`) so empty arrays keep their type on the
  wire and signature validation is exact; added
  `Primitive::arg_type()` and `ArgType::is_array_element()`; added the
  checked constructor `Value::array`.
- `docs/protocol.md` §4: the argument-unit layout is now normative and
  exact (tag byte, value, zero padding to the next multiple of 8;
  arrays use a 16-byte header), the §4.1 worked example is byte-exact
  against the encoder (the Phase 1 example was internally inconsistent),
  over-length strings are `limit_exceeded` (not `invalid_string`, which
  is reserved for encoding violations), and enum/bitset forward-
  compatibility is spelled out: unknown values are structurally valid
  and must be ignored at the semantic layer; strict mode (tooling)
  rejects them.
- `docs/protocol.md` §2: header/ancillary FD count mismatch is
  `fd_mismatch` (the taxonomy's code for it), clarifying loose Phase 1
  wording.

### Fixed
- Corrected the v1 inventory bookkeeping: the spec set defines **33
  interfaces / 89 requests / 120 events / 31 enums / 16 bitsets**
  (earlier internal notes mis-stated 45/53/109; the TOML sources were
  always correct and unchanged).

## [0.1.0-phase1] — 2026-09-22 — Foundations

### Added
- Repository scaffold: dual MIT/Apache-2.0 licensing, contribution and security
  policy, CI workflow (fmt + clippy + tests), `scripts/verify.sh`.
- `docs/architecture.md` — complete subsystem architecture (21 areas), crate map,
  process/threading/memory models, performance and security strategy.
- `docs/protocol.md` — LDP wire protocol: message envelope, tagged argument
  encoding, object model with generations, bootstrap handshake, version and
  capability negotiation, error taxonomy, event coalescing classes.
- `docs/spec-format.md` — the TOML protocol specification grammar and validation
  rules used by the `ldpc` compiler (Phase 2).
- `docs/threat-model.md` — adversary model, permission matrix, attack surface,
  layered (manifest + runtime escalation) security design.
- `docs/roadmap.md` — 20-phase build plan with per-phase exit criteria.
- `spec/` — complete v1 protocol specification, 8 modules: `core`, `input`,
  `shell`, `data`, `color`, `security`, `a11y`, `session`.
- `crates/ldp-core` v0.1.0 — foundation types: `ObjectId`/`ClientId` identity
  with client/server split, version negotiation, error taxonomy with stable wire
  codes, wire argument value model, geometry + region algebra with damage
  subtraction, frame-timing/deadline types, color-space and HDR metadata types,
  buffer formats (fourcc/modifiers/plane layout), 128-bit capability bitsets,
  256-bit access tokens, protocol limits. Zero dependencies,
  `#![forbid(unsafe_code)]`, 100% documented public API, unit + invariant tests.

### Notes
- Phase 1 contains no `unsafe` code, no third-party dependencies, and no stub
  implementations. Later crates (transport, GPU, display) introduce reviewed
  `unsafe` blocks and runtime-loaded native libraries by policy.
