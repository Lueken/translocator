import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
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
type InstallationMeta = {
  name: string;
  version: string;
  start_params: string;
  env_vars: string;
  auto_backup: boolean;
  compression: number;
  icon: string;
  favorite: boolean;
  last_played: number;
  total_time_played: number;
};
type InstallationCard = { path: string; meta: InstallationMeta; mod_count: number; has_session: boolean };
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
type Toast = { id: number; msg: string; undo?: () => void; ok?: boolean };

const DEFAULT_INSTALLS = "C:\\Users\\31686\\AppData\\Roaming\\VSLInstallations";
const DEFAULT_GAME_EXE = "C:\\Users\\31686\\AppData\\Roaming\\VSLGameVersions\\1.22.3\\Vintagestory.exe";

const THEMES: { id: Theme; name: string; desc: string; colors: string[] }[] = [
  { id: "almanac", name: "Temporal Almanac", desc: "Parchment ledger, serif", colors: ["#0E1512", "#E9DDC4", "#B26A22", "#3C7A5C"] },
  { id: "workshop", name: "Blued Workshop", desc: "Gunmetal + cyan charge", colors: ["#13171D", "#1B242D", "#C77B3B", "#74D6C8"] },
  { id: "terminal", name: "Field Terminal", desc: "Amber phosphor, mono", colors: ["#141210", "#1A1610", "#E8B24A", "#83BE9E"] },
];
const COMPAT_LABEL: Record<Compat, string> = {
  exact: "author-tagged for your version",
  minor: "same 1.x line, should work",
  unlikely: "no matching version tag, probably incompatible",
};
const compatClass = (c: Compat) => (c === "exact" ? "ok" : c === "minor" ? "warn" : "bad");
const entryStatus = (u: ModUpdate): { cls: string; label: string } => {
  if (!u.latest_compatible) return { cls: "bad", label: "Needs newer game" };
  const rel = u.newer.find((r) => r.modversion === u.latest_compatible);
  return rel?.compat === "exact" ? { cls: "ok", label: "Compatible" } : { cls: "warn", label: "Should work" };
};
const fmtPlaytime = (secs: number) => {
  if (!secs) return "never played";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h) return `${h}h ${m}m played`;
  return m ? `${m}m played` : "under a minute";
};
const fmtLastPlayed = (ms: number) => {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const d = Math.floor(hr / 24);
  return d < 30 ? `${d}d ago` : new Date(ms).toLocaleDateString();
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

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand("copy");
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

// localStorage helpers for per-install ignore/pin lists
const igKey = (p: string) => `tl-ignores:${p}`;
const pinKey = (p: string) => `tl-pins:${p}`;
const loadIgnores = (p: string): Record<string, string> => JSON.parse(localStorage.getItem(igKey(p)) || "{}");
const loadPins = (p: string): string[] => JSON.parse(localStorage.getItem(pinKey(p)) || "[]");

const Gear = ({ size = 32 }: { size?: number }) => (
  <svg className="gear" viewBox="0 0 100 100" aria-hidden="true" style={{ width: size, height: size }}>
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

function Titlebar() {
  const win = getCurrentWindow();
  return (
    <div className="titlebar" data-tauri-drag-region>
      <Gear size={16} />
      <span className="tb-name" data-tauri-drag-region>Translocator</span>
      <div className="tb-drag" data-tauri-drag-region />
      <button className="winbtn" title="Minimize" onClick={() => win.minimize()}>
        <svg viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.2"><line x1="2" y1="6" x2="10" y2="6" /></svg>
      </button>
      <button className="winbtn" title="Maximize" onClick={() => win.toggleMaximize()}>
        <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2"><rect x="2.5" y="2.5" width="7" height="7" /></svg>
      </button>
      <button className="winbtn close" title="Close" onClick={() => win.close()}>
        <svg viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.2"><line x1="3" y1="3" x2="9" y2="9" /><line x1="9" y1="3" x2="3" y2="9" /></svg>
      </button>
    </div>
  );
}

function App() {
  const [gameExe, setGameExe] = useState(DEFAULT_GAME_EXE);
  const [installationsDir, setInstallationsDir] = useState(DEFAULT_INSTALLS);
  const [gameVersion, setGameVersion] = useState("1.22.3");

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [prelogintoken, setPrelogintoken] = useState<string | null>(null);

  const [account, setAccount] = useState<Account | null>(null);
  const [installs, setInstalls] = useState<InstallationCard[]>([]);
  const [editing, setEditing] = useState<InstallationCard | null>(null);
  const [draft, setDraft] = useState<InstallationMeta | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<InstallationCard | null>(null);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const [target, setTarget] = useState<string>("");

  const [updates, setUpdates] = useState<ModUpdate[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [checked, setChecked] = useState<string | null>(null); // last-checked time
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [installing, setInstalling] = useState<{ modid: string; pct: number } | null>(null); // pct < 0 = indeterminate
  const [filter, setFilter] = useState("");
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [ignores, setIgnores] = useState<Record<string, string>>({});
  const [pins, setPins] = useState<string[]>([]);

  const [showBackups, setShowBackups] = useState(false);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [confirmRestore, setConfirmRestore] = useState<BackupInfo | null>(null);

  const [search, setSearch] = useState("");
  const [results, setResults] = useState<ModSummary[]>([]);
  const [installed, setInstalled] = useState<string[]>([]);

  const [view, setView] = useState<View>("updates");
  const [theme, setTheme] = useState<Theme>(() => (localStorage.getItem("tl-theme") as Theme) || "almanac");
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);

  const say = (line: string) => setLog((l) => [`${new Date().toLocaleTimeString()}  ${line}`, ...l].slice(0, 200));
  const toast = (msg: string, undo?: () => void, ok = true) => {
    const id = ++toastId.current;
    setToasts((t) => [...t, { id, msg, undo, ok }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 7000);
  };

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
    const un = listen<{ done: number; total: number }>("check-progress", (e) => setProgress(e.payload));
    const un2 = listen<{ modid: string; received: number; total: number }>("install-progress", (e) => {
      const { modid, received, total } = e.payload;
      setInstalling({ modid, pct: total > 0 ? Math.min(100, (received / total) * 100) : -1 });
    });
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (!target && installs.length) setTarget(installs[0].path);
  }, [installs, target]);
  useEffect(() => {
    if (target) {
      refreshInstalled(target);
      setIgnores(loadIgnores(target));
      setPins(loadPins(target));
      setUpdates([]);
      setChecked(null);
      // the selected install's pinned version drives update-compatibility
      const card = installs.find((i) => i.path === target);
      if (card?.meta.version) setGameVersion(card.meta.version);
    }
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
        say(`Logged in as ${res.account.playername}.`);
        toast(`Signed in as ${res.account.playername}`);
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
    say("Logged out.");
  }
  async function refreshInstalls() {
    try {
      const list = await invoke<InstallationCard[]>("list_installations", { installationsDir, defaultVersion: gameVersion });
      setInstalls(list);
      say(`Found ${list.length} installation(s).`);
    } catch (e) {
      say(`Error listing installs: ${e}`);
    }
  }
  async function saveInstallation(path: string, meta: InstallationMeta) {
    await invoke("save_installation", { path, meta });
    if (path === target && meta.version) setGameVersion(meta.version);
    await refreshInstalls();
  }
  async function toggleFavorite(card: InstallationCard) {
    await saveInstallation(card.path, { ...card.meta, favorite: !card.meta.favorite });
  }
  async function openFolder(path: string) {
    try {
      await invoke("open_install_folder", { path });
    } catch (e) {
      say(`Open folder error: ${e}`);
    }
  }
  async function deleteInstallation(card: InstallationCard) {
    setConfirmDelete(null);
    try {
      await invoke("delete_installation", { path: card.path });
      say(`Deleted installation ${card.meta.name}.`);
      toast(`Deleted ${card.meta.name}`);
      if (target === card.path) setTarget("");
      await refreshInstalls();
    } catch (e) {
      say(`Delete error: ${e}`);
    }
  }
  async function refreshInstalled(installDir: string) {
    try {
      setInstalled(await invoke<string[]>("list_mod_files", { installDir }));
    } catch {
      setInstalled([]);
    }
  }
  async function doPlay(path?: string) {
    const p = path ?? target;
    const inst = installs.find((i) => i.path === p);
    if (!account || !inst) return;
    setBusy(true);
    try {
      say(`▶ ${inst.meta.name}: validate → stamp → launch ...`);
      const res = await invoke<PlayResult>("play", { gameExe, installDir: inst.path, account });
      if (res.status === "needsRelogin") {
        say(`✗ Session rejected by server (${res.reason}). Re-login needed.`);
        toast("Session expired. Sign in again on the Account screen.", undefined, false);
        setAccount(null);
      } else {
        say(`■ ${inst.meta.name} exited (code ${res.exit_code}).`);
        await refreshInstalls(); // pick up new playtime
        if (res.rotated) {
          setAccount(res.account);
          say("↻ Game rotated the session; captured and saved the new key.");
        } else {
          say("✓ Session unchanged and still valid.");
        }
      }
    } catch (e) {
      say(`Error launching: ${e}`);
    } finally {
      setBusy(false);
    }
  }
  async function checkUpdates() {
    if (!target) return;
    setBusy(true);
    setProgress({ done: 0, total: installed.length });
    try {
      const name = installs.find((i) => i.path === target)?.meta.name ?? target;
      say(`Checking updates for ${name} (game ${gameVersion}) ...`);
      const ups = await invoke<ModUpdate[]>("check_updates", { installDir: target, gameVersion });
      setUpdates(ups);
      setChecked(new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }));
      say(`${ups.length} mod(s) have newer releases.`);
    } catch (e) {
      say(`Update check error: ${e}`);
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }
  async function installVersion(u: ModUpdate, modversion: string, opts?: { silentToast?: boolean }) {
    setBusy(true);
    setInstalling({ modid: u.modid, pct: -1 });
    try {
      say(`Updating ${u.name} → ${modversion} ...`);
      const newFile = await invoke<string>("install_release", {
        installDir: target,
        modidstr: u.modid,
        modversion,
        oldFilename: u.installed_filename,
      });
      say(`✓ ${u.name} now at ${modversion}.`);
      setUpdates((prev) => prev.filter((x) => x.modid !== u.modid));
      await refreshInstalled(target);
      if (!opts?.silentToast) {
        const prevVersion = u.installed_version;
        const dir = target;
        toast(`${u.name} updated to ${modversion}`, async () => {
          try {
            await invoke<string>("install_release", {
              installDir: dir,
              modidstr: u.modid,
              modversion: prevVersion,
              oldFilename: newFile,
            });
            say(`↩ ${u.name} rolled back to ${prevVersion}.`);
            toast(`${u.name} rolled back to ${prevVersion}`);
            await refreshInstalled(dir);
          } catch (e) {
            say(`Rollback error: ${e}`);
          }
        });
      }
    } catch (e) {
      say(`Update error: ${e}`);
    } finally {
      setBusy(false);
      setInstalling(null);
    }
  }
  async function updateAllLatest() {
    const targets = readyUpdates.filter((u) => u.latest_compatible);
    if (!targets.length) return;
    setBusy(true);
    try {
      try {
        const id = await invoke<string>("backup_mods", { installDir: target });
        say(`Backed up ${installed.length} mods before updating (id ${id}).`);
        await refreshBackups(target);
      } catch (e) {
        say(`⚠ Backup failed (${e}); continuing with update.`);
      }
      say(`Updating ${targets.length} mod(s) to latest compatible ...`);
      let ok = 0;
      for (const u of targets) {
        setInstalling({ modid: u.modid, pct: -1 });
        try {
          await invoke<string>("install_release", {
            installDir: target,
            modidstr: u.modid,
            modversion: u.latest_compatible,
            oldFilename: u.installed_filename,
          });
          ok++;
          say(`  ✓ ${u.name} → ${u.latest_compatible}`);
        } catch (e) {
          say(`  ✗ ${u.name}: ${e}`);
        }
      }
      toast(`Updated ${ok} of ${targets.length} mods`);
      await refreshInstalled(target);
      await checkUpdates();
    } finally {
      setBusy(false);
      setInstalling(null);
    }
  }
  async function refreshBackups(installDir: string) {
    try {
      setBackups(await invoke<BackupInfo[]>("list_backups", { installDir }));
    } catch {
      setBackups([]);
    }
  }
  async function doRestore(b: BackupInfo) {
    setConfirmRestore(null);
    setBusy(true);
    try {
      // Safety net first, so the restore itself is undoable.
      try {
        await invoke<string>("backup_mods", { installDir: target });
      } catch { /* non-fatal */ }
      say(`Restoring backup ${b.id} ...`);
      await invoke("restore_backup", { installDir: target, id: b.id });
      say(`✓ Restored Mods folder to backup ${b.id}.`);
      toast(`Restored snapshot from ${b.created}`);
      await refreshInstalled(target);
      await refreshBackups(target);
      setUpdates([]);
      setChecked(null);
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
      toast(`Backed up ${installed.length} mods`);
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
  async function copyReport() {
    const name = installs.find((i) => i.path === target)?.meta.name ?? "installation";
    const lines: string[] = [
      `# ${name} update report (${new Date().toISOString().slice(0, 10)})`,
      `${visibleUpdates.length} updates pending for game ${gameVersion}`,
      "",
    ];
    for (const u of visibleUpdates) {
      const st = entryStatus(u);
      lines.push(`## ${u.name}  ${u.installed_version} → ${u.latest_compatible ?? u.newer[0].modversion}  (${st.label.toLowerCase()})`);
      for (const r of u.newer) {
        lines.push(`### ${r.modversion} (${r.created?.slice(0, 10) || "n/a"}, ${r.compat})`);
        const log = stripHtml(r.changelog);
        if (log) lines.push(log);
        lines.push("");
      }
    }
    const ok = await copyText(lines.join("\n"));
    toast(ok ? "Update report copied to clipboard" : "Copy failed; see log", undefined, ok);
    if (!ok) say(lines.join("\n"));
  }
  function ignoreVersion(u: ModUpdate) {
    const v = u.latest_compatible ?? u.newer[0]?.modversion;
    if (!v) return;
    const next = { ...ignores, [u.modid]: v };
    setIgnores(next);
    localStorage.setItem(igKey(target), JSON.stringify(next));
    setMenuFor(null);
    toast(`${u.name} ${v} ignored until a newer release`);
  }
  function togglePin(u: ModUpdate) {
    const next = pins.includes(u.modid) ? pins.filter((m) => m !== u.modid) : [...pins, u.modid];
    setPins(next);
    localStorage.setItem(pinKey(target), JSON.stringify(next));
    setMenuFor(null);
    toast(pins.includes(u.modid) ? `${u.name} unpinned` : `${u.name} pinned (updates hidden)`);
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
    if (!target) return;
    setBusy(true);
    try {
      say(`Installing "${m.name}" ...`);
      const fname = await invoke<string>("install_mod", { installDir: target, modidstr: m.modidstr });
      say(`✓ Installed ${fname}.`);
      toast(`Installed ${m.name}`);
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
      say(`  ↳ missing dependency ${dep.modid}${dep.version ? ` (${dep.version})` : ""}; installing...`);
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

  // ---- derived state for the register ----
  const visibleUpdates = useMemo(
    () =>
      updates.filter((u) => {
        if (pins.includes(u.modid)) return false;
        const best = u.latest_compatible ?? u.newer[0]?.modversion;
        if (best && ignores[u.modid] === best) return false;
        if (filter) {
          const f = filter.toLowerCase();
          if (!u.name.toLowerCase().includes(f) && !u.modid.includes(f)) return false;
        }
        return true;
      }),
    [updates, pins, ignores, filter]
  );
  const readyUpdates = useMemo(() => visibleUpdates.filter((u) => u.latest_compatible), [visibleUpdates]);
  const heldUpdates = useMemo(() => visibleUpdates.filter((u) => !u.latest_compatible), [visibleUpdates]);
  const countExact = readyUpdates.filter((u) => entryStatus(u).cls === "ok").length;
  const countMinor = readyUpdates.filter((u) => entryStatus(u).cls === "warn").length;
  const hiddenCount = updates.filter(
    (u) => pins.includes(u.modid) || ignores[u.modid] === (u.latest_compatible ?? u.newer[0]?.modversion)
  ).length;

  const targetInfo = installs.find((i) => i.path === target);
  const targetName = targetInfo?.meta.name ?? "";
  const NAV: { id: View; label: string; count?: number }[] = [
    { id: "installations", label: "Installations", count: installs.length },
    { id: "updates", label: "Updates", count: updates.length || undefined },
    { id: "mods", label: "Mods", count: installed.length || undefined },
    { id: "account", label: "Account" },
    { id: "settings", label: "Settings" },
  ];

  const renderRow = (u: ModUpdate, i: number, held: boolean) => {
    const st = entryStatus(u);
    const open = expanded.has(u.modid);
    return (
      <div key={u.modid} style={held ? { opacity: 0.82 } : undefined}>
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
            <button
              className={(held ? "mini" : "cta") + (installing?.modid === u.modid ? " installing" : "")}
              disabled={busy || held}
              title={held ? "This release targets a newer game version" : undefined}
              onClick={() => u.latest_compatible && installVersion(u, u.latest_compatible)}
            >
              <span className="cta-label">Update</span>
              {installing?.modid === u.modid && (
                <span
                  className={"btn-prog" + (installing.pct < 0 ? " indet" : "")}
                  style={installing.pct >= 0 ? { width: `${installing.pct}%` } : undefined}
                />
              )}
            </button>
            <span style={{ position: "relative" }}>
              <button className="mini more" onClick={() => setMenuFor(menuFor === u.modid ? null : u.modid)} title="More actions">⋯</button>
              {menuFor === u.modid && (
                <span className="menu">
                  <button onClick={() => ignoreVersion(u)}>Ignore this version</button>
                  <button onClick={() => togglePin(u)}>{pins.includes(u.modid) ? "Unpin mod" : "Pin mod (hide updates)"}</button>
                  <button onClick={() => { setMenuFor(null); openModDB(u.assetid); }}>Open on ModDB</button>
                </span>
              )}
            </span>
          </div>
        </div>
        {open && (
          <div className="folio">
            {u.newer.length > 1 && (
              <div className="span-note">
                Updating {u.installed_version} → {u.latest_compatible ?? u.newer[0].modversion} applies everything below. Read the notes before deciding.
              </div>
            )}
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
  };

  return (
    <div className="shell">
      <Titlebar />
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
          <button className="acct-chip" onClick={() => setView("account")}>
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
          {targetInfo && (
            <div className="dock">
              <div className="dock-name">{targetName}</div>
              <div className="dock-meta tab">{gameVersion} · {installed.length} mods</div>
              <button className="play" disabled={!account || busy} onClick={() => doPlay()}
                title={account ? `Launch ${targetName}` : "Sign in first"}>
                ▶&nbsp;&nbsp;Play
              </button>
            </div>
          )}
        </aside>

        <div className="main">
          {/* ---------------- UPDATES ---------------- */}
          {view === "updates" && (
            <>
              <div className="lhd">
                <div>
                  <div className="eyebrow">Ledger of Changes</div>
                  <h1 className="title lhd-title">{targetName || "Updates"}</h1>
                </div>
                <div className="controls">
                  <select value={target} onChange={(e) => setTarget(e.target.value)}>
                    {installs.map((i) => (
                      <option key={i.path} value={i.path}>{i.meta.name}</option>
                    ))}
                  </select>
                  <span className="pchip">Game <b>{gameVersion}</b></span>
                  <span className="grow" />
                  <button className="btn" disabled={busy || !target} onClick={checkUpdates}>Check for updates</button>
                  {readyUpdates.length > 0 && (
                    <div className="ctacol">
                      <button className="cta" disabled={busy} onClick={updateAllLatest}>Update all compatible</button>
                      <span className="capt">Mods back up before update</span>
                    </div>
                  )}
                </div>
              </div>

              {updates.length > 0 && (
                <div className="toolbar">
                  <input className="filterbox" placeholder="Filter mods…" value={filter} onChange={(e) => setFilter(e.target.value)} />
                  <span className="counts">
                    <b>{visibleUpdates.length}</b> updates
                    <span className="cdot" style={{ background: "var(--ok)" }} />{countExact} compatible
                    <span className="cdot" style={{ background: "var(--warn)" }} />{countMinor} should work
                    <span className="cdot" style={{ background: "var(--bad)" }} />{heldUpdates.length} held back
                    {hiddenCount > 0 && <span style={{ marginLeft: 10, color: "var(--fg-faint)" }}>({hiddenCount} ignored/pinned)</span>}
                  </span>
                  <button className="link" style={{ marginLeft: "auto" }} onClick={copyReport}>⎘ Copy update report</button>
                </div>
              )}

              <div className="view">
                {progress && (
                  <div className="checking">
                    <div className="checking-t">Consulting the ledger…</div>
                    <div className="prog"><i style={{ width: `${progress.total ? (progress.done / progress.total) * 100 : 0}%` }} /></div>
                    <div className="prog-n tab">{progress.done} / {progress.total} mods checked</div>
                  </div>
                )}

                {!progress && updates.length === 0 && (
                  checked ? (
                    <div className="empty">
                      <Gear size={40} />
                      <h3>Every entry is current</h3>
                      <p>All {installed.length} mods match their newest compatible release.<br />Last checked at {checked}.</p>
                      <button className="btn" disabled={busy} onClick={checkUpdates}>Check again</button>
                    </div>
                  ) : (
                    <div className="empty">
                      <Gear size={40} />
                      <h3>The ledger awaits</h3>
                      <p>Check {targetName || "your installation"} against ModDB to see what has changed.</p>
                      <button className="btn" disabled={busy || !target} onClick={checkUpdates}>Check for updates</button>
                    </div>
                  )
                )}

                {visibleUpdates.length > 0 && (
                  <div className="register">
                    {readyUpdates.length > 0 && <div className="ghead">Ready to update · {readyUpdates.length}</div>}
                    {readyUpdates.map((u, i) => renderRow(u, i, false))}
                    {heldUpdates.length > 0 && <div className="ghead hold">Held back, needs a newer game · {heldUpdates.length}</div>}
                    {heldUpdates.map((u, i) => renderRow(u, readyUpdates.length + i, true))}
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
                        <p className="muted">No backups yet. One is taken automatically before "Update all compatible".</p>
                      ) : (
                        <div className="list">
                          {backups.map((b) => (
                            <div className="li" key={b.id}>
                              <span><b className="nm">{b.created}</b> <span className="meta">{b.mod_count} mod{b.mod_count === 1 ? "" : "s"} · {b.id}</span></span>
                              <button className="mini" disabled={busy} onClick={() => setConfirmRestore(b)}>Restore</button>
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
                  <div className="empty">
                    <Gear size={40} />
                    <h3>No installations found</h3>
                    <p>Set your installations folder in Settings, then refresh.</p>
                    <button className="btn" onClick={() => setView("settings")}>Open Settings</button>
                  </div>
                ) : (
                  <div className="inst-grid">
                    {installs.map((inst) => (
                      <div className="inst-card" key={inst.path}>
                        <button
                          className={"fav" + (inst.meta.favorite ? " on" : "")}
                          title={inst.meta.favorite ? "Unfavorite" : "Favorite"}
                          onClick={() => toggleFavorite(inst)}
                        >
                          {inst.meta.favorite ? "★" : "☆"}
                        </button>
                        <div className="inst-ico">{inst.meta.icon || inst.meta.name[0]?.toUpperCase() || "?"}</div>
                        <div className="inst-body">
                          <div className="inst-name">{inst.meta.name}</div>
                          <div className="inst-meta tab">
                            {inst.meta.version || "—"} · {inst.mod_count} mod{inst.mod_count === 1 ? "" : "s"}
                            {inst.has_session && <span style={{ color: "var(--ok)" }}> · ● session</span>}
                          </div>
                          <div className="inst-sub">
                            {fmtPlaytime(inst.meta.total_time_played)}
                            {inst.meta.last_played ? ` · ${fmtLastPlayed(inst.meta.last_played)}` : ""}
                          </div>
                          <div className="inst-actions">
                            <button className="cta" disabled={!account || busy} onClick={() => doPlay(inst.path)}>▶ Play</button>
                            <button className="mini" onClick={() => { setTarget(inst.path); setView("updates"); }}>Updates</button>
                            <button className="mini" onClick={() => { setEditing(inst); setDraft({ ...inst.meta }); }}>Edit</button>
                            <button className="mini" onClick={() => openFolder(inst.path)}>Folder</button>
                          </div>
                        </div>
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
                  {installs.map((i) => (<option key={i.path} value={i.path}>{i.meta.name}</option>))}
                </select>
              </div>
              <div className="view">
                <form style={{ display: "flex", gap: 8, marginBottom: 12 }} onSubmit={(e) => { e.preventDefault(); doSearch(); }}>
                  <input style={{ flex: 1 }} placeholder="Search ModDB…" value={search} onChange={(e) => setSearch(e.target.value)} />
                  <button className="btn" type="submit" disabled={busy}>Search</button>
                </form>
                {results.length > 0 && (
                  <div className="list" style={{ marginBottom: 14 }}>
                    {results.map((m) => (
                      <div className="li" key={m.modid}>
                        <span><span className="nm">{m.name}</span> <span className="meta">by {m.author} · {m.downloads.toLocaleString()} downloads</span></span>
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
                    <p className="muted">Your session is saved and carried into every installation at launch. No re-login.</p>
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

      {/* confirm restore modal */}
      {confirmRestore && (
        <div className="overlay" onClick={() => setConfirmRestore(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Restore this snapshot?</h3>
            <p>
              Your Mods folder will be replaced with the backup from <b>{confirmRestore.created}</b>{" "}
              ({confirmRestore.mod_count} mod{confirmRestore.mod_count === 1 ? "" : "s"}). The current folder is
              backed up first, so this can be undone.
            </p>
            <div className="acts">
              <button className="btn" onClick={() => setConfirmRestore(null)}>Cancel</button>
              <button className="danger" onClick={() => doRestore(confirmRestore)}>Restore snapshot</button>
            </div>
          </div>
        </div>
      )}

      {/* edit installation modal */}
      {editing && draft && (
        <div className="overlay" onClick={() => setEditing(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>Edit installation</h3>
            <label className="field"><span className="lab">Name</span>
              <input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></label>
            <label className="field"><span className="lab">Game version</span>
              <input style={{ width: 140 }} value={draft.version} onChange={(e) => setDraft({ ...draft, version: e.target.value })} /></label>
            <label className="field"><span className="lab">Start parameters</span>
              <input value={draft.start_params} placeholder="e.g. --openWorld ..." onChange={(e) => setDraft({ ...draft, start_params: e.target.value })} /></label>
            <label className="field"><span className="lab">Environment variables</span>
              <input value={draft.env_vars} placeholder="KEY=value, KEY2=value2" onChange={(e) => setDraft({ ...draft, env_vars: e.target.value })} /></label>
            <label className="field row-check">
              <input type="checkbox" checked={draft.auto_backup} onChange={(e) => setDraft({ ...draft, auto_backup: e.target.checked })} />
              <span>Back up mods before playing</span>
            </label>
            <div className="acts" style={{ justifyContent: "space-between" }}>
              <button className="danger" onClick={() => { setConfirmDelete(editing); setEditing(null); }}>Delete…</button>
              <span style={{ display: "flex", gap: 8 }}>
                <button className="btn" onClick={() => setEditing(null)}>Cancel</button>
                <button className="cta" onClick={async () => { await saveInstallation(editing.path, draft); setEditing(null); toast(`Saved ${draft.name}`); }}>Save</button>
              </span>
            </div>
          </div>
        </div>
      )}

      {/* confirm delete installation */}
      {confirmDelete && (
        <div className="overlay" onClick={() => setConfirmDelete(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Delete this installation?</h3>
            <p>
              <b>{confirmDelete.meta.name}</b> and its entire folder — mods, saves, and config
              ({confirmDelete.mod_count} mod{confirmDelete.mod_count === 1 ? "" : "s"}) — will be permanently deleted.
              This cannot be undone.
            </p>
            <div className="acts">
              <button className="btn" onClick={() => setConfirmDelete(null)}>Cancel</button>
              <button className="danger" onClick={() => deleteInstallation(confirmDelete)}>Delete permanently</button>
            </div>
          </div>
        </div>
      )}

      {/* toast stack */}
      <div className="toasts">
        {toasts.map((t) => (
          <div className="toast" key={t.id}>
            <span className="toast-msg">{t.ok !== false ? "✓ " : ""}{t.msg}</span>
            {t.undo && <button className="undo" onClick={() => { t.undo!(); setToasts((x) => x.filter((y) => y.id !== t.id)); }}>Undo</button>}
          </div>
        ))}
      </div>
    </div>
  );
}

export default App;
