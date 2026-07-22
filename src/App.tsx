import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./themes.css";

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
type ModSummary = { modid: number; modidstr: string; name: string; summary: string; author: string; downloads: number };
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
type Theme = "almanac" | "workshop" | "terminal";
type View = "installations" | "updates" | "mods" | "account" | "settings";

const DEFAULT_INSTALLS = "C:\\Users\\31686\\AppData\\Roaming\\VSLInstallations";
const DEFAULT_GAME_EXE = "C:\\Users\\31686\\AppData\\Roaming\\VSLGameVersions\\1.22.3\\Vintagestory.exe";

const THEMES: { id: Theme; name: string; desc: string; colors: string[] }[] = [
  { id: "almanac", name: "Temporal Almanac", desc: "Parchment ledger, serif", colors: ["#0E1512", "#E9DDC4", "#B26A22", "#3C7A5C"] },
  { id: "workshop", name: "Blued Workshop", desc: "Gunmetal + cyan charge", colors: ["#13171D", "#1B242D", "#C77B3B", "#74D6C8"] },
  { id: "terminal", name: "Field Terminal", desc: "Amber phosphor, mono", colors: ["#141210", "#1A1610", "#E8B24A", "#83BE9E"] },
];
const COMPAT_LABEL: Record<Compat, string> = {
  exact: "author-tagged for your version",
  minor: "same 1.x line — should work",
  unlikely: "no matching version tag — probably incompatible",
};
const compatClass = (c: Compat) => (c === "exact" ? "ok" : c === "minor" ? "warn" : "bad");
const entryStatus = (u: ModUpdate): { cls: string; label: string } => {
  if (!u.latest_compatible) return { cls: "bad", label: "Needs newer game" };
  const rel = u.newer.find((r) => r.modversion === u.latest_compatible);
  return rel?.compat === "exact" ? { cls: "ok", label: "Compatible" } : { cls: "warn", label: "Should work" };
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

const Gear = () => (
  <svg className="gear" viewBox="0 0 100 100" aria-hidden="true">
    <g fill="currentColor">
      {[0, 45, 90, 135, 180, 225, 270, 315].map((a) => (
        <rect key={a} x="44" y="1" width="12" height="22" rx="3" transform={`rotate(${a} 50 50)`} />
      ))}
      <circle cx="50" cy="50" r="30" />
    </g>
    <circle cx="50" cy="50" r="13" fill="var(--side-bg)" />
  </svg>
);
const GearMark = () => (
  <svg className="gw" viewBox="0 0 24 24">
    <path fill="currentColor" d="M13.3 2h-2.6l-.4 2.2a8 8 0 0 0-1.7.7L6.7 3.6 4.9 5.4l1.3 1.9a8 8 0 0 0-.7 1.7L3.3 9.4v2.6l2.2.4c.2.6.4 1.1.7 1.7l-1.3 1.9 1.8 1.8 1.9-1.3c.5.3 1.1.5 1.7.7l.4 2.2h2.6l.4-2.2c.6-.2 1.2-.4 1.7-.7l1.9 1.3 1.8-1.8-1.3-1.9c.3-.5.5-1.1.7-1.7l2.2-.4V9.4l-2.2-.4a8 8 0 0 0-.7-1.7l1.3-1.9-1.8-1.8-1.9 1.3a8 8 0 0 0-1.7-.7L13.3 2zM12 15a3 3 0 1 1 0-6 3 3 0 0 1 0 6z" />
  </svg>
);
const Chevron = ({ open }: { open: boolean }) => (
  <svg viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.6" fill="none" style={open ? { transform: "rotate(90deg)" } : undefined}>
    <path d="M4 2l4 4-4 4" />
  </svg>
);

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

  const [updates, setUpdates] = useState<ModUpdate[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [showBackups, setShowBackups] = useState(false);
  const [backups, setBackups] = useState<BackupInfo[]>([]);

  const [search, setSearch] = useState("");
  const [results, setResults] = useState<ModSummary[]>([]);
  const [installed, setInstalled] = useState<string[]>([]);

  const [view, setView] = useState<View>("updates");
  const [theme, setTheme] = useState<Theme>(() => (localStorage.getItem("tl-theme") as Theme) || "almanac");

  const say = (line: string) => setLog((l) => [`${new Date().toLocaleTimeString()}  ${line}`, ...l].slice(0, 200));

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("tl-theme", theme);
  }, [theme]);

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
      const res = await invoke<LoginOutcome>("login", { email, password, totp: totp || null, prelogintoken });
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
      const res = await invoke<PlayResult>("play", { gameExe, installDir: inst.path, account, startParams: null });
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
      await invoke<string>("install_release", { installDir: target, modidstr: u.modid, modversion, oldFilename: u.installed_filename });
      say(`✓ ${u.name} now at ${modversion}.`);
      setUpdates((prev) => prev.filter((x) => x.modid !== u.modid));
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
          await invoke<string>("install_release", { installDir: target, modidstr: u.modid, modversion: u.latest_compatible, oldFilename: u.installed_filename });
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
  async function backupNow() {
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
  }
  async function openModDB(assetid: number) {
    try {
      await invoke("open_url", { url: `https://mods.vintagestory.at/show/mod/${assetid}` });
    } catch (e) {
      say(`Open error: ${e}`);
    }
  }
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

  const targetName = installs.find((i) => i.path === target)?.name ?? "—";
  const NAV: { id: View; label: string; count?: number }[] = [
    { id: "installations", label: "Installations", count: installs.length },
    { id: "updates", label: "Updates", count: updates.length || undefined },
    { id: "mods", label: "Mods", count: installed.length || undefined },
    { id: "account", label: "Account" },
    { id: "settings", label: "Settings" },
  ];

  return (
    <div className="layout">
      <aside className="side">
        <div className="brand">
          <Gear />
          <div>
            <div className="brand-name">Translocator</div>
            <div className="brand-sub">{THEMES.find((t) => t.id === theme)?.name}</div>
          </div>
        </div>
        <div className="hr" />
        <button className="acct-chip" style={{ background: "none", border: "none", cursor: "pointer" }} onClick={() => setView("account")}>
          <div className="wax">{account ? account.playername[0]?.toUpperCase() : "?"}</div>
          <div style={{ textAlign: "left" }}>
            <div className="who">{account ? account.playername : "Sign in"}</div>
            <div className="signed">{account ? <><span className="dotv" />Signed in</> : "Not signed in"}</div>
          </div>
        </button>
        <div className="hr" />
        <div className="idx">Index</div>
        <nav>
          {NAV.map((n) => (
            <button key={n.id} className={"navbtn" + (view === n.id ? " active" : "")} onClick={() => setView(n.id)}>
              <span className="l">{n.label}</span>
              <span className="c">{n.count ?? ""}</span>
            </button>
          ))}
        </nav>
        <div className="side-foot">
          <button onClick={() => setView("settings")}>Settings</button>
          <button onClick={() => invoke("open_url", { url: "https://mods.vintagestory.at" }).catch(() => {})} title="Open ModDB">ModDB</button>
        </div>
      </aside>

      <div className="main">
        {/* ---------------- UPDATES ---------------- */}
        {view === "updates" && (
          <>
            <div className="topbar">
              <div>
                <div className="eyebrow">Ledger of Changes</div>
                <h1 className="title">{targetName}</h1>
              </div>
              <span className="grow" />
              <select value={target} onChange={(e) => setTarget(e.target.value)}>
                {installs.map((i) => (
                  <option key={i.path} value={i.path}>{i.name}</option>
                ))}
              </select>
              <span className="pchip">Game <b>{gameVersion}</b></span>
              <button className="btn" disabled={busy || !target} onClick={checkUpdates}>Check for updates</button>
              {updates.length > 0 && (
                <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", gap: 3 }}>
                  <button className="cta" disabled={busy} onClick={updateAllLatest}>Update all compatible</button>
                  <span style={{ fontSize: 11, color: "var(--fg-muted)" }}>Mods back up before update</span>
                </div>
              )}
            </div>
            <div className="view">
              {updates.length > 0 && (
                <div className="summary">
                  <span className="say"><b>{updates.length}</b> of {installed.length || "—"} mods have newer releases</span>
                  <span className="legend">
                    <span><span className="sw" style={{ background: "var(--ok)" }} />Compatible</span>
                    <span><span className="sw" style={{ background: "var(--warn)" }} />Should work</span>
                    <span><span className="sw" style={{ background: "var(--bad)" }} />Needs newer game</span>
                  </span>
                </div>
              )}
              {updates.length === 0 ? (
                <p className="muted">Pick an installation and check for updates.</p>
              ) : (
                <div className="register">
                  {updates.map((u, i) => {
                    const st = entryStatus(u);
                    const open = expanded.has(u.modid);
                    return (
                      <div key={u.modid}>
                        <div className="entry" style={open ? { background: "var(--panel-hover)" } : undefined}>
                          <div className="lineno">
                            <button className="chev" onClick={() => toggle(u.modid)} aria-expanded={open} title="Show version notes">
                              <Chevron open={open} />
                            </button>
                            <span className="n">{String(i + 1).padStart(2, "0")}</span>
                          </div>
                          <div>
                            <div className="mname">{u.name}</div>
                            <div className="mid">{u.modid}</div>
                          </div>
                          <div className="right">
                            <span className="margin">
                              <span className="from tab">{u.installed_version}</span>
                              <GearMark />
                              <span className="to tab">{u.latest_compatible ?? u.newer[0].modversion}</span>
                            </span>
                            <span className={"pill " + st.cls}><span className="d" />{st.label}</span>
                            <button className="link" onClick={() => openModDB(u.assetid)}>ModDB ↗</button>
                            <button className={st.cls === "bad" ? "mini" : "cta"} disabled={busy || st.cls === "bad"} onClick={() => u.latest_compatible && installVersion(u, u.latest_compatible)}>
                              {st.cls === "bad" ? "Update" : `Update`}
                            </button>
                          </div>
                        </div>
                        {open && (
                          <div className="folio">
                            {u.newer.map((r) => (
                              <div className="rel" key={r.modversion}>
                                <div className={"v " + compatClass(r.compat)} title={COMPAT_LABEL[r.compat]}>
                                  <span className="d" />{r.modversion}
                                </div>
                                <div className="dt">{r.created?.slice(0, 10)}</div>
                                <p className="lg">{stripHtml(r.changelog) || "(no notes)"}</p>
                                <button className="mini a" disabled={busy} onClick={() => installVersion(u, r.modversion)}>Install this version</button>
                              </div>
                            ))}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}

              {/* backups */}
              <div style={{ marginTop: 16 }}>
                <button className="link" onClick={() => { const next = !showBackups; setShowBackups(next); if (next && target) refreshBackups(target); }}>
                  {showBackups ? "▾" : "▸"} Backups{backups.length ? ` (${backups.length})` : ""}
                </button>
                {showBackups && (
                  <div style={{ marginTop: 8 }}>
                    <button className="btn" disabled={busy || !target} onClick={backupNow} style={{ marginBottom: 8 }}>Back up now</button>
                    {backups.length === 0 ? (
                      <p className="muted">No backups yet — one is taken automatically before "Update all compatible".</p>
                    ) : (
                      <div className="list">
                        {backups.map((b) => (
                          <div className="li" key={b.id}>
                            <span><b className="nm">{b.created}</b> <span className="meta">{b.mod_count} mod{b.mod_count === 1 ? "" : "s"} · {b.id}</span></span>
                            <button className="mini" disabled={busy} onClick={() => doRestore(b.id)} title="Replace Mods folder with this snapshot">Restore</button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            </div>
          </>
        )}

        {/* ---------------- INSTALLATIONS ---------------- */}
        {view === "installations" && (
          <>
            <div className="topbar">
              <div><div className="eyebrow">Installations</div><h1 className="title">Your installations</h1></div>
              <span className="grow" />
              <button className="btn" onClick={refreshInstalls}>Refresh</button>
            </div>
            <div className="view">
              {installs.length === 0 ? (
                <p className="muted">None found — set your installations folder in Settings, then Refresh.</p>
              ) : (
                <div className="list">
                  {installs.map((inst) => (
                    <div className="li" key={inst.path}>
                      <span>
                        <span className="nm">{inst.name}</span>{" "}
                        <span className="meta" style={{ color: inst.has_session ? "var(--ok)" : "var(--fg-faint)" }}>
                          {inst.has_session ? "● session ready" : "○ no session"}
                        </span>
                      </span>
                      <span style={{ display: "flex", gap: 10 }}>
                        <button className="mini" onClick={() => { setTarget(inst.path); setView("updates"); }}>Updates</button>
                        <button className="cta" disabled={!account || busy} onClick={() => doPlay(inst)}>Play</button>
                      </span>
                    </div>
                  ))}
                </div>
              )}
              {!account && <p className="muted" style={{ marginTop: 12 }}>Sign in on the Account screen to enable Play.</p>}
            </div>
          </>
        )}

        {/* ---------------- MODS ---------------- */}
        {view === "mods" && (
          <>
            <div className="topbar">
              <div><div className="eyebrow">Browse &amp; install</div><h1 className="title">Mods</h1></div>
              <span className="grow" />
              <select value={target} onChange={(e) => setTarget(e.target.value)}>
                {installs.map((i) => (<option key={i.path} value={i.path}>{i.name}</option>))}
              </select>
            </div>
            <div className="view">
              <form style={{ display: "flex", gap: 8, marginBottom: 12 }} onSubmit={(e) => { e.preventDefault(); doSearch(); }}>
                <input style={{ flex: 1 }} placeholder="search ModDB…" value={search} onChange={(e) => setSearch(e.target.value)} />
                <button className="btn" type="submit" disabled={busy}>Search</button>
              </form>
              {results.length > 0 && (
                <div className="list" style={{ marginBottom: 14 }}>
                  {results.map((m) => (
                    <div className="li" key={m.modid}>
                      <span><span className="nm">{m.name}</span> <span className="meta">by {m.author} · {m.downloads.toLocaleString()} dl</span></span>
                      <span style={{ display: "flex", gap: 8 }}>
                        <button className="mini" onClick={() => doTip(m)} title="Show author donation link">♥ Tip</button>
                        <button className="cta" disabled={busy || !target} onClick={() => doInstall(m)}>Install</button>
                      </span>
                    </div>
                  ))}
                </div>
              )}
              <p className="muted">Installed in {targetName}: {installed.length} mod{installed.length === 1 ? "" : "s"}.</p>
            </div>
          </>
        )}

        {/* ---------------- ACCOUNT ---------------- */}
        {view === "account" && (
          <>
            <div className="topbar"><div><div className="eyebrow">Vintage Story</div><h1 className="title">Account</h1></div></div>
            <div className="view" style={{ maxWidth: 420 }}>
              {account ? (
                <>
                  <p>Signed in as <b>{account.playername}</b>.</p>
                  <p className="muted">Your session is saved and carried into every installation at launch — no re-login.</p>
                  <button className="btn" onClick={doLogout}>Log out</button>
                </>
              ) : (
                <>
                  <label className="field"><span className="lab">Email</span><input value={email} onChange={(e) => setEmail(e.target.value)} /></label>
                  <label className="field"><span className="lab">Password</span><input type="password" value={password} onChange={(e) => setPassword(e.target.value)} /></label>
                  {prelogintoken && <label className="field"><span className="lab">2FA / TOTP code</span><input value={totp} onChange={(e) => setTotp(e.target.value)} /></label>}
                  <button className="cta" disabled={busy} onClick={doLogin}>{prelogintoken ? "Submit 2FA code" : "Log in"}</button>
                </>
              )}
            </div>
          </>
        )}

        {/* ---------------- SETTINGS ---------------- */}
        {view === "settings" && (
          <>
            <div className="topbar"><div><div className="eyebrow">Configuration</div><h1 className="title">Settings</h1></div></div>
            <div className="view" style={{ maxWidth: 640 }}>
              <label className="field"><span className="lab">Game executable</span><input value={gameExe} onChange={(e) => setGameExe(e.target.value)} /></label>
              <label className="field"><span className="lab">Installations folder</span><input value={installationsDir} onChange={(e) => setInstallationsDir(e.target.value)} /></label>
              <label className="field"><span className="lab">Game version (for compatibility)</span><input style={{ width: 120 }} value={gameVersion} onChange={(e) => setGameVersion(e.target.value)} /></label>

              <div className="field">
                <span className="lab">Theme</span>
                <div className="themes">
                  {THEMES.map((t) => (
                    <button key={t.id} className={"theme-card" + (theme === t.id ? " sel" : "")} onClick={() => setTheme(t.id)}>
                      <div className="swatch">{t.colors.map((c, k) => (<i key={k} style={{ background: c }} />))}</div>
                      <div className="tn">{t.name}</div>
                      <div className="td">{t.desc}</div>
                    </button>
                  ))}
                </div>
              </div>

              <div className="field">
                <span className="lab">Activity log</span>
                <pre className="logbox">{log.join("\n") || "…"}</pre>
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export default App;
