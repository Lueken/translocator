import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// ---- Types mirroring the Rust command surface (src-tauri/src/lib.rs) ----
type Account = {
  uid: string;
  playername: string;
  email: string;
  sessionkey: string;
  sessionsignature: string;
  mptoken?: string | null;
  entitlements?: unknown;
};

type LoginOutcome =
  | { status: "success"; account: Account }
  | { status: "needsTotp"; prelogintoken: string; reason?: string | null }
  | { status: "failed"; reason: string };

type InstallInfo = { name: string; path: string; has_session: boolean };

type PlayResult =
  | { status: "needsRelogin"; reason: string }
  | { status: "played"; exit_code: number; rotated: boolean; account: Account };

type ModSummary = {
  modid: number;
  modidstr: string;
  name: string;
  summary: string;
  author: string;
  downloads: number;
};

type MissingDep = { modid: string; version: string };

type Compat = "exact" | "minor" | "unlikely";
type ReleaseInfo = { modversion: string; compat: Compat; changelog: string; created: string };
type ModUpdate = {
  modid: string;
  name: string;
  assetid: number;
  installed_version: string;
  installed_filename: string;
  newer: ReleaseInfo[];
  latest_compatible: string | null;
};

type BackupInfo = { id: string; mod_count: number; created: string };

// VS Launcher's default folders on this machine (editable in the UI).
const DEFAULT_INSTALLS = "C:\\Users\\31686\\AppData\\Roaming\\VSLInstallations";
const DEFAULT_GAME_EXE =
  "C:\\Users\\31686\\AppData\\Roaming\\VSLGameVersions\\1.22.3\\Vintagestory.exe";

const COMPAT_COLOR: Record<Compat, string> = { exact: "#2a7", minor: "#e0a000", unlikely: "#c33" };
const COMPAT_LABEL: Record<Compat, string> = {
  exact: "author-tagged for your version",
  minor: "same 1.x line — should work",
  unlikely: "no matching version tag — probably incompatible",
};

const stripHtml = (s: string) =>
  s
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|li|div)>/gi, "\n")
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/\n{3,}/g, "\n\n")
    .trim();

function App() {
  const [gameExe, setGameExe] = useState(DEFAULT_GAME_EXE);
  const [installationsDir, setInstallationsDir] = useState(DEFAULT_INSTALLS);
  const [gameVersion, setGameVersion] = useState("1.22.3");

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [prelogintoken, setPrelogintoken] = useState<string | null>(null);

  const [account, setAccount] = useState<Account | null>(null);
  const [installs, setInstalls] = useState<InstallInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<string[]>([]);

  const [target, setTarget] = useState<string>("");

  // Updates
  const [updates, setUpdates] = useState<ModUpdate[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  // Backups
  const [showBackups, setShowBackups] = useState(false);
  const [backups, setBackups] = useState<BackupInfo[]>([]);

  // Browse (demoted)
  const [showBrowse, setShowBrowse] = useState(false);
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<ModSummary[]>([]);
  const [installed, setInstalled] = useState<string[]>([]);

  const say = (line: string) =>
    setLog((l) => [`${new Date().toLocaleTimeString()}  ${line}`, ...l].slice(0, 200));

  useEffect(() => {
    (async () => {
      const acct = await invoke<Account | null>("get_account");
      if (acct) {
        setAccount(acct);
        say(`Restored session for ${acct.playername}.`);
      }
      await refreshInstalls();
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!target && installs.length) setTarget(installs[0].path);
  }, [installs, target]);

  useEffect(() => {
    if (target) refreshInstalled(target);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target]);

  const toggle = (modid: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(modid)) n.delete(modid);
      else n.add(modid);
      return n;
    });

  async function doLogin() {
    setBusy(true);
    try {
      say(prelogintoken ? "Submitting 2FA code..." : "POST /v2/gamelogin ...");
      const res = await invoke<LoginOutcome>("login", {
        email,
        password,
        totp: totp || null,
        prelogintoken,
      });
      if (res.status === "success") {
        setAccount(res.account);
        setPrelogintoken(null);
        setTotp("");
        say(`Logged in as ${res.account.playername} — saved (survives restart).`);
      } else if (res.status === "needsTotp") {
        setPrelogintoken(res.prelogintoken);
        say(`2FA required${res.reason ? `: ${res.reason}` : ""}. Enter your TOTP code.`);
      } else {
        say(`Login failed: ${res.reason}`);
      }
    } catch (e) {
      say(`Error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function doLogout() {
    await invoke("logout");
    setAccount(null);
    say("Logged out — cleared saved session.");
  }

  async function refreshInstalls() {
    try {
      const list = await invoke<InstallInfo[]>("list_installs", { installationsDir });
      setInstalls(list);
      say(`Found ${list.length} installation(s).`);
    } catch (e) {
      say(`Error listing installs: ${e}`);
    }
  }

  async function refreshInstalled(installDir: string) {
    try {
      setInstalled(await invoke<string[]>("list_mod_files", { installDir }));
    } catch {
      setInstalled([]);
    }
  }

  async function doPlay(inst: InstallInfo) {
    if (!account) return;
    setBusy(true);
    try {
      say(`▶ ${inst.name}: validate → stamp → launch ...`);
      const res = await invoke<PlayResult>("play", {
        gameExe,
        installDir: inst.path,
        account,
        startParams: null,
      });
      if (res.status === "needsRelogin") {
        say(`✗ Session rejected by server (${res.reason}). Re-login needed.`);
        setAccount(null);
      } else {
        say(`■ ${inst.name} exited (code ${res.exit_code}).`);
        if (res.rotated) {
          setAccount(res.account);
          say("↻ Game rotated the session — captured + saved the new key (this is the fix).");
        } else {
          say("✓ Session unchanged and still valid — no logout loop.");
        }
      }
    } catch (e) {
      say(`Error launching: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  // ---- Updates ----
  async function checkUpdates() {
    if (!target) {
      say("Pick a target installation first.");
      return;
    }
    setBusy(true);
    try {
      const name = installs.find((i) => i.path === target)?.name ?? target;
      say(`Checking updates for ${name} (game ${gameVersion}) ...`);
      const ups = await invoke<ModUpdate[]>("check_updates", { installDir: target, gameVersion });
      setUpdates(ups);
      say(`${ups.length} mod(s) have newer releases.`);
    } catch (e) {
      say(`Update check error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function installVersion(u: ModUpdate, modversion: string) {
    setBusy(true);
    try {
      say(`Updating ${u.name} → ${modversion} ...`);
      await invoke<string>("install_release", {
        installDir: target,
        modidstr: u.modid,
        modversion,
        oldFilename: u.installed_filename,
      });
      say(`✓ ${u.name} now at ${modversion}.`);
      setUpdates((prev) => prev.filter((x) => x.modid !== u.modid)); // optimistic
      await refreshInstalled(target);
    } catch (e) {
      say(`Update error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function updateAllLatest() {
    const targets = updates.filter((u) => u.latest_compatible);
    if (!targets.length) {
      say("No compatible updates to apply.");
      return;
    }
    setBusy(true);
    try {
      try {
        const id = await invoke<string>("backup_mods", { installDir: target });
        say(`Backed up ${installed.length} mods before updating (id ${id}).`);
        await refreshBackups(target);
      } catch (e) {
        say(`⚠ Backup failed (${e}) — continuing with update.`);
      }
      say(`Updating ${targets.length} mod(s) to latest compatible ...`);
      for (const u of targets) {
        try {
          await invoke<string>("install_release", {
            installDir: target,
            modidstr: u.modid,
            modversion: u.latest_compatible,
            oldFilename: u.installed_filename,
          });
          say(`  ✓ ${u.name} → ${u.latest_compatible}`);
        } catch (e) {
          say(`  ✗ ${u.name}: ${e}`);
        }
      }
      await refreshInstalled(target);
      await checkUpdates();
    } finally {
      setBusy(false);
    }
  }

  async function refreshBackups(installDir: string) {
    try {
      setBackups(await invoke<BackupInfo[]>("list_backups", { installDir }));
    } catch {
      setBackups([]);
    }
  }

  async function doRestore(id: string) {
    if (!target) return;
    setBusy(true);
    try {
      say(`Restoring backup ${id} ...`);
      await invoke("restore_backup", { installDir: target, id });
      say(`✓ Restored Mods folder to backup ${id}.`);
      await refreshInstalled(target);
      await checkUpdates();
    } catch (e) {
      say(`Restore error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function openModDB(assetid: number) {
    try {
      await invoke("open_url", { url: `https://mods.vintagestory.at/show/mod/${assetid}` });
    } catch (e) {
      say(`Open error: ${e}`);
    }
  }

  // ---- Browse (demoted) ----
  async function doSearch() {
    setBusy(true);
    try {
      say(`Searching ModDB for "${search}" ...`);
      setResults(await invoke<ModSummary[]>("search_mods", { text: search }));
    } catch (e) {
      say(`Search error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function doInstall(m: ModSummary) {
    if (!target) {
      say("Pick a target installation first.");
      return;
    }
    setBusy(true);
    try {
      const name = installs.find((i) => i.path === target)?.name ?? target;
      say(`Installing "${m.name}" into ${name} ...`);
      const fname = await invoke<string>("install_mod", { installDir: target, modidstr: m.modidstr });
      say(`✓ Installed ${fname}.`);
      await resolveDeps(target, fname, new Set([m.modidstr.toLowerCase()]));
      await refreshInstalled(target);
    } catch (e) {
      say(`Install error: ${e}`);
    } finally {
      setBusy(false);
    }
  }

  async function resolveDeps(installDir: string, filename: string, seen: Set<string>) {
    const missing = await invoke<MissingDep[]>("check_deps", { installDir, filename });
    for (const dep of missing) {
      if (seen.has(dep.modid)) continue;
      seen.add(dep.modid);
      say(`  ↳ missing dependency ${dep.modid}${dep.version ? ` (${dep.version})` : ""} — installing...`);
      try {
        const f = await invoke<string>("install_mod", { installDir, modidstr: dep.modid });
        say(`  ✓ installed dependency ${f}`);
        await resolveDeps(installDir, f, seen);
      } catch (e) {
        say(`  ✗ dependency ${dep.modid} not installable: ${e}`);
      }
    }
  }

  async function doTip(m: ModSummary) {
    try {
      const links = await invoke<string[]>("mod_donations", { modidstr: m.modidstr });
      if (links.length) say(`♥ Tip ${m.name}: ${links.join("   ")}`);
      else say(`${m.name}: no donation link found.`);
    } catch (e) {
      say(`Tip lookup error: ${e}`);
    }
  }

  const box: React.CSSProperties = { border: "1px solid #ccc", borderRadius: 8, padding: 16, marginBottom: 16 };
  const input: React.CSSProperties = { width: "100%", padding: 6, marginTop: 4 };

  return (
    <main style={{ maxWidth: 760, margin: "0 auto", padding: 24, fontFamily: "system-ui" }}>
      <h1 style={{ marginBottom: 4 }}>⚙ Translocator</h1>
      <p style={{ color: "#666", marginTop: 0 }}>Prototype — reliable carryover + mod update manager</p>

      <section style={box}>
        <strong>Paths</strong>
        <label style={{ display: "block", marginTop: 8 }}>
          Game executable
          <input style={input} value={gameExe} onChange={(e) => setGameExe(e.target.value)} />
        </label>
        <label style={{ display: "block", marginTop: 8 }}>
          Installations folder
          <input style={input} value={installationsDir} onChange={(e) => setInstallationsDir(e.target.value)} />
        </label>
      </section>

      <section style={box}>
        <strong>Account</strong>
        {account ? (
          <div style={{ marginTop: 8 }}>
            Logged in as <b>{account.playername}</b> <button onClick={doLogout}>Log out</button>
          </div>
        ) : (
          <div style={{ marginTop: 8 }}>
            <input style={input} placeholder="email" value={email} onChange={(e) => setEmail(e.target.value)} />
            <input
              style={input}
              type="password"
              placeholder="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            {prelogintoken && (
              <input style={input} placeholder="2FA / TOTP code" value={totp} onChange={(e) => setTotp(e.target.value)} />
            )}
            <button style={{ marginTop: 8 }} disabled={busy} onClick={doLogin}>
              {prelogintoken ? "Submit 2FA code" : "Log in"}
            </button>
          </div>
        )}
      </section>

      <section style={box}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
          <strong>Installations</strong>
          <button onClick={refreshInstalls}>Refresh</button>
        </div>
        {installs.length === 0 ? (
          <p style={{ color: "#888" }}>None loaded — hit Refresh.</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0 }}>
            {installs.map((inst) => (
              <li
                key={inst.path}
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  padding: "6px 0",
                  borderBottom: "1px solid #eee",
                }}
              >
                <span>
                  {inst.name}{" "}
                  <small style={{ color: inst.has_session ? "#2a7" : "#999" }}>
                    {inst.has_session ? "● session" : "○ no session"}
                  </small>
                </span>
                <button disabled={!account || busy} onClick={() => doPlay(inst)}>
                  Play
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* ---------- Mod updates: the star ---------- */}
      <section style={box}>
        <strong>Mod updates</strong>
        <div style={{ display: "flex", gap: 8, alignItems: "center", margin: "8px 0", flexWrap: "wrap" }}>
          <select value={target} onChange={(e) => setTarget(e.target.value)}>
            {installs.map((i) => (
              <option key={i.path} value={i.path}>
                {i.name}
              </option>
            ))}
          </select>
          <label style={{ fontSize: 13 }}>
            Game{" "}
            <input style={{ width: 72, padding: 4 }} value={gameVersion} onChange={(e) => setGameVersion(e.target.value)} />
          </label>
          <button disabled={busy || !target} onClick={checkUpdates}>
            Check for updates
          </button>
          {updates.length > 0 && (
            <button disabled={busy} onClick={updateAllLatest}>
              Update all compatible
            </button>
          )}
        </div>

        {updates.length === 0 ? (
          <p style={{ color: "#888" }}>Pick an installation and check for updates.</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0 }}>
            {updates.map((u) => (
              <li key={u.modid} style={{ padding: "8px 0", borderBottom: "1px solid #eee" }}>
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <button onClick={() => toggle(u.modid)} style={{ width: 26 }} title="Show version notes">
                    {expanded.has(u.modid) ? "▾" : "▸"}
                  </button>
                  <span style={{ flex: 1 }}>
                    <b>{u.name}</b>{" "}
                    <small style={{ color: "#888" }}>
                      {u.installed_version} → {u.latest_compatible ?? `${u.newer[0].modversion} (incompatible)`}
                    </small>
                  </span>
                  {u.latest_compatible ? (
                    <span style={{ color: "#2a7", fontSize: 12 }} title="a compatible update exists">
                      ● compatible
                    </span>
                  ) : (
                    <span style={{ color: "#c33", fontSize: 12 }} title="only incompatible updates found">
                      ● incompatible
                    </span>
                  )}
                  <button onClick={() => openModDB(u.assetid)} title="Open on ModDB">
                    ↗
                  </button>
                </div>

                {expanded.has(u.modid) && (
                  <ul style={{ listStyle: "none", paddingLeft: 34, marginTop: 6 }}>
                    {u.newer.map((r) => (
                      <li key={r.modversion} style={{ padding: "6px 0", borderTop: "1px dashed #eee" }}>
                        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <span style={{ color: COMPAT_COLOR[r.compat] }} title={COMPAT_LABEL[r.compat]}>
                            ●
                          </span>
                          <b style={{ flex: 1 }}>{r.modversion}</b>
                          <small style={{ color: "#999" }}>{r.created?.slice(0, 10)}</small>
                          <button disabled={busy} onClick={() => installVersion(u, r.modversion)}>
                            Install this
                          </button>
                        </div>
                        {r.changelog && (
                          <pre
                            style={{
                              whiteSpace: "pre-wrap",
                              fontSize: 11,
                              color: "#555",
                              margin: "4px 0 0",
                              maxHeight: 160,
                              overflow: "auto",
                            }}
                          >
                            {stripHtml(r.changelog) || "(no notes)"}
                          </pre>
                        )}
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ul>
        )}

        {/* ---------- Backups ---------- */}
        <div style={{ marginTop: 12, borderTop: "1px solid #eee", paddingTop: 10 }}>
          <button
            onClick={() => {
              const next = !showBackups;
              setShowBackups(next);
              if (next && target) refreshBackups(target);
            }}
            style={{ background: "none", border: "none", padding: 0, cursor: "pointer", fontWeight: 600, fontSize: 13 }}
          >
            {showBackups ? "▾" : "▸"} Backups{backups.length ? ` (${backups.length})` : ""}
          </button>
          {showBackups && (
            <>
              <div style={{ margin: "6px 0" }}>
                <button
                  disabled={busy || !target}
                  onClick={async () => {
                    if (!target) return;
                    setBusy(true);
                    try {
                      const id = await invoke<string>("backup_mods", { installDir: target });
                      say(`Backed up ${installed.length} mods (id ${id}).`);
                      await refreshBackups(target);
                    } catch (e) {
                      say(`Backup error: ${e}`);
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  Back up now
                </button>
              </div>
              {backups.length === 0 ? (
                <p style={{ color: "#888", fontSize: 12, margin: "4px 0" }}>
                  No backups yet — one is taken automatically before "Update all compatible".
                </p>
              ) : (
                <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
                  {backups.map((b) => (
                    <li
                      key={b.id}
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "center",
                        padding: "5px 0",
                        borderBottom: "1px solid #eee",
                        fontSize: 12,
                      }}
                    >
                      <span>
                        <b>{b.created}</b>{" "}
                        <small style={{ color: "#888" }}>
                          {b.mod_count} mod{b.mod_count === 1 ? "" : "s"} · {b.id}
                        </small>
                      </span>
                      <button disabled={busy} onClick={() => doRestore(b.id)} title="Replace Mods folder with this snapshot">
                        Restore
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      </section>

      {/* ---------- Browse & install: demoted ---------- */}
      <section style={box}>
        <button
          onClick={() => setShowBrowse((v) => !v)}
          style={{ background: "none", border: "none", padding: 0, cursor: "pointer", fontWeight: 600 }}
        >
          {showBrowse ? "▾" : "▸"} Browse &amp; install mods
        </button>
        {showBrowse && (
          <>
            <form
              style={{ display: "flex", gap: 8, marginTop: 8 }}
              onSubmit={(e) => {
                e.preventDefault();
                doSearch();
              }}
            >
              <input
                style={{ flex: 1, padding: 6 }}
                placeholder="search mods..."
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <button type="submit" disabled={busy}>
                Search
              </button>
            </form>
            {results.length > 0 && (
              <ul style={{ listStyle: "none", padding: 0, marginTop: 8, maxHeight: 220, overflow: "auto" }}>
                {results.map((m) => (
                  <li
                    key={m.modid}
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      padding: "6px 0",
                      borderBottom: "1px solid #eee",
                    }}
                  >
                    <span style={{ flex: 1 }}>
                      <b>{m.name}</b>{" "}
                      <small style={{ color: "#888" }}>
                        by {m.author} · {m.downloads.toLocaleString()} dl
                      </small>
                    </span>
                    <span style={{ display: "flex", gap: 6 }}>
                      <button onClick={() => doTip(m)} title="Show author donation link">
                        ♥
                      </button>
                      <button disabled={busy || !target} onClick={() => doInstall(m)}>
                        Install
                      </button>
                    </span>
                  </li>
                ))}
              </ul>
            )}
            <div style={{ marginTop: 12 }}>
              <small style={{ color: "#666" }}>Installed in target ({installed.length}):</small>
              {installed.length > 0 && (
                <ul style={{ margin: "4px 0 0", paddingLeft: 18, fontSize: 12, color: "#444" }}>
                  {installed.map((f) => (
                    <li key={f}>{f}</li>
                  ))}
                </ul>
              )}
            </div>
          </>
        )}
      </section>

      <section style={box}>
        <strong>Log</strong>
        <pre
          style={{
            marginTop: 8,
            maxHeight: 220,
            overflow: "auto",
            background: "#111",
            color: "#ddd",
            padding: 12,
            borderRadius: 6,
            fontSize: 12,
          }}
        >
          {log.join("\n") || "…"}
        </pre>
      </section>
    </main>
  );
}

export default App;
