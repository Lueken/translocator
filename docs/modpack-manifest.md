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
- **Curator vouches for the bytes.** ModDB exposes no file hash, so the curator
  computes `sha256` for each pinned file at pack-build time. The launcher
  verifies every download against it.
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
      "sha256": "<curator-computed>",    // verified on download
      "required": true                   // false = optional, user-toggleable, client-only
    }
  ],

  "config": [                            // overrides written into the install after mods resolve
    {
      "path": "ModConfig/PrimitiveSurvival.json",   // relative to the install dataPath
      "content": "{ \"someSetting\": true }"        // inline text; keeps the pack a single doc
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
| `sha256` | string | Lowercase hex. Curator-computed; verified on download. |
| `required` | bool | `true` = always installed and frozen. `false` = optional (see below). |

### `config[]` (optional)

Override files written into the install after mods resolve.

| Field | Type | Notes |
|-------|------|-------|
| `path` | string | Relative to the install dataPath. **Must not** be absolute or contain `..`. |
| `content` | string | Inline file text (VS configs are JSON). |

## Install / resolution algorithm

Given a manifest, a pack-managed install is built as:

1. **Select mods.** Take every entry where `side != "server"` (server-only mods
   are listed for the record but never loaded on the client, so they are skipped
   client-side) and (`required == true` **or** the user opted the mod in).
2. **Resolve + download.** For each selected mod, fetch
   `/download?fileid=<fileid>` over HTTPS. Stream to a temp file.
3. **Verify.** Compute the download's SHA-256 and compare to `sha256`. On
   mismatch, discard and fail the install for that mod. Confirm the file is a
   real zip (`PK` magic) before it lands in `Mods/`.
4. **Place.** Move verified zips into the install's `Mods/` folder.
5. **Apply config.** Write each `config[].content` to `<install>/<path>` after
   validating the path is relative and contains no `..`.
6. **Freeze.** Record `managed_by: "<pack.id>@<pack.version>"` in the install's
   `translocator.json`. While this is set, the per-mod update manager is
   disabled; the only update action is "pack update available" (see below).
7. **Server (optional).** If a `server` block is present and `auto_add` is true,
   add the server address and surface a Connect button.

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
  action that re-runs the resolution algorithm for the new set (add/remove/
  re-version mods, re-apply configs) atomically.
- Individual per-mod updates remain available only for **personal** (non-managed)
  installs.

## Validation rules

A manifest is rejected if any of the following fail:

- `manifest_version` is a supported integer.
- `pack.id`, `pack.name`, `pack.version`, `pack.game_version` are non-empty.
- Every `mods[]` entry has `modid`, `fileid`, and a 64-char lowercase-hex
  `sha256`. (ModDB-only: there is no non-ModDB source, so a mod without a
  `fileid` is invalid.)
- `side` is one of `client` / `server` / `both`.
- Every `config[].path` is relative, contains no `..`, and does not resolve
  outside the install dataPath.
