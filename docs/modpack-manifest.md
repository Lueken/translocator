# Translocator modpack manifest

The format for a Translocator modpack. A pack is a **manifest document, not a zip
of jars**: it references its member mods on ModDB by file, and the launcher
resolves, downloads (from the source), verifies, and applies them at install
time. Nothing is re-hosted, so downloads and stats stay with the authors on
ModDB.

## Principles

- **ModDB-only.** Every mod in a pack must be posted on ModDB. No self-hosted or
  non-ModDB mods, for security: there is no arbitrary URL to fetch a jar from,
  and every file is a moderated ModDB release pinned by `fileid` + `sha256`.
- **The manifest is the lockfile.** The whole document at `pack.version` is one
  frozen set. A pack-managed install applies exactly this set and freezes it.
- **Publisher-driven updates only.** In a pack-managed install the user cannot
  change individual mod versions. The only update path is the publisher shipping
  a new `pack.version`, which re-resolves the whole set atomically. This is what
  keeps every player matched to the server (a mod mismatch is the most common
  reason a join fails).
- **ModDB's bytes are the hash source (locked 2026-07-24).** ModDB exposes no
  file hash, so Translocator computes one; the pin must describe what installers
  will actually receive. At pack-build time the curator downloads each pinned
  `fileid` from ModDB and hashes THOSE bytes as `sha256`, then compares against
  the publisher's local zip and warns on mismatch (local copy drifted from what
  ModDB serves). The launcher verifies every install-time download against the
  pin, like against like.
- **Converges with ModDB #54.** ModDB's own modpack direction (issue
  anegostudios/vsmoddb#54) is a manifest of ModDB-referenced members plus
  overrides/configs, with the applying half in the launcher. This format mirrors
  that shape, and `pack.moddb` + per-mod `modid` are the seam: when native
  modpacks land, a pack becomes a ModDB entry whose required-relations (#117) are
  its members, and the launcher already knows how to consume it.

## Schema (v1)

Field names that mirror ModDB are kept verbatim (`modid`, `modidstr`,
`modversion`, `fileid`, `releaseid`, `side`) so the mapping is 1:1. Our own
fields are snake_case.

```jsonc
{
  "manifest_version": 1,                 // schema version, for migration

  "pack": {
    "id": "the-quire",                   // stable slug, unique per publisher
    "name": "The Quire",
    "version": "0.3.169",                // the PACK's version; bumping = a new frozen set
    "author": "Venah",
    "summary": "Curated pack for The Quire server.",
    "description": "…markdown for the pack page…",
    "tags": ["Survival", "Exploration", "Overhaul"],
    "game_version": "1.22.3",            // the VS version this pack targets
    "min_launcher_version": "0.2.0",     // oldest Translocator that can install this pack
    "strict": true,                      // true = self-contained: manifest is the whole Mods folder
    "icon": "icon.png",                  // optional, relative asset name
    "created": "2026-07-22T00:00:00Z",
    "updated": "2026-07-22T18:00:00Z",
    "moddb": { "modid": null, "assetid": null }  // set once the pack itself is a ModDB entry (#54)
  },

  "links": {                             // all optional; power the pack page
    "website": "https://thequirevs.com",
    "discord": "https://discord.gg/…",
    "source": null,
    "donate": "https://ko-fi.com/…"
  },

  "server": {                            // OPTIONAL. Present only for server packs.
    "address": "play.thequirevs.com:42420",
    "auto_add": true                     // offer one-click add/connect after install
  },

  "mods": [
    {
      "modid": 322,                      // ModDB numeric identity (stable)
      "modidstr": "primitivesurvival",   // string id (display / search / handbook)
      "name": "Primitive Survival",      // frozen display name (renders offline)
      "modversion": "5.0.6",             // display version
      "fileid": 99106,                   // THE download pin -> /download?fileid=99106
      "releaseid": 45029,                // exact release record (links to its changelog)
      "side": "both",                    // client | server | both (from ModDB mod.side)
      "sha256": "<hash of the ModDB-served file>",  // verified on download
      "required": true                   // false = optional, user-toggleable, client-only
    }
  ],

  "overrides": [                         // publisher-owned files written into the install after mods resolve
    {
      "path": "ModConfig/PrimitiveSurvival.json",   // relative to the install dataPath; no ".." / absolute
      "encoding": "utf8",                // "utf8" (default) | "base64" (escape hatch for a binary override)
      "content": "{ \"someSetting\": true }"        // inline; keeps the pack a single doc
    }
  ]
}
```

## Field reference

### `pack` (required)

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Stable slug. Identifies the pack across versions. |
| `name` | string | Display name. |
| `version` | string | Semver-ish. The freeze key: an install records `pack.id@pack.version`. |
| `author` | string | Publisher name. |
| `summary` | string | One line for the market card. |
| `description` | string (markdown) | Pack-page body. |
| `tags` | string[] | Market filters. |
| `game_version` | string | VS version the pack targets. |
| `min_launcher_version` | string | Oldest Translocator that can install this pack. An older launcher refuses and prompts to update, so a pack using newer schema features never half-installs on a client that can't understand it. |
| `strict` | bool? | Default `false`. `true` = the pack is self-contained (locked 2026-07-24): the manifest defines the ENTIRE `Mods/` folder. The launcher refuses to install extra mods into the managed install, and each pack update sweeps foreign zips out (after a mods backup, listing what is removed). `false` = user-added mods are allowed; updates manage only pack-owned files (tracked per install) and leave the rest alone. Server packs like The Quire want `true`; content-collection packs want `false`. |
| `icon` | string? | Optional asset name for the pack logo. |
| `created` / `updated` | ISO 8601 | Timestamps. |
| `moddb` | object? | `{modid, assetid}` once the pack is a ModDB entry; both `null` until then. |

### `links` (optional)

All fields optional strings: `website`, `discord`, `source`, `donate`. A `null`
or missing link renders no button.

### `server` (optional)

Present only for server packs. `address` (host:port) and `auto_add` (bool). When
present, the launcher can add the server and offer Connect after install. A
mods-only pack omits this block entirely.

### `mods[]` (required, may be empty)

| Field | Type | Notes |
|-------|------|-------|
| `modid` | int | ModDB mod identity. Maps to a #54 required-relation. |
| `modidstr` | string | ModDB string id. |
| `name` | string | Frozen display name. |
| `modversion` | string | Display version. |
| `fileid` | int | The download pin. Resolves to `https://mods.vintagestory.at/download?fileid=<fileid>`. |
| `releaseid` | int | Exact release record; links to its changelog. |
| `side` | enum | `client` \| `server` \| `both`. |
| `sha256` | string | Lowercase hex. Computed by the curator from the ModDB-served bytes (see Principles); verified on every install-time download. |
| `required` | bool | `true` = always installed and frozen. `false` = optional (see below). |

### `overrides[]` (optional)

Publisher-owned files written into the install after mods resolve. This is the
publisher's own content (config choices, assets), **not** third-party mods, so
the ModDB-only rule does not apply here. The main use is `ModConfig/*.json`.

**Overrides are recommended defaults, not enforced state (locked 2026-07-24).**
Mods are frozen; settings are the player's. Example driving this: Quire players
who hate the Mists of Stability fog disable it in its config; a pack update must
not silently turn it back on.

- **First install:** write every override.
- **Pack update, override carried over from the previous version:** re-apply
  ONLY if the file on disk is still equivalent to the override as last applied.
  "Equivalent" is a **semantic compare for `.json`**: parse both sides,
  canonicalize (sorted keys), compare values. Byte-compare is wrong for JSON
  because mods (ConfigLib especially) re-serialize their configs on every game
  launch, reordering keys and adding defaults; a byte test would mark every
  config user-edited after one launch and the update path would go dead.
  Non-JSON overrides fall back to byte-hash compare. If either side fails to
  parse, fail safe: treat as user-edited. A user-edited file is NEVER
  overwritten; the launcher may note "pack default changed, your edit kept". A
  missing file counts as untouched.
- **Pack update, NEW override for a path the pack never shipped before:** apply
  it. Mods generate their configs at first launch, so an existing file there is
  usually generated defaults, and the publisher is declaring that config
  curated now. Safety net: the displaced file is backed up first and the update
  notes "new pack default applied, previous file backed up".
- **Pack update, override REMOVED from the manifest:** leave the file on disk,
  drop it from tracking. Deleting config out from under a player is never right.

| Field | Type | Notes |
|-------|------|-------|
| `path` | string | Relative to the install dataPath. **Must not** be absolute or contain `..`. |
| `encoding` | enum? | `utf8` (default) or `base64`. Use `base64` only for the occasional binary override; large binaries inline bloat the manifest and are discouraged. |
| `content` | string | Inline file content, decoded per `encoding`. |

## Install / resolution algorithm

Given a manifest, a pack-managed install is built as:

0. **Gate on launcher version.** If this Translocator is older than
   `pack.min_launcher_version`, refuse the install and prompt the user to update
   the launcher. This runs before anything is written.
1. **Ensure the game version.** Make sure `pack.game_version` is present in the
   shared version cache (download it if missing, via the existing
   `ensure_version` path) and pin the install to it. A pack install must never
   land on a game version it does not target.
2. **Select mods.** Take every entry where `side != "server"` (server-only mods
   are listed for the record but never loaded on the client, so they are skipped
   client-side) and (`required == true` **or** the user opted the mod in).
   Opt-in/opt-out choices are recorded (step 7) so pack updates preserve them.
3. **Stage: download + verify ALL, then place (locked 2026-07-24).** For each
   selected mod, fetch `/download?fileid=<fileid>` over HTTPS to a temp
   location; compute its SHA-256 and compare to `sha256`; confirm the file is a
   real zip (`PK` magic). Nothing touches `Mods/` until EVERY selected mod has
   downloaded and verified. On any failure, discard the stage and leave the
   install exactly as it was: a half-updated set matches neither pack version,
   which is the exact mismatch the freeze exists to prevent. On a pack update,
   take a mods backup before the swap.
4. **Place.** Swap the fully verified stage into the install's `Mods/` folder.
   On a pack update this is a reconciliation: pack-owned zips not in the new
   manifest are removed (this covers modid renames, where the old zip must go
   or both load). In a `strict` pack, foreign zips are swept too, after the
   backup, listing what is removed. In a non-strict pack, user-added zips are
   left alone.
5. **Apply overrides** per the rules in the `overrides[]` section (first
   install writes all; updates never clobber user-edited files; new overrides
   apply with a backup of the displaced file; removed overrides leave the file).
6. **Server (optional).** If a `server` block is present and `auto_add` is true,
   add the server address and surface a Connect button.
7. **Freeze.** Record in the install's `translocator.json`:
   - `managed_by: "<pack.id>@<pack.version>"`
   - the list of pack-owned mod filenames (what reconciliation manages)
   - the canonical hash of each override as applied
     (`override_hashes: { "<path>": "<hash>" }`, semantic-canonical for JSON)
   - the user's optional-mod opt-in/opt-out choices
   While `managed_by` is set, the per-mod update manager is disabled; the only
   update action is "pack update available" (see below).

## Optional mods

`required: false` marks a client-only extra the user can toggle (a minimap, an
extra HUD). Rules:

- Optional mods should be `side: "client"`. They must not affect server
  compatibility.
- Enabling an optional mod still installs the **manifest-pinned** version; the
  freeze applies to optional mods too.
- On a strictly mod-matched server, extra client mods may be rejected at join.
  That is the publisher's call: for a strict server pack, mark nothing optional.

## Updates (the freeze in practice)

- A pack has a stable manifest URL that returns its **latest** version.
- The launcher compares the installed `managed_by` version to the latest
  manifest's `pack.version`. If newer, it offers a single **"Update pack"**
  action that re-runs the resolution algorithm for the new set: stage and
  verify everything, back up, then reconcile `Mods/` (add / remove / re-version
  pack-owned zips) and apply overrides per their rules, as one operation.
- Optional-mod choices and (in non-strict packs) user-added mods survive
  updates; user-edited configs always survive updates.
- Individual per-mod updates remain available only for **personal** (non-managed)
  installs.

## What the freeze is, and is not (locked 2026-07-24)

Vintage Story has **no server-side mod verification**: the client join packet
carries no mod list, so a server never learns what a client runs (verified by
decompiling VintagestoryLib; `Packet_ClientIdentification` has no mod fields).
Any player can take a Translocator-installed folder, launch vanilla VS at the
same dataPath, and join with an altered `Mods/` folder. No client-side design
changes that. Therefore:

- **The freeze and `strict` are consistency features, not anti-cheat.** They
  guarantee that everyone who uses the launcher path has exactly the working,
  server-matched set: no version-drift crashes, no half-updated packs, no
  "why can't I join" support burden. That is their whole claim.
- **The `sha256` pins are delivery integrity.** What Translocator installs is
  byte-for-byte what the publisher pinned: no tampered downloads, no substituted
  files at install time. What a player does with the files afterward is theirs.
- **Enforcement, if a server wants it, is server-side.** The Hub exposes
  `GET /packs/:id/verify`, an authoritative lockset, exactly so a server-side
  companion mod can challenge clients against it and kick mismatches. That
  catches the casual case (renamed or drifted mods). A determined cheater
  patches the reporting client mod and lies; that is true of any non-kernel
  anti-cheat and is out of scope for the pack format.
- **The lockset is signed by the publisher, not by the Hub.** Each published
  version carries a detached Ed25519 signature over a canonical payload binding
  `pack.id`, `pack.version`, `pack.game_version`, `pack.strict`, a digest of the
  whole manifest, and every `fileid:side:sha256`. The Hub verifies on publish
  and never holds a private key, so a server admin learns *which publisher*
  froze a revision rather than only that the Hub is willing to serve it. The
  `/verify` response is self-contained and can be saved to disk, so a Hub
  outage never stops a server admitting players. Format:
  `translocator-hub/docs/pack-signing.md`.
- **`strict` is signed too**, so a pack cannot be flipped between self-contained
  and permissive in transit. A verifier uses it to decide whether mods outside
  the lockset are grounds to reject a joining client. Note the spec default is
  `false`: an omitted `strict` means extras are tolerated.

## Validation rules

A manifest is rejected if any of the following fail:

- `manifest_version` is a supported integer.
- `pack.id`, `pack.name`, `pack.version`, `pack.game_version`, and
  `pack.min_launcher_version` are non-empty.
- Every `mods[]` entry has `modid`, `fileid`, and a 64-char lowercase-hex
  `sha256`. (ModDB-only: there is no non-ModDB source, so a mod without a
  `fileid` is invalid.)
- `side` is one of `client` / `server` / `both`.
- Every `overrides[]` entry has `encoding` of `utf8` or `base64` (or omitted =
  `utf8`), and a `path` that is relative, contains no `..`, and does not resolve
  outside the install dataPath.
