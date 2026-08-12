# Translocator

A Vintage Story launcher by Lueken Good Design LLC. Manages accounts, game
versions, installations, mods, and modpacks from one place, with an optional
locally-built optimized client.

Not affiliated with Anego Studios. Downloads game binaries only from official
servers and mods only from the official Mod Database, always at the source, so
download counts and stats stay with the authors. Nothing is re-hosted.

## What it does

- **Login that sticks.** Validate before stamp, read back after exit. The
  session rotation that logs other launchers out is handled; sign in once.
  The login wall doubles as the ownership gate: the game's own auth refuses
  accounts without a purchase.
- **Installations.** Isolated dataPath folders with per-install metadata
  (version pin, launch params, backups, playtime). Adopts VS Launcher and
  StoryForge installs in place.
- **Shared version cache.** Each game version downloads once and serves every
  installation pinned to it.
- **Mods.** ModDB search with in-app discovery (descriptions, latest release),
  dependency resolution, and a changelog-driven update manager with three-tier
  compatibility.
- **Modpack Hub.** Packs are signed manifests, not zip bundles: every mod is
  pinned by ModDB fileid + sha256 and downloaded from the source at install
  time. Publishing signs with an Ed25519 device key bound to the publisher's
  VS account; installing verifies every byte, then freezes the install to the
  pack version. Spec: `docs/modpack-manifest.md`.
- **Servers and worlds.** Public server browser with direct join, saved private
  servers, world listing with metadata, whole-install and mods-only backups.
- **Optimum (optional).** Builds Zaldaryon's performance-optimized client
  locally, one build per game version, with automatic fallback to vanilla.
  Toolchain (user-local .NET 10 SDK, MinGit, ilspycmd) provisions itself at
  first build. Separately licensed; its own notice is shown before anything
  runs.
- **Self-updating.** tauri-plugin-updater against
  `translocator.app/launcher/latest.json` (fallback `thequirevs.com`), signed
  releases only.

## Layout

- `src/` React + TypeScript frontend (single `App.tsx`, three themes in
  `themes.css`)
- `src-tauri/src/` Rust backend, one module per concern: `auth`, `session`,
  `installations`, `versions`, `mods`, `updates`, `deps`, `backup`, `worlds`,
  `servers`, `hub`, `curator`, `pack_install`, `signing`, `optimum`,
  `migrate`, `store`
- `docs/` format specs
- The Hub (pack registry, `api.translocator.app`) lives in the separate
  `translocator-hub` repo.

## Development

```
npm install
npm run tauri dev
```

Sign in with a real VS account; the backend refuses every command without a
stored session. Devtools (F12) are enabled in release builds during beta.

## Releasing

1. Bump the version in BOTH `package.json` and `src-tauri/tauri.conf.json`.
   Never reuse a version number for a changed build.
2. Build signed:
   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$env:USERPROFILE\.tauri\translocator.key" -Raw
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "..."
   npm run tauri build
   ```
3. Upload the NSIS installer + `.sig` through the admin uploader; it
   regenerates the updater manifest. Installed copies offer the update on
   next launch.

## License

The launcher is free software under **GPL-3.0** (see LICENSE). Fork it, change
it, redistribute it, sell it. The only obligation is the GPL's own: a modified
version you distribute stays GPL-3.0 and ships its source.

**What that covers, precisely.** This code: the source in this repository and
binaries built from it. It does not restrict anyone from writing a Vintage
Story launcher, and could not; copyright protects expression, not ideas. A
dozen launchers already exist and more are welcome. Read this repo, understand
an approach, write your own implementation, and it is yours. What GPL-3.0
governs is copying from this codebase.

**The Modpack Hub is separate** and not covered here. Its source is not
published. The launcher reaches it over a public read API, and nothing stops
anyone running a hub of their own.

This replaced GPL-3.0 with the Commons Clause, which was not open source and
did not hold together: GPLv3 section 7 lets a recipient strip any term that is
a further restriction, and a no-sell clause bolted onto a licence that
expressly permits selling is precisely that. The clause is gone rather than
redrafted, because any noncompete strong enough to stop a competitor also stops
the community fork, and for a one-maintainer project that is the worse risk.

Official builds are produced and signed only by Lueken Good Design LLC.
Contributions are accepted under a Developer Certificate of Origin, no CLA and
no copyright assignment; see CONTRIBUTING.md. Optimum is an independent project
under its own license (GPL-3.0 with the Commons Clause) and is never bundled or
redistributed here.
