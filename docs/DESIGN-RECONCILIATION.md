# Design reconciliation pass (for Fable)

**Status:** not started. Run when usage allows.
**Goal:** make the running app's **Almanac** theme render pixel-identical to the
design mockups, and stop the CSS drift that comes from re-deriving rules by hand.

## Why this exists

There are two artifacts describing the same UI:

- **Mockups** (`docs/mockups/*.html`) - single-theme, hardcoded hex, the design
  source of truth. `almanac-design-pass-2.html` is the most complete and current
  (updates view + all states/feedback/safety patterns). `workshop.html` and
  `terminal.html` are the other two themes; `almanac-base.html` is the earlier
  Almanac take.
- **App stylesheet** (`src/themes.css`) - token-based, three themes swapped via
  `:root[data-theme="..."]`, styling one React component tree (`src/App.tsx`).

Every time a component was touched during the build, its rules were re-derived
from memory instead of diffed against the mockup. That introduced drift (wrong
font sizes, stray borders, off paddings) that then had to be caught one
screenshot at a time. This pass fixes it at the root: the mockup and the app
should share the same numbers, so they cannot drift.

## The rule

**The mockup is the source of truth for structure and dimensions. Tokens are the
source of truth for color.** For each mockup CSS rule:

1. Confirm the app's corresponding rule matches **exactly** on every non-color
   property - padding, margin, font-size, letter-spacing, border-radius,
   gap, border-width, flex/grid structure, line-height.
2. For each **color** in the mockup rule, map it to the token whose **Almanac**
   value equals that hex (see the token map below). If no token matches, the fix
   is to **correct the token's Almanac value**, not to hardcode the color in the
   component. Never introduce a new hardcoded hex in `themes.css` component rules.
3. If the mockup and app genuinely disagree on a non-color property, the mockup
   wins unless a later user instruction overrode it (those overrides are listed
   in "Deliberate deviations" below - honor them).

## Token map (mockup hex -> Almanac token)

From `almanac-design-pass-2.html` `:root` vs `src/themes.css` `:root` (Almanac):

| Mockup var / hex | themes.css token | Almanac value |
|---|---|---|
| `--field #0E1512` | `--app-bg` | `#0E1512` |
| `--side #101915` / `--side2 #0C1310` | `--side-bg2` / `--side-bg` | `#111B16` / `#0C1310` |
| `--leaf #E9DDC4` (parchment) | `--content-bg` | `#E9DDC4` |
| `--leaf2 #e2d4b9` | `--content-bg2` | `#e2d4b9` |
| `--ink #211C12` | `--fg` | `#211C12` |
| `--ink2 #5f5741` | `--fg-muted` | `#5f5741` |
| `--ink3 #8f8467` | `--fg-faint` | `#8f8467` |
| `--rule #cbb98f` | `--line` | `#cbb98f` |
| `--rule2 #d8cba8` | `--line-soft` | `#d7c9a4` (**check: mockup d8cba8 vs token d7c9a4 - reconcile**) |
| `--copper #B26A22` | `--accent` | `#B26A22` |
| `--copper2 #8a4f13` | `--accent-ink` | `#8a4f13` |
| `--copper-fg #f4e9d2` | `--accent-fg` | `#f4e9d2` |
| `--verd #3C7A5C` | `--ok` | `#3C7A5C` |
| `--ochre #8a6414` | `--warn` | `#8a6414` |
| `--oxblood #8c3a2c` | `--bad` | `#8c3a2c` |
| `--parch #E9DDC4` | `--side-fg` | `#E9DDC4` |
| `--parch2 #b6a985` | `--side-muted` | `#b6a985` |
| `--pfaint #6f8578` | `--side-faint` | `#6f8578` |

Note the button gradient in the mockup (`.cta`, `.play`) is
`linear-gradient(180deg,#bd7327,#a55d1c)` with border `#7a430e`. The app
currently uses flat `var(--accent)`. Decide with the owner whether to port the
gradient (adds warmth, matches mockup) or keep flat. If ported, add a
`--accent-grad` token per theme so Workshop/Terminal stay coherent.

## Blocks to reconcile (checklist)

Diff each mockup rule against `src/themes.css`. Known-touched, verify all:

- [ ] `.titlebar` / `.winbtn` (mockup `.tbar` / `.winbtn`)
- [ ] `.side`, `.brand`, `.brand-name/-sub`, `.hr`, `.acct-chip` (mockup `.acct`), `.wax`, `.who`, `.signed`, `.idx`, `.navbtn` (mockup `.nav`), `.dock`, `.play`
- [ ] `.lhd`, `.eyebrow`, `.title`/`.lhd-title`, `.controls`, `.pchip` (mockup `.chip`), `.ctacol`, `.capt`, `.btn`, `.cta` + `.btn-prog`
- [ ] `.toolbar`, `.filterbox` (mockup `.filter`), `.counts`, `.cdot`, report link (mockup `.report`)
- [ ] `.register`, `.ghead` (+`.hold`), `.entry`, `.lineno`, `.chev`, `.mname`/`.mid`, `.right`, `.margin`, `.gw`, `.pill` (+ok/warn/bad), `.more`/`.menu`
- [ ] `.folio`, `.span-note`, `.rel` (+`.v`/`.dt`/`.lg`/`.a`)
- [ ] `.checking`/`.prog`/`.prog-n`, `.empty`
- [ ] `.list`/`.li` (installs/mods/backups), `.field`, `.themes`/`.theme-card`, `.logbox`
- [ ] `.overlay`/`.modal`/`.danger`
- [ ] `.toast`/`.toast-msg`/`.undo`
- [ ] scrollbars (accent thumb, per-theme radius)

## Deliberate deviations (do NOT "fix" these - the owner asked for them)

- **Toast:** no colored left border; `✓` prefix inline in text color on success
  toasts (`ok !== false`); Undo lightened via `color-mix(... var(--ok) 68% #fff)`
  so it reads on the dark pill. (Mockup had a hardcoded `#77b699`; the color-mix
  is the theme-adaptive equivalent - keep it.)
- **"Mods back up before update" caption:** centered under the Update-all button
  (`.ctacol{align-items:center}`), not right-aligned.
- **Dropdown:** the app uses a native `<select>`; the mockup uses a custom `.sel`
  with a verdigris status dot. Native select's open option-list resists theming.
  A separate backlog item covers replacing it with a custom dropdown; until then
  the closed control should still match theme colors.

## How to verify

1. `npm run tauri dev`, sign in, check updates on a real pack so the register is
   populated (empty-state won't exercise most rules).
2. Open `docs/mockups/almanac-design-pass-2.html` in a browser beside the app.
3. Compare header, register rows, expanded folio, pills, toolbar, toast,
   backups list, restore modal, empty/checking states. They should be
   indistinguishable on Almanac.
4. Switch to Workshop and Terminal in Settings; confirm structure is identical
   and only color/type/radius change (no broken layout).
5. `npx tsc --noEmit` and `npx vite build` clean before committing.

Commit as one focused change: "Reconcile Almanac theme to design mockups".
