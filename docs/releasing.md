# Releasing

Cutting a Translocator release, and the two things that have already gone wrong
doing it.

## The updater keypair

The launcher only installs an update whose signature verifies against the
public key **compiled into the running build** (`plugins.updater.pubkey` in
`src-tauri/tauri.conf.json`). That has a consequence worth stating plainly
before anything else on this page:

> Losing the private key or its password means every already-installed launcher
> can never auto-update again. A new keypair is easy to make; convincing the
> installs that already trust the old one is impossible. They have to be
> reinstalled by hand.

Current keypair, live from 0.3.0:

| | |
|---|---|
| Key id | `AC05D28717E91DA` |
| Public key | `RWTakX5xKF3ACiDepEiPOX15FNzcTlSpG7J/Qrlwd8ELGHOWb5PigLEu` |
| Private key | `~/.tauri/translocator-v2.key` (encrypted) |
| Password | password manager, entry "Translocator updater key (v2)" |

Previous keypair `1BA047F738F5753C` (`~/.tauri/translocator.key`) was retired
on 2026-08-11 because its password was recorded incorrectly and could not be
recovered. Builds 0.2.x trust that key and cannot update to 0.3.0; those installs
need a manual reinstall once. The old key files are kept rather than deleted, in
case the password ever turns up and someone wants to verify an old release.

**When rotating, save the password before you do anything else with the key.**
That is the entire lesson of the 0.3.0 release.

## Building

The signing values come from environment variables, and **the build and the
`signer sign` CLI do not read the same ones**. Getting this wrong cost two
failed attempts on the 0.3.0 release, in both directions.

| | `tauri build` | `tauri signer sign` |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | **this one.** Takes the key contents *or* a path | maps to `-k`, key contents |
| `TAURI_SIGNING_PRIVATE_KEY_PATH` | ignored | maps to `-f`, a path |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | yes | yes |

Two consequences:

- The bundler only looks at `TAURI_SIGNING_PRIVATE_KEY`. Setting only
  `_PATH` fails with "a public key has been found, but no private key",
  which sounds like a missing key rather than a misnamed variable.
- `signer sign` refuses `-f` while `TAURI_SIGNING_PRIVATE_KEY` is set in the
  shell, because that env var *is* `-k` and the two conflict. Clear it before
  probing.

Building, in PowerShell:

```powershell
Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PATH -ErrorAction SilentlyContinue
$env:TAURI_SIGNING_PRIVATE_KEY = "$HOME\.tauri\translocator-v2.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = 'single quotes, always'
npm run tauri build
```

Single quotes are not a style preference. PowerShell interpolates `$` and
backtick inside double quotes, so a password containing either is silently
mangled into something else and the failure reads as a wrong password.

**Check the password before committing to a full build.** Signing a scratch
file takes two seconds and a build takes minutes:

```powershell
Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
"probe" | Out-File -Encoding ascii $env:TEMP\sigtest.txt
npx tauri signer sign -f "$HOME\.tauri\translocator-v2.key" -p 'password' $env:TEMP\sigtest.txt
```

A `.sig` is not proof on its own; it proves *a* key signed, not the key the app
trusts. To confirm the signature verifies under the public key actually
compiled into the build, decode `plugins.updater.pubkey` and the `.sig` (both
are base64-wrapped) and check the 8-byte key ids in each match. Tauri signs
prehashed, so the signed message is `blake2b512(file)`, not the file.

**Check the `.sig` exists afterwards.** With the signing variables unset, the
build succeeds and simply produces no signature, with no error. The updater
artifacts land in:

```
src-tauri/target/release/bundle/nsis/translocator_<version>_x64-setup.exe
src-tauri/target/release/bundle/nsis/translocator_<version>_x64-setup.exe.sig
```

## Version numbers

Three files carry the version and they feed different things:
`src-tauri/tauri.conf.json` names the installer and the updater manifest,
`package.json` is the frontend, and `src-tauri/Cargo.toml` is what
`env!("CARGO_PKG_VERSION")` compiles in. That last one is the number the
`min_launcher_version` pack gate compares against and the version the Hub logs
for each client.

CI fails when they disagree (`.github/workflows/ci.yml`, "versions must agree").
They had silently sat at 0.2.4 and 0.1.0 through four releases before that check
existed.

Bump the minor for a change that breaks pack compatibility in either direction,
since `min_launcher_version` is how a pack refuses an older launcher and the
number needs to read as significant.

## Publishing

1. Upload the installer and its `.sig` through the release form on
   translocator.app, which writes `/launcher/latest.json`.
2. Confirm `https://translocator.app/launcher/latest.json` serves and names the
   new version. A 404 there means installed launchers fall through to the
   `thequirevs.com` endpoint.
3. Attach the installer to the GitHub release with its SHA-256 in the notes.
