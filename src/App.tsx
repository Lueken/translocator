import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { check as checkAppUpdate, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import gearIcon from "./assets/gear.png";
import "./themes.css";

// ---- Types mirroring the Rust command surface (src-tauri/src/lib.rs) ----
// Non-sensitive view of the signed-in account. The session key/signature stay
// in the Rust backend and never cross into the webview.
type Account = {
  playername: string;
  email: string;
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
  backups_limit: number;
  icon: string;
  favorite: boolean;
  last_played: number;
  total_time_played: number;
  // pack-managed freeze (absent on personal installs)
  managed_by?: string;
  pack_strict?: boolean;
  pack_owned_files?: string[];
  override_hashes?: Record<string, string>;
  optional_choices?: Record<string, boolean>;
};
type InstallationCard = { path: string; meta: InstallationMeta; mod_count: number; has_session: boolean };
type PlayResult =
  | { status: "needsRelogin"; reason: string }
  | { status: "needsVersion"; version: string }
  | { status: "played"; exit_code: number; rotated: boolean; account: Account };
type ModSummary = { modid: number; modidstr: string; name: string; summary: string; author: string; downloads: number };
type ReleaseView = { modversion: string; tags: string[]; changelog: string; created: string };
type ModDetailView = { name: string; text: string; side: string; assetid: number; releases: ReleaseView[] };
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
type BackupInfo = { id: string; kind: string; mod_count: number; size: number; created: string };
type AvailableVersion = { version: string; url: string; md5: string; filesize: string; cached: boolean };
type WorldInfo = {
  path: string;
  filename: string;
  name: string;
  seed: number | null;
  playstyle: string;
  world_height: number | null;
  created_version: string;
  last_version: string;
  last_played: string;
  size_bytes: number;
  modified_ms: number;
  parsed: boolean;
};
type DetectedLauncher = { launcher: string; installations_dir: string; can_enrich: boolean; count: number };
// ---- Hub / Market types (src-tauri/src/hub.rs) ----
type PackSummary = {
  id: string;
  name: string;
  summary?: string | null;
  tags?: string[] | null;
  game_version?: string | null;
  icon?: string | null;
  latest_version?: string | null;
  updated?: string | null;
};
type PackManifestMod = { modid: number; modidstr: string; name: string; modversion: string; side: string; required: boolean; fileid?: number; sha256?: string };
type PackManifest = {
  manifest_version: number;
  pack: { id: string; name: string; version: string; author: string; summary?: string; description?: string; tags?: string[]; game_version: string; icon?: string; strict?: boolean; min_launcher_version?: string };
  links?: { website?: string; discord?: string; source?: string; donate?: string } | null;
  server?: { address: string; auto_add: boolean } | null;
  mods: PackManifestMod[];
  overrides?: { path: string }[];
};
type PackDetail = {
  id: string;
  name: string;
  summary?: string | null;
  tags?: string[] | null;
  icon?: string | null;
  gameVersion?: string | null;
  links?: { website?: string; discord?: string; source?: string; donate?: string } | null;
  latest_version?: string | null;
  published?: string | null;
};
type PublicServer = {
  name: string;
  address: string;
  players: number;
  max_players: number;
  game_version: string;
  mod_count: number;
  has_password: boolean;
  whitelisted: boolean;
  playstyle: string;
  description: string;
};
type PrivateServer = { id: string; name: string; address: string; password: string; install_path: string };
// ---- Curator (src-tauri/src/curator.rs) ----
type Unresolved = { filename: string; modid: string; version: string; reason: string };
type CuratorPreview = { manifest: PackManifest; unresolved: Unresolved[]; resolved_count: number };
type CuratorMeta = {
  id: string;
  name: string;
  version: string;
  author: string;
  summary: string;
  description: string;
  tags: string;
  game_version: string;
  icon: string;
  strict: boolean;
};
type PublisherStatus = { signed_in: boolean; playername: string | null; has_key: boolean; fingerprint: string | null };
type Theme = "almanac" | "workshop" | "terminal";

// ---- App EULA (shown before the login wall; acceptance version-stamped) ----
// Bump the version only on MATERIAL changes: it re-prompts every user.
const APP_EULA_VERSION = "1.1";
const APP_EULA: { h: string; p: string }[] = [
  {
    h: "1. Independent project",
    p: "Translocator is an independent, community-developed launcher by Lueken Good Design LLC. It is not affiliated with, endorsed by, sponsored by, or associated with Anego Studios or the Vintage Story development team. \"Vintage Story\" and related marks are trademarks of Anego Studios. All rights to the game, its assets, and its intellectual property belong to their respective owners. Mods remain the property of their authors.",
  },
  {
    h: "2. What Translocator does",
    p: "Translocator manages Vintage Story installations. It downloads official game binaries only from Anego Studios' official servers, and downloads mods only from the official Vintage Story Mod Database, always at the source, so download counts and statistics stay with the authors. Modpacks are manifests that reference mods by their official database entries; Translocator never re-hosts, bundles, or redistributes the game, its binaries, or any third-party mod. A Vintage Story account is required: signing in with it is the gate to using Translocator at all.",
  },
  {
    h: "3. Optional optimized client (Optimum)",
    p: "Translocator can invoke Optimum, an independent project by Zaldaryon, to build a performance-optimized client. That build happens entirely on this machine, from this machine's own game installation and from source repositories published by Anego Studios; nothing derived from it is ever uploaded or redistributed by Translocator. Optimum is separately licensed (GPL v3.0 with the Commons Clause) and presents its own notice; Translocator will show it to you and record your acceptance before Optimum is ever run on your behalf. Declining only means your installations run the standard client.",
  },
  {
    h: "4. Your account and data",
    p: "Your credentials are sent only to Vintage Story's official authentication servers, never to Translocator's operator or any third party. Your session is stored encrypted on this machine, tied to your Windows user account. Translocator collects no telemetry.",
  },
  {
    h: "5. License",
    p: "Translocator is proprietary software of Lueken Good Design LLC, provided free of charge for personal use. This notice is a plain-language summary; the complete and controlling terms are in the Translocator LICENSE file, which is installed alongside the application and included in the source repository. You may not sell, redistribute, or misrepresent the origin of Translocator. Publishing modpacks through Translocator does not transfer any rights in the referenced mods, which remain subject to their authors' licenses.",
  },
  {
    h: "6. Disclaimer of warranty",
    p: "THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES, OR OTHER LIABILITY ARISING FROM THE USE OF THIS SOFTWARE. BACK UP YOUR WORLDS.",
  },
  {
    h: "7. Acceptance",
    p: "By proceeding, you acknowledge that you have read, understood, and agree to the terms above.",
  },
];
type View = "installations" | "updates" | "mods" | "worlds" | "servers" | "market" | "pack" | "curator" | "account" | "settings";
type Toast = { id: number; msg: string; undo?: () => void; ok?: boolean };

// The Hub base URL. Read-only Market/pack calls route through Rust (the webview
// CSP forbids direct network access).
const HUB_URL = "https://api.translocator.app";

// Paths start from localStorage; on first run they're filled from
// `suggested_paths` (resolved to this machine's %APPDATA%), never hardcoded.

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
const fmtBytes = (n: number) => {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
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
// VS playstyle langcodes → readable labels (fallback: title-case the raw code).
const PLAYSTYLE_LABEL: Record<string, string> = {
  surviveandbuild: "Survival",
  surviveandbuildhard: "Survival (Hard)",
  wildernesssurvival: "Wilderness",
  exploration: "Exploration",
  creativebuilding: "Creative",
};
const playstyleLabel = (s: string) =>
  s ? PLAYSTYLE_LABEL[s] ?? s.replace(/^\w/, (c) => c.toUpperCase()) : "";
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

const slugify = (s: string) =>
  s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
const EMPTY_CUR_META: CuratorMeta = {
  id: "", name: "", version: "1.0.0", author: "", summary: "", description: "", tags: "", game_version: "", icon: "", strict: false,
};

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

// No OS titlebar: the window is frameless and the controls float top-right,
// integrated over the content. The strip itself is the drag region.
function WindowChrome() {
  const win = getCurrentWindow();
  return (
    <div className="winchrome" data-tauri-drag-region>
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

type Prereqs = { dotnet: boolean; git: boolean; ilspycmd: boolean };
type OptimumStatus = {
  use_optimum: boolean;
  eula_accepted: boolean;
  eula_release: string | null;
  prereqs: Prereqs;
  package_state: "none" | "building" | "ready" | "failed" | "unsupported";
  package_path: string | null;
  detail: string | null;
  release: string | null;
};
type EulaText = { release: string; text: string };

function App() {
  const [gameExe, setGameExe] = useState(() => localStorage.getItem("tl-game-exe") || "");
  const [installationsDir, setInstallationsDir] = useState(() => localStorage.getItem("tl-installs") || "");
  const [gameVersion, setGameVersion] = useState("1.22.3");

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [prelogintoken, setPrelogintoken] = useState<string | null>(null);

  const [account, setAccount] = useState<Account | null>(null);
  const [authChecked, setAuthChecked] = useState(false);
  // null = still reading the record; false = notice must be shown; true = clear.
  const [appEulaOk, setAppEulaOk] = useState<boolean | null>(null);
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
  const [searching, setSearching] = useState(false);
  const [searched, setSearched] = useState(false);
  const [installed, setInstalled] = useState<string[]>([]);
  const [modDetails, setModDetails] = useState<Record<string, ModDetailView | "loading">>({});
  const [openMod, setOpenMod] = useState<string | null>(null);

  const [detected, setDetected] = useState<DetectedLauncher[]>([]);

  const [worlds, setWorlds] = useState<WorldInfo[]>([]);
  const [worldsBusy, setWorldsBusy] = useState(false);
  const [confirmDeleteWorld, setConfirmDeleteWorld] = useState<WorldInfo | null>(null);

  // ---- Market / packs ----
  const [packs, setPacks] = useState<PackSummary[]>([]);
  const [packsBusy, setPacksBusy] = useState(false);
  const [packsLoaded, setPacksLoaded] = useState(false);
  const [packsError, setPacksError] = useState<string | null>(null);
  const [selectedPack, setSelectedPack] = useState<string | null>(null);
  const [packDetail, setPackDetail] = useState<PackDetail | null>(null);
  const [packManifest, setPackManifest] = useState<PackManifest | null>(null);
  const [packBusy, setPackBusy] = useState(false);
  const [packError, setPackError] = useState<string | null>(null);
  // ---- Install-pack flow ----
  const [packInstallOpen, setPackInstallOpen] = useState(false);
  const [packInstallName, setPackInstallName] = useState("");
  const [packSeed, setPackSeed] = useState<string>("");
  const [packOptionals, setPackOptionals] = useState<Record<string, boolean>>({});
  const [packInstalling, setPackInstalling] = useState(false);
  const [packProgress, setPackProgress] = useState<{ phase: string; detail: string; done: number; total: number; received: number; bytesTotal: number } | null>(null);

  // ---- Servers ----
  const [serverTab, setServerTab] = useState<"public" | "private">("public");
  const [publicServers, setPublicServers] = useState<PublicServer[]>([]);
  const [serversBusy, setServersBusy] = useState(false);
  const [serversLoaded, setServersLoaded] = useState(false);
  const [serversError, setServersError] = useState<string | null>(null);
  const [serverSearch, setServerSearch] = useState("");
  const [serverVersionFilter, setServerVersionFilter] = useState("");
  const [serverSort, setServerSort] = useState<"players" | "name">("players");
  const [joinServer, setJoinServer] = useState<PublicServer | null>(null);
  const [joinInstall, setJoinInstall] = useState("");
  const [joinPassword, setJoinPassword] = useState("");
  const [privateServers, setPrivateServers] = useState<PrivateServer[]>([]);
  const [privDraft, setPrivDraft] = useState<PrivateServer | null>(null);

  // ---- Curator ----
  const [curInstall, setCurInstall] = useState("");
  const [curMeta, setCurMeta] = useState<CuratorMeta>(() => {
    try { return { ...EMPTY_CUR_META, ...JSON.parse(localStorage.getItem("tl-curator-meta") || "{}") }; }
    catch { return EMPTY_CUR_META; }
  });
  const [curIdEdited, setCurIdEdited] = useState(() => !!curMeta.id && curMeta.id !== slugify(curMeta.name));
  const [curLinks, setCurLinks] = useState({ website: "", discord: "", source: "", donate: "" });
  const [curServer, setCurServer] = useState({ address: "", auto_add: false });
  const [curConfigs, setCurConfigs] = useState<string[]>([]);
  const [curSelected, setCurSelected] = useState<Set<string>>(new Set());
  const [curPreview, setCurPreview] = useState<CuratorPreview | null>(null);
  const [curBusy, setCurBusy] = useState(false);
  const [pubBusy, setPubBusy] = useState(false);
  const [pubStatus, setPubStatus] = useState<PublisherStatus | null>(null);
  const [registering, setRegistering] = useState(false);

  // game versions (shared dedup cache)
  const [availableVersions, setAvailableVersions] = useState<AvailableVersion[]>([]);
  const [versionProgress, setVersionProgress] = useState<{ version: string; phase: string; pct: number } | null>(null);
  const [creating, setCreating] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createVersion, setCreateVersion] = useState("");
  const [cachedVersions, setCachedVersions] = useState<string[]>([]);

  // Optimum: one optimized client package per game version, built locally.
  const [optimum, setOptimum] = useState<OptimumStatus | null>(null);
  const [optimumPhase, setOptimumPhase] = useState<string | null>(null);
  const [eula, setEula] = useState<EulaText | null>(null);
  const [eulaAgreed, setEulaAgreed] = useState(false);
  const [eulaBusy, setEulaBusy] = useState(false);
  // Bumped from the progress listener, whose closure is pinned to the first
  // render and so cannot re-read the currently selected version itself.
  const [optimumTick, setOptimumTick] = useState(0);
  const [seedSettings, setSeedSettings] = useState(() => localStorage.getItem("tl-seed-settings") !== "0");
  // First-launch Optimum card: shown once per machine, after login.
  const [optimumOnboarded, setOptimumOnboarded] = useState(() => localStorage.getItem("tl-optimum-onboarded") === "1");
  // Launcher self-update (tauri-plugin-updater against translocator.app).
  const [appUpdate, setAppUpdate] = useState<Update | null>(null);
  const [appUpdateDismissed, setAppUpdateDismissed] = useState(false);
  const [appUpdating, setAppUpdating] = useState<number | null>(null); // pct, -1 = indeterminate
  const [backingUp, setBackingUp] = useState(false);

  const [view, setView] = useState<View>("installations");
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
    if (gameExe) localStorage.setItem("tl-game-exe", gameExe);
  }, [gameExe]);
  useEffect(() => {
    if (installationsDir) localStorage.setItem("tl-installs", installationsDir);
  }, [installationsDir]);

  // Everything that lives behind the login wall: paths, installations, versions.
  // The backend refuses all of it while no account is stored, so it runs on
  // startup only when a session was restored, and again right after a login.
  // Paths come from localStorage rather than state so a login-time call never
  // reads a stale closure.
  async function bootstrapWorkspace() {
    // Resolve first-run defaults for this machine (no hardcoded paths). Only
    // fill blanks; a user's saved paths always win.
    let dir = localStorage.getItem("tl-installs") || "";
    let exe = localStorage.getItem("tl-game-exe") || "";
    if (!dir || !exe) {
      try {
        const sp = await invoke<{ installations_dir: string; game_exe: string }>("suggested_paths");
        if (!dir) { dir = sp.installations_dir; setInstallationsDir(dir); }
        if (!exe) { exe = sp.game_exe; setGameExe(exe); }
        say(`Using default paths for this machine.`);
      } catch (e) {
        say(`Could not resolve default paths: ${e}`);
      }
    }
    const found = await refreshInstalls(dir);
    // First run with nothing adopted yet: look for other launchers to import.
    if (found === 0) {
      try {
        const dl = await invoke<DetectedLauncher[]>("detect_launchers");
        setDetected(dl);
        if (dl.length) say(`Detected ${dl.map((d) => `${d.count} from ${d.launcher}`).join(", ")}.`);
      } catch (e) {
        say(`Launcher detection error: ${e}`);
      }
    }
    fetchVersions();
  }

  useEffect(() => {
    (async () => {
      // The notice gates everything, including the login form. A read error
      // fails toward showing it again: re-consent is harmless, silence is not.
      try {
        setAppEulaOk((await invoke<string | null>("eula_status")) === APP_EULA_VERSION);
      } catch {
        setAppEulaOk(false);
      }
      const acct = await invoke<Account | null>("get_account");
      if (acct) {
        setAccount(acct);
        say(`Restored session for ${acct.playername}.`);
        await bootstrapWorkspace();
      }
      setAuthChecked(true);
    })();
    const un = listen<{ done: number; total: number }>("check-progress", (e) => setProgress(e.payload));
    const un2 = listen<{ modid: string; received: number; total: number }>("install-progress", (e) => {
      const { modid, received, total } = e.payload;
      setInstalling({ modid, pct: total > 0 ? Math.min(100, (received / total) * 100) : -1 });
    });
    const un3 = listen<{ version: string; phase: string; received: number; total: number }>("version-progress", (e) => {
      const { version, phase, received, total } = e.payload;
      setVersionProgress({ version, phase, pct: total > 0 ? Math.min(100, (received / total) * 100) : -1 });
    });
    const un4 = listen<{ version: string; phase: string; detail: string }>("optimum-progress", (e) => {
      const { version, phase, detail } = e.payload;
      if (phase === "done" || phase === "failed" || phase === "toolchain-done") {
        setOptimumPhase(null);
        say(detail);
        toast(detail, undefined, phase !== "failed");
        setOptimumTick((t) => t + 1);
      } else if (phase === "fallback") {
        say(`${version}: ${detail}`);
        toast(detail, undefined, false);
      } else {
        setOptimumPhase(detail);
      }
    });
    const un5 = listen<{ pack_id: string; phase: string; detail: string; done: number; total: number; received: number; bytes_total: number }>("pack-progress", (e) => {
      const { phase, detail, done, total, received, bytes_total } = e.payload;
      setPackProgress({ phase, detail, done, total, received, bytesTotal: bytes_total });
    });
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      un4.then((f) => f());
      un5.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (!target && installs.length) setTarget(installs[0].path);
  }, [installs, target]);
  // One launcher-update check per run, once signed in.
  const updateChecked = useRef(false);
  useEffect(() => {
    if (account && !updateChecked.current) {
      updateChecked.current = true;
      checkLauncherUpdate();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account]);
  // First visit to the Mods tab: load the most-downloaded listing so the page
  // never opens empty and ambiguous ("did it fetch? do I have to search?").
  useEffect(() => {
    if (view === "mods" && account && !searched && !searching) doSearch();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, account]);
  useEffect(() => {
    if (account && view === "settings") refreshOptimum();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, view, gameVersion, optimumTick]);
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

  useEffect(() => {
    if (view === "worlds" && target) refreshWorlds(target);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, target]);

  useEffect(() => {
    if (view === "market") loadPacks();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  useEffect(() => {
    if (view === "servers" && serverTab === "public") loadPublicServers();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, serverTab]);

  useEffect(() => {
    if (view === "servers") loadPrivateServers();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  const toggle = (modid: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(modid)) n.delete(modid);
      else n.add(modid);
      return n;
    });

  // The auth server answers with wire codes ("invalidemailorpassword"); map
  // them to the same human messages the game shows, plus one Translocator
  // addition. Verified 2026-08-10: an account that signs into the website
  // fine but owns no game copy gets this same "invalid" answer from
  // /v2/gamelogin - the game's auth deliberately doesn't distinguish "no
  // purchase" from "wrong password", so the message must cover both.
  const LOGIN_REASONS: Record<string, string> = {
    invalidemailorpassword:
      "Invalid email or password - or this account doesn't own Vintage Story. Website, forum, and ModDB sign-ins work without a purchase, but the game login Translocator uses only accepts accounts with the game on them.",
    missingemailorpassword: "Missing email or password.",
    requirelogincode:
      "New location detected: an access code was sent to your email. Re-enter your password and add the code.",
    wronglogincode: "Invalid account access code. Try again, or request a new code after about 65 minutes.",
    requiretotpcode: "Enter the access code from your authenticator app.",
    wrongtotpcode: "Incorrect authenticator code. Please try again.",
    ipchanged: "Your IP address changed during login (mobile/WiFi switch?). Please try again.",
    blocked: "This IP address has been blocked by the account server. Contact Vintage Story support to unblock it.",
    temporarilyblocked: "This IP address is temporarily blocked by the account server. Try again later.",
    bruteforceprotected: "Over 10 failed attempts. Wait 20 minutes before trying again.",
    cantconnect: "Could not reach the Vintage Story account server. Check your internet connection.",
  };
  const friendlyLoginReason = (r: string) => LOGIN_REASONS[r.trim().toLowerCase()] || r;

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
        await bootstrapWorkspace();
      } else if (res.status === "needsTotp") {
        setPrelogintoken(res.prelogintoken);
        say(`2FA required${res.reason ? `: ${res.reason}` : ""}. Enter your TOTP code.`);
      } else {
        say(`Login failed: ${res.reason}`);
        toast(friendlyLoginReason(res.reason), undefined, false);
      }
    } catch (e) {
      say(`Error: ${e}`);
      toast(`${e}`, undefined, false);
    } finally {
      setBusy(false);
    }
  }
  async function doLogout() {
    try {
      await invoke("logout");
    } catch (e) {
      say(`Logout error: ${e}`);
    }
    setAccount(null);
    setView("installations");
    say("Logged out.");
  }
  async function refreshInstalls(dirOverride?: string): Promise<number> {
    const dir = dirOverride ?? installationsDir;
    if (!dir) return 0; // paths not resolved yet
    try {
      const list = await invoke<InstallationCard[]>("list_installations", { installationsDir: dir, defaultVersion: gameVersion });
      setInstalls(list);
      say(`Found ${list.length} installation(s).`);
      return list.length;
    } catch (e) {
      say(`Error listing installs: ${e}`);
      return 0;
    }
  }
  // Re-scan for other launchers on demand (Settings). Surfaces the import
  // banner on the Installations screen if any are found.
  async function scanLaunchers() {
    setBusy(true);
    try {
      const dl = await invoke<DetectedLauncher[]>("detect_launchers");
      setDetected(dl);
      if (dl.length) {
        setView("installations");
        toast(`Found ${dl.map((d) => `${d.count} from ${d.launcher}`).join(", ")}`);
      } else {
        toast("No VS Launcher or StoryForge installations found", undefined, false);
      }
    } catch (e) {
      say(`Launcher scan error: ${e}`);
    } finally {
      setBusy(false);
    }
  }
  // Point Translocator at a detected launcher's folder and adopt in place,
  // carrying over each install's settings from that launcher's config. No data
  // is copied or moved; a fresh session is stamped at launch.
  async function importFromLauncher(d: DetectedLauncher) {
    setBusy(true);
    try {
      setInstallationsDir(d.installations_dir);
      const res = await invoke<{ enriched: number; launcher: string | null }>("import_from_launcher", { installationsDir: d.installations_dir });
      const found = await refreshInstalls(d.installations_dir);
      setDetected([]);
      setView("installations");
      const carried = res.enriched > 0 ? ` (${res.enriched} with settings carried over)` : "";
      say(`Imported ${found} installation(s) from ${d.launcher}${carried}.`);
      toast(`Imported ${found} from ${d.launcher}${carried}`);
    } catch (e) {
      say(`Import error: ${e}`);
      toast(`Import failed: ${e}`, undefined, false);
    } finally {
      setBusy(false);
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
  async function refreshWorlds(installDir: string) {
    setWorldsBusy(true);
    try {
      setWorlds(await invoke<WorldInfo[]>("list_worlds", { installDir }));
    } catch (e) {
      say(`Worlds list error: ${e}`);
      setWorlds([]);
    } finally {
      setWorldsBusy(false);
    }
  }
  // Copy a single world into the install's backup folder before a risky change.
  async function backupWorld(w: WorldInfo) {
    setBusy(true);
    try {
      const dest = await invoke<string>("backup_world", { installDir: target, worldPath: w.path });
      say(`Backed up "${w.name}" → ${dest}`);
      toast(`Backed up "${w.name}"`);
    } catch (e) {
      say(`World backup error: ${e}`);
      toast(`Backup failed: ${e}`, undefined, false);
    } finally {
      setBusy(false);
    }
  }
  async function deleteWorld(w: WorldInfo) {
    setConfirmDeleteWorld(null);
    setBusy(true);
    try {
      await invoke("delete_world", { installDir: target, worldPath: w.path });
      say(`Deleted world "${w.name}".`);
      toast(`Deleted "${w.name}"`);
      await refreshWorlds(target);
    } catch (e) {
      say(`Delete world error: ${e}`);
      toast(`Delete failed: ${e}`, undefined, false);
    } finally {
      setBusy(false);
    }
  }
  // ---- Market ----
  async function loadPacks(force = false) {
    if (packsBusy) return;
    if (packsLoaded && !force) return;
    setPacksBusy(true);
    setPacksError(null);
    try {
      setPacks(await invoke<PackSummary[]>("hub_list_packs", { hubUrl: HUB_URL }));
      setPacksLoaded(true);
    } catch (e) {
      setPacksError(String(e));
    } finally {
      setPacksBusy(false);
    }
  }
  async function loadPublicServers(force = false) {
    if (serversBusy) return;
    if (serversLoaded && !force) return;
    setServersBusy(true);
    setServersError(null);
    try {
      setPublicServers(await invoke<PublicServer[]>("list_public_servers"));
      setServersLoaded(true);
    } catch (e) {
      setServersError(String(e));
    } finally {
      setServersBusy(false);
    }
  }
  // Open the join dialog for a server, defaulting the installation to one on the
  // server's game version (falling back to the current target, then the first).
  function openJoin(s: PublicServer) {
    const match =
      installs.find((i) => i.meta.version === s.game_version) ||
      installs.find((i) => i.path === target) ||
      installs[0];
    setJoinServer(s);
    setJoinInstall(match?.path ?? "");
    setJoinPassword("");
  }
  async function connectServer(name: string, address: string, installDir: string, password: string) {
    if (!account) return;
    setJoinServer(null);
    setBusy(true);
    const inst = installs.find((i) => i.path === installDir);
    try {
      say(`▶ Connecting to ${name || address} via ${inst?.meta.name ?? installDir} ...`);
      const res = await invoke<PlayResult>("connect_server", { gameExe, installDir, address, password: password || null });
      if (res.status === "needsVersion") {
        toast(`Vintage Story ${res.version} isn't downloaded yet. Press Play on the installation once to fetch it, then join.`, undefined, false);
      } else if (res.status === "needsRelogin") {
        say(`✗ Session rejected by server (${res.reason}). Re-login needed.`);
        toast("Session expired. Sign in again to continue.", undefined, false);
        setAccount(null);
      } else {
        say(`■ ${inst?.meta.name ?? "Game"} exited (code ${res.exit_code}).`);
        await refreshInstalls();
        if (res.rotated) {
          setAccount(res.account);
          say("↻ Game rotated the session; captured the new key.");
        }
      }
    } catch (e) {
      say(`Connect error: ${e}`);
      toast(`Could not connect: ${e}`, undefined, false);
    } finally {
      setBusy(false);
    }
  }
  async function addServerToInstall(name: string, address: string, installDir: string, password: string) {
    setJoinServer(null);
    const inst = installs.find((i) => i.path === installDir);
    try {
      await invoke("add_server_to_install", { installDir, name: name || address, address, password: password || null });
      say(`Added ${name || address} to ${inst?.meta.name ?? installDir}'s server list.`);
      toast(`Added to ${inst?.meta.name ?? "installation"}; it'll show in the in-game server list`);
    } catch (e) {
      say(`Add server error: ${e}`);
      toast(`Could not add server: ${e}`, undefined, false);
    }
  }
  // ---- Private (saved) servers ----
  async function loadPrivateServers() {
    try {
      setPrivateServers(await invoke<PrivateServer[]>("list_private_servers"));
    } catch (e) {
      say(`Saved servers load error: ${e}`);
    }
  }
  function openAddPrivate() {
    setPrivDraft({ id: crypto.randomUUID(), name: "", address: "", password: "", install_path: target || installs[0]?.path || "" });
  }
  async function savePrivateServer(s: PrivateServer) {
    const address = s.address.trim();
    if (!address) return;
    try {
      const list = await invoke<PrivateServer[]>("save_private_server", { server: { ...s, address, name: s.name.trim() } });
      setPrivateServers(list);
      setPrivDraft(null);
      toast(`Saved ${s.name.trim() || address}`);
    } catch (e) {
      say(`Save server error: ${e}`);
      toast(`Could not save: ${e}`, undefined, false);
    }
  }
  async function removePrivateServer(id: string) {
    try {
      setPrivateServers(await invoke<PrivateServer[]>("remove_private_server", { id }));
      toast("Server removed");
    } catch (e) {
      say(`Remove server error: ${e}`);
    }
  }
  // ---- Curator ----
  // Enter the publish flow: default the source install, prefill author/version
  // from what we know, and list its config files as override candidates.
  async function openCurator() {
    const install = curInstall || target || installs[0]?.path || "";
    setCurInstall(install);
    setCurPreview(null);
    setView("curator");
    setCurMeta((m) => ({
      ...m,
      author: m.author || account?.playername || "",
      game_version: installs.find((i) => i.path === install)?.meta.version || m.game_version,
    }));
    try {
      setPubStatus(await invoke<PublisherStatus>("publisher_status"));
    } catch { /* non-fatal */ }
    if (install) {
      try { setCurConfigs(await invoke<string[]>("list_config_files", { installDir: install })); }
      catch { setCurConfigs([]); }
    }
  }
  // Owner path: seed the Curator from a pack's published manifest, so a new
  // version starts from everything already posted (id, page copy, links,
  // server, override picks) and only the source re-detection is fresh. The Hub
  // still enforces ownership at publish (the signature must come from the
  // pack's account); this just saves re-typing what is already published.
  function bumpPatch(v: string): string {
    const parts = v.trim().split(".");
    const last = parseInt(parts[parts.length - 1], 10);
    if (Number.isNaN(last)) return v;
    parts[parts.length - 1] = String(last + 1);
    return parts.join(".");
  }
  async function startPackUpdate() {
    if (!packManifest) return;
    const p = packManifest.pack;
    const l = packManifest.links ?? packDetail?.links ?? {};
    setCurMeta({
      id: p.id,
      name: p.name,
      version: bumpPatch(packDetail?.latest_version || p.version),
      author: p.author,
      summary: p.summary || "",
      description: p.description || "",
      tags: (p.tags || []).join(", "),
      game_version: p.game_version,
      icon: p.icon || "",
      strict: !!p.strict,
    });
    setCurIdEdited(true); // the id is published identity; never re-derive it from the name
    setCurLinks({ website: l?.website || "", discord: l?.discord || "", source: l?.source || "", donate: l?.donate || "" });
    setCurServer(
      packManifest.server
        ? { address: packManifest.server.address, auto_add: packManifest.server.auto_add }
        : { address: "", auto_add: false }
    );
    const install = curInstall || target || installs[0]?.path || "";
    setCurInstall(install);
    setCurPreview(null);
    setView("curator");
    try { setPubStatus(await invoke<PublisherStatus>("publisher_status")); } catch { /* non-fatal */ }
    if (install) {
      try { setCurConfigs(await invoke<string[]>("list_config_files", { installDir: install })); }
      catch { setCurConfigs([]); }
    }
    setCurSelected(new Set((packManifest.overrides || []).map((o) => o.path)));
    say(`Updating ${p.name}: fields loaded from the published pack, version drafted as ${bumpPatch(packDetail?.latest_version || p.version)}.`);
  }
  // Switching the source install invalidates the preview and re-lists configs.
  async function pickCuratorInstall(path: string) {
    setCurInstall(path);
    setCurPreview(null);
    setCurSelected(new Set());
    const v = installs.find((i) => i.path === path)?.meta.version;
    if (v) setCurMeta((m) => ({ ...m, game_version: v }));
    try { setCurConfigs(await invoke<string[]>("list_config_files", { installDir: path })); }
    catch { setCurConfigs([]); }
  }
  function setCurName(name: string) {
    setCurMeta((m) => ({ ...m, name, id: curIdEdited ? m.id : slugify(name) }));
  }
  async function buildManifest() {
    if (!curInstall) return;
    setCurBusy(true);
    setCurPreview(null);
    try {
      say(`Curating pack from ${installs.find((i) => i.path === curInstall)?.meta.name ?? curInstall} ...`);
      const links = { website: curLinks.website.trim() || null, discord: curLinks.discord.trim() || null, source: curLinks.source.trim() || null, donate: curLinks.donate.trim() || null };
      const hasLinks = Object.values(links).some(Boolean);
      const preview = await invoke<CuratorPreview>("curate_pack", {
        installDir: curInstall,
        pack: {
          id: curMeta.id.trim(),
          name: curMeta.name.trim(),
          version: curMeta.version.trim(),
          author: curMeta.author.trim(),
          summary: curMeta.summary.trim(),
          description: curMeta.description.trim(),
          tags: curMeta.tags.split(",").map((t) => t.trim()).filter(Boolean),
          game_version: curMeta.game_version.trim(),
          min_launcher_version: "",
          strict: curMeta.strict,
          icon: curMeta.icon.trim(),
        },
        server: curServer.address.trim() ? { address: curServer.address.trim(), auto_add: curServer.auto_add } : null,
        links: hasLinks ? links : null,
        overridePaths: Array.from(curSelected),
      });
      setCurPreview(preview);
      localStorage.setItem("tl-curator-meta", JSON.stringify(curMeta));
      say(`Manifest built: ${preview.resolved_count} resolved, ${preview.unresolved.length} unresolved.`);
    } catch (e) {
      say(`Curate error: ${e}`);
      toast(`Could not build the manifest: ${e}`, undefined, false);
    } finally {
      setCurBusy(false);
    }
  }
  async function publishManifest() {
    if (!curPreview) return;
    setPubBusy(true);
    try {
      await invoke<string>("publish_pack", { hubUrl: HUB_URL, manifest: curPreview.manifest });
      say(`✓ Published ${curPreview.manifest.pack.name} v${curPreview.manifest.pack.version}.`);
      toast(`Published ${curPreview.manifest.pack.name} v${curPreview.manifest.pack.version}`);
      await loadPacks(true);
      setView("market");
    } catch (e) {
      say(`Publish error: ${e}`);
      const msg = String(e);
      if (msg.includes("unknown publisher uid") || msg.includes("no active signing keys")) {
        toast("This device isn't registered yet. Click Register this device, then publish again.", undefined, false);
      } else {
        toast(`Publish failed: ${e}`, undefined, false);
      }
    } finally {
      setPubBusy(false);
    }
  }
  // Bind this machine's signing key to the signed-in VS account (idempotent:
  // the Hub re-proves the account and re-binds the same key harmlessly).
  async function registerDevice() {
    setRegistering(true);
    try {
      const res = await invoke<{ publisher?: { playername?: string }; key_fingerprint?: string }>(
        "register_publisher", { hubUrl: HUB_URL }
      );
      setPubStatus(await invoke<PublisherStatus>("publisher_status"));
      toast(`This device can now publish as ${res.publisher?.playername ?? account?.playername ?? "you"}`);
      say(`Registered signing key ${res.key_fingerprint ?? ""} with the Hub.`);
    } catch (e) {
      say(`Device registration error: ${e}`);
      toast(`Registration failed: ${e}`, undefined, false);
    } finally {
      setRegistering(false);
    }
  }
  async function openPack(id: string) {
    setSelectedPack(id);
    setView("pack");
    setPackBusy(true);
    setPackError(null);
    setPackDetail(null);
    setPackManifest(null);
    try {
      const [detail, manifest] = await Promise.all([
        invoke<PackDetail>("hub_pack", { hubUrl: HUB_URL, id }),
        invoke<PackManifest>("hub_pack_manifest", { hubUrl: HUB_URL, id, version: null }),
      ]);
      setPackDetail(detail);
      setPackManifest(manifest);
    } catch (e) {
      setPackError(String(e));
    } finally {
      setPackBusy(false);
    }
  }

  // Check for a launcher update once per run, quietly: failures (offline,
  // endpoint not live yet, unset pubkey in dev) log to the console feed and
  // never toast, because most runs there is simply nothing to say.
  async function checkLauncherUpdate() {
    try {
      const upd = await checkAppUpdate();
      if (upd) {
        setAppUpdate(upd);
        say(`Translocator ${upd.version} is available (running ${upd.currentVersion}).`);
      }
    } catch (e) {
      say(`Launcher update check skipped: ${e}`);
    }
  }
  async function installLauncherUpdate() {
    if (!appUpdate) return;
    setAppUpdating(-1);
    try {
      let total = 0;
      let got = 0;
      await appUpdate.downloadAndInstall((ev) => {
        if (ev.event === "Started") { total = ev.data.contentLength ?? 0; }
        else if (ev.event === "Progress") { got += ev.data.chunkLength; setAppUpdating(total > 0 ? Math.min(100, (got / total) * 100) : -1); }
        else if (ev.event === "Finished") { setAppUpdating(100); }
      });
      await relaunch();
    } catch (e) {
      setAppUpdating(null);
      say(`Launcher update failed: ${e}`);
      toast(`Update failed: ${e}`, undefined, false);
    }
  }

  // The one-time Optimum choice. Either way the answer is recorded and the
  // card never returns; Settings owns the toggle from here on.
  async function chooseOptimumDefault(useIt: boolean) {
    try {
      await invoke("set_use_optimum", { enabled: useIt });
    } catch (e) {
      say(`Could not save the optimized-client choice: ${e}`);
    }
    localStorage.setItem("tl-optimum-onboarded", "1");
    setOptimumOnboarded(true);
    if (useIt) {
      toast("Optimized client enabled. It builds in the background per game version.");
      // Complete the consent chain in one sitting: background builds stay
      // (silently) skipped until Optimum's own notice is accepted, so a card
      // opt-in that never visits Settings would otherwise do nothing.
      try {
        const st = await invoke<OptimumStatus>("optimum_status", { version: null });
        if (!st.eula_accepted) await showEula();
      } catch {
        /* Settings can finish the consent later; the toggle is saved. */
      }
    } else {
      toast("Using the standard client. Change anytime in Settings.");
    }
  }

  // Open the install-pack modal with sensible defaults: the pack's name, the
  // usual settings-seed source, optionals off until toggled.
  function openInstallPack() {
    if (!packManifest) return;
    setPackInstallName(packManifest.pack.name);
    setPackSeed(seedSettings ? seedSource().value : "");
    setPackOptionals({});
    setPackProgress(null);
    setPackInstallOpen(true);
  }

  // The install-pack flow: the backend stages + verifies every pinned mod
  // before anything is placed, then freezes the install to the pack version.
  async function doInstallPack() {
    if (!packManifest || !selectedPack) return;
    const name = packInstallName.trim() || packManifest.pack.name;
    setPackInstalling(true);
    setPackProgress(null);
    try {
      const path = await invoke<string>("install_pack", {
        installationsDir,
        hubUrl: HUB_URL,
        packId: selectedPack,
        name,
        seedFrom: packSeed || null,
        optionalChoices: packOptionals,
      });
      say(`Installed ${packManifest.pack.name} v${packManifest.pack.version} at ${path}`);
      toast(`${packManifest.pack.name} installed. Ready to play.`);
      setPackInstallOpen(false);
      await refreshInstalls();
      setTarget(path);
      setView("installations");
    } catch (e) {
      say(`Pack install error: ${e}`);
      toast(`Install failed: ${e}`, undefined, false);
    } finally {
      setPackInstalling(false);
      setPackProgress(null);
    }
  }

  // ModDB descriptions arrive as author-written HTML. It is parsed INERT
  // (DOMParser documents have no browsing context: nothing loads, nothing
  // executes) and rebuilt from a whitelist: author content only ever becomes
  // text nodes inside elements WE create, never markup. Structure survives
  // (headings, lists, emphasis, links via the https-only open_url gate);
  // images, tables, styles, and everything unknown flatten to their text.
  function renderRichText(html: string): ReactNode {
    const doc = new DOMParser().parseFromString(html, "text/html");
    let key = 0;
    const render = (node: Node): ReactNode => {
      if (node.nodeType === Node.TEXT_NODE) return node.textContent;
      if (node.nodeType !== Node.ELEMENT_NODE) return null;
      const el = node as Element;
      const k = ++key;
      const tag = el.tagName.toLowerCase();
      // Never render even the TEXT of these: script bodies are code, not prose.
      if (tag === "script" || tag === "style" || tag === "iframe" || tag === "object" || tag === "embed") return null;
      const kids = Array.from(el.childNodes).map(render);
      switch (tag) {
        case "h1": case "h2": return <div className="rt-h1" key={k}>{kids}</div>;
        case "h3": case "h4": case "h5": case "h6": return <div className="rt-h2" key={k}>{kids}</div>;
        case "p": return <p className="rt-p" key={k}>{kids}</p>;
        case "br": return <br key={k} />;
        case "strong": case "b": return <strong key={k}>{kids}</strong>;
        case "em": case "i": return <em key={k}>{kids}</em>;
        case "u": return <u key={k}>{kids}</u>;
        case "s": case "strike": case "del": return <s key={k}>{kids}</s>;
        case "ul": case "ol": return <ul className="rt-list" key={k}>{kids}</ul>;
        case "li": return <li key={k}>{kids}</li>;
        case "code": return <code key={k}>{kids}</code>;
        case "pre": return <pre className="rt-pre" key={k}>{kids}</pre>;
        case "blockquote": return <blockquote className="rt-quote" key={k}>{kids}</blockquote>;
        case "a": {
          const href = el.getAttribute("href") || "";
          if (/^https?:\/\//i.test(href)) {
            // Badge links (an image as the whole label) would render empty
            // once images are dropped; fall back to alt text or the hostname.
            let label: ReactNode = kids;
            if (!el.textContent?.trim()) {
              const alt = Array.from(el.querySelectorAll("img")).map((i) => i.getAttribute("alt") || "").find(Boolean);
              label = alt || href.replace(/^https?:\/\/(www\.)?/i, "").split("/")[0];
            }
            return <button className="link" key={k} onClick={() => invoke("open_url", { url: href })}>{label} ↗</button>;
          }
          return <span key={k}>{kids}</span>;
        }
        case "img": return null;
        case "tr": return <div key={k}>{kids}</div>;
        default: return <span key={k}>{kids}</span>;
      }
    };
    return Array.from(doc.body.childNodes).map(render);
  }

  // Expand a search result into its discovery detail (description + recent
  // releases), fetched once and cached for the session.
  async function toggleModDetail(m: ModSummary) {
    if (openMod === m.modidstr) {
      setOpenMod(null);
      return;
    }
    setOpenMod(m.modidstr);
    if (!modDetails[m.modidstr]) {
      setModDetails((d) => ({ ...d, [m.modidstr]: "loading" }));
      try {
        const det = await invoke<ModDetailView>("mod_detail", { modidstr: m.modidstr });
        setModDetails((d) => ({ ...d, [m.modidstr]: det }));
      } catch (e) {
        say(`Mod detail error: ${e}`);
        toast(`Could not load details: ${e}`, undefined, false);
        setModDetails((d) => {
          const n = { ...d };
          delete n[m.modidstr];
          return n;
        });
        setOpenMod(null);
      }
    }
  }

  // Open a mod's ModDB page (URL resolved backend-side: the page route wants
  // the assetid, which manifests don't carry).
  async function openModPage(modidstr: string) {
    try {
      const url = await invoke<string>("mod_page_url", { modidstr });
      await invoke("open_url", { url });
    } catch (e) {
      say(`ModDB page lookup error: ${e}`);
      toast(`Could not open the ModDB page: ${e}`, undefined, false);
    }
  }

  // Open a mod author's donation link, looked up on demand.
  async function tipModAuthor(modidstr: string, name: string) {
    try {
      const links = await invoke<string[]>("mod_donations", { modidstr });
      if (links.length) {
        await invoke("open_url", { url: links[0] });
        say(`♥ Tip ${name}: ${links[0]}`);
      } else {
        toast(`${name}'s author lists no tip link on ModDB`, undefined, false);
      }
    } catch (e) {
      say(`Tip lookup error: ${e}`);
    }
  }

  // Native folder picker for the installations directory; re-lists on pick.
  async function browseInstallsDir() {
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: "Choose your installations folder", defaultPath: installationsDir || undefined });
      if (typeof picked === "string") {
        setInstallationsDir(picked);
        await refreshInstalls(picked);
      }
    } catch (e) {
      say(`Folder picker error: ${e}`);
    }
  }
  // Native file picker for the fallback game executable.
  async function browseGameExe() {
    try {
      const picked = await openDialog({
        directory: false,
        multiple: false,
        title: "Choose Vintagestory.exe",
        defaultPath: gameExe || undefined,
        filters: [{ name: "Vintage Story", extensions: ["exe"] }],
      });
      if (typeof picked === "string") setGameExe(picked);
    } catch (e) {
      say(`File picker error: ${e}`);
    }
  }
  async function fetchVersions() {
    try {
      setAvailableVersions(await invoke<AvailableVersion[]>("list_available_versions"));
      setCachedVersions(await invoke<string[]>("list_cached_versions"));
    } catch (e) {
      say(`Version list error: ${e}`);
    }
  }
  // Ensure a version is downloaded + installed in the shared cache. Returns
  // false if it failed (caller aborts). Already-cached versions return instantly.
  async function ensureVersion(version: string): Promise<boolean> {
    const av = availableVersions.find((v) => v.version === version);
    if (av?.cached || cachedVersions.includes(version)) return true;
    if (!av) {
      say(`Version ${version} is not in the manifest; can't auto-install.`);
      toast(`Can't install ${version}: not found`, undefined, false);
      return false;
    }
    setVersionProgress({ version, phase: "download", pct: -1 });
    try {
      say(`Downloading + installing game ${version} (${av.filesize}) ...`);
      await invoke("ensure_version", { version, url: av.url, md5: av.md5 });
      say(`✓ Game ${version} is ready in the cache.`);
      toast(`Installed game ${version}`);
      await fetchVersions();
      return true;
    } catch (e) {
      say(`Version install error: ${e}`);
      toast(`Failed to install ${version}`, undefined, false);
      return false;
    } finally {
      setVersionProgress(null);
    }
  }
  // Where a new install inherits game settings from: your most-recently played
  // install, or the base game if you have none yet.
  function seedSource(): { label: string; value: string } {
    const played = installs
      .filter((i) => i.meta.last_played > 0)
      .sort((a, b) => b.meta.last_played - a.meta.last_played)[0];
    return played
      ? { label: played.meta.name, value: played.path }
      : { label: "your base Vintage Story install", value: "__base__" };
  }
  async function doCreate() {
    const name = createName.trim();
    if (!name || !createVersion) return;
    const ok = await ensureVersion(createVersion);
    if (!ok) return;
    try {
      const seed_from = seedSettings ? seedSource().value : null;
      const path = await invoke<string>("create_installation", { installationsDir, name, version: createVersion, seedFrom: seed_from });
      if (seed_from) say(`Copied game settings from ${seedSource().label}.`);
      say(`Created installation ${name} on ${createVersion}.`);
      toast(`Created ${name}`);
      setCreating(false);
      setCreateName("");
      await refreshInstalls();
      setTarget(path);
      setView("updates");
    } catch (e) {
      say(`Create error: ${e}`);
      toast(`Create failed: ${e}`, undefined, false);
    }
  }
  async function removeVersion(version: string) {
    try {
      await invoke("remove_version", { version });
      say(`Removed cached game ${version}.`);
      toast(`Removed game ${version}`);
      await fetchVersions();
    } catch (e) {
      say(`Remove version error: ${e}`);
    }
  }

  // ---- Optimum -------------------------------------------------------------
  async function refreshOptimum() {
    try {
      setOptimum(await invoke<OptimumStatus>("optimum_status", { version: gameVersion || null }));
    } catch {
      setOptimum(null); // signed out, or the wall said no: leave the panel quiet
    }
  }
  async function installToolchain() {
    setBusy(true);
    setOptimumPhase("Preparing the build toolchain...");
    try {
      await invoke<Prereqs>("provision_toolchain");
      say("Optimum build toolchain installed.");
    } catch (e) {
      say(`Toolchain error: ${e}`);
      toast(`Toolchain install failed: ${e}`, undefined, false);
    } finally {
      setOptimumPhase(null);
      setBusy(false);
      refreshOptimum();
    }
  }
  async function showEula() {
    setEulaBusy(true);
    setEulaAgreed(false);
    try {
      setEula(await invoke<EulaText>("optimum_eula", { version: gameVersion }));
    } catch (e) {
      say(`Optimum notice error: ${e}`);
      toast(`Could not fetch Optimum's notice: ${e}`, undefined, false);
    } finally {
      setEulaBusy(false);
    }
  }
  async function acceptEula() {
    if (!eula) return;
    try {
      await invoke("accept_optimum_eula", { release: eula.release });
      say(`Accepted the Optimum ${eula.release} end-user notice.`);
      setEula(null);
      await refreshOptimum();
    } catch (e) {
      toast(`Could not record acceptance: ${e}`, undefined, false);
    }
  }
  async function optimizeNow() {
    if (!gameVersion) return;
    setOptimumPhase("Starting...");
    try {
      await invoke<string>("optimize_version", { version: gameVersion });
    } catch (e) {
      setOptimumPhase(null);
      say(`Optimize error: ${e}`);
      toast(`Optimization failed: ${e}`, undefined, false);
    }
    refreshOptimum();
  }
  async function toggleOptimum(on: boolean) {
    setOptimum((o) => (o ? { ...o, use_optimum: on } : o));
    try {
      await invoke("set_use_optimum", { enabled: on });
    } catch (e) {
      toast(`Could not save that: ${e}`, undefined, false);
      refreshOptimum();
    }
  }

  async function deleteInstallation(card: InstallationCard) {
    setConfirmDelete(null);
    if (target === card.path) setTarget("");
    toast(`Deleting ${card.meta.name}…`);
    // The backend renames the folder out of the listing immediately, then
    // grinds through the actual removal off-thread. Refresh once the rename
    // has landed so the card vanishes right away; confirm again when the disk
    // work truly finishes.
    const done = invoke("delete_installation", { installationsDir, path: card.path });
    setTimeout(() => { void refreshInstalls(); }, 350);
    try {
      await done;
      say(`Deleted installation ${card.meta.name}.`);
      toast(`Deleted ${card.meta.name}`);
    } catch (e) {
      say(`Delete error: ${e}`);
      toast(`Delete failed: ${e}`, undefined, false);
    }
    await refreshInstalls();
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
      let res = await invoke<PlayResult>("play", { gameExe, installDir: inst.path });
      if (res.status === "needsVersion") {
        // Pinned version exists nowhere on this machine (not our cache, not an
        // adopted launcher's binaries): download it once, then retry the launch.
        say(`VS ${res.version} isn't on this machine yet; downloading it first.`);
        let list = availableVersions;
        if (!list.length) {
          try {
            list = await invoke<AvailableVersion[]>("list_available_versions");
            setAvailableVersions(list);
          } catch {
            list = [];
          }
        }
        const needed = res.version;
        const av = list.find((v) => v.version === needed);
        if (!av) {
          toast(`Vintage Story ${needed} isn't offered by the official version manifest`, undefined, false);
          return;
        }
        setVersionProgress({ version: av.version, phase: "download", pct: -1 });
        try {
          await invoke("ensure_version", { version: av.version, url: av.url, md5: av.md5 });
          say(`✓ Game ${av.version} is ready in the cache; launching.`);
        } catch (e) {
          say(`Version install error: ${e}`);
          toast(`Failed to install ${av.version}: ${e}`, undefined, false);
          return;
        } finally {
          setVersionProgress(null);
        }
        res = await invoke<PlayResult>("play", { gameExe, installDir: inst.path });
      }
      if (res.status === "needsVersion") {
        toast(`Could not launch: Vintage Story ${res.version} is still unavailable`, undefined, false);
      } else if (res.status === "needsRelogin") {
        say(`✗ Session rejected by server (${res.reason}). Re-login needed.`);
        toast("Session expired. Sign in again to continue.", undefined, false);
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
      // Safety net first (matching kind), so the restore itself is recoverable.
      try {
        if (b.kind === "full") {
          const m = installs.find((i) => i.path === target)?.meta;
          await invoke<string>("backup_install", { installDir: target, compression: m?.compression ?? 6, keep: m?.backups_limit ?? 5 });
        } else {
          await invoke<string>("backup_mods", { installDir: target });
        }
      } catch { /* non-fatal */ }
      say(`Restoring ${b.kind} backup ${b.id} ...`);
      await invoke("restore_backup", { installDir: target, id: b.id });
      say(`✓ Restored ${b.kind === "full" ? "installation" : "Mods folder"} to backup ${b.id}.`);
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
  async function backupFull() {
    if (!target) return;
    const m = installs.find((i) => i.path === target)?.meta;
    setBusy(true);
    setBackingUp(true);
    try {
      say(`Backing up the whole installation (this can take a moment) ...`);
      const id = await invoke<string>("backup_install", { installDir: target, compression: m?.compression ?? 6, keep: m?.backups_limit ?? 5 });
      say(`✓ Full backup ${id} created.`);
      toast(`Backed up the whole installation`);
      await refreshBackups(target);
    } catch (e) {
      say(`Full backup error: ${e}`);
      toast(`Backup failed: ${e}`, undefined, false);
    } finally {
      setBusy(false);
      setBackingUp(false);
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
    setSearching(true);
    try {
      say(search.trim() ? `Searching ModDB for "${search}" ...` : "Loading the most downloaded mods from ModDB ...");
      setResults(await invoke<ModSummary[]>("search_mods", { text: search }));
      setSearched(true);
    } catch (e) {
      say(`Search error: ${e}`);
    } finally {
      setSearching(false);
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

  // ---- derived state for the server browser ----
  const SERVER_CAP = 150; // keep the DOM light; refine with search/filter
  const serverVersions = useMemo(() => {
    const set = new Set(publicServers.map((s) => s.game_version).filter(Boolean));
    return Array.from(set).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
  }, [publicServers]);
  const filteredServers = useMemo(() => {
    const q = serverSearch.trim().toLowerCase();
    const list = publicServers.filter((s) => {
      if (serverVersionFilter && s.game_version !== serverVersionFilter) return false;
      if (q && !`${s.name} ${s.address} ${s.description}`.toLowerCase().includes(q)) return false;
      return true;
    });
    return list.sort((a, b) =>
      serverSort === "players" ? b.players - a.players || a.name.localeCompare(b.name) : a.name.localeCompare(b.name)
    );
  }, [publicServers, serverSearch, serverVersionFilter, serverSort]);
  const shownServers = filteredServers.slice(0, SERVER_CAP);

  const targetInfo = installs.find((i) => i.path === target);
  const targetName = targetInfo?.meta.name ?? "";
  const targetManaged = targetInfo?.meta.managed_by || "";
  const targetStrict = !!targetManaged && !!targetInfo?.meta.pack_strict;
  const NAV: { id: View; label: string; count?: number }[] = [
    { id: "installations", label: "Installations", count: installs.length },
    { id: "updates", label: "Mod Updates", count: updates.length || undefined },
    { id: "worlds", label: "Worlds", count: worlds.length || undefined },
    { id: "servers", label: "Servers" },
    { id: "market", label: "Modpack Hub", count: packs.length || undefined },
    { id: "mods", label: "Mods", count: installed.length || undefined },
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

  // The END-USER NOTICE, ahead of even the login wall: first launch (or a
  // materially changed notice) shows it once, acceptance is version-stamped.
  if (appEulaOk !== true) {
    return (
      <div className="shell">
        <WindowChrome />
        <div className="wall">
          {appEulaOk === false && (
            <div className="wall-card eula-card">
              <div className="wall-brand">
                <img src={gearIcon} className="brand-gear" alt="" />
                <div>
                  <div className="wall-name">Translocator</div>
                  <div className="muted">End-user notice and license agreement</div>
                </div>
              </div>
              <div className="eula-scroll">
                {APP_EULA.map((s) => (
                  <div key={s.h}>
                    <div className="eula-h">{s.h}</div>
                    <p className="eula-p">{s.p}</p>
                  </div>
                ))}
              </div>
              <div className="acts">
                <button className="btn" onClick={() => getCurrentWindow().close()}>Decline and close</button>
                <button
                  className="cta"
                  onClick={async () => {
                    try {
                      await invoke("accept_app_eula", { version: APP_EULA_VERSION });
                      setAppEulaOk(true);
                    } catch (e) {
                      toast(`Could not record acceptance: ${e}`, undefined, false);
                    }
                  }}
                >
                  I agree, continue
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    );
  }

  // The login wall: with no stored account the launcher is the login form and
  // nothing else. No nav, no installations, no browsing. The backend refuses
  // every other command anyway, so there is no half-usable state to fall into.
  // WindowChrome stays mounted or the frameless window cannot be dragged or
  // closed from here.
  if (!authChecked || !account) {
    return (
      <div className="shell">
        <WindowChrome />
        <div className="wall">
          {authChecked && (
            <div className="wall-card">
              <div className="wall-brand">
                <img src={gearIcon} className="brand-gear" alt="" />
                <div>
                  <div className="wall-name">Translocator</div>
                  <div className="muted">Sign in with your Vintage Story account to continue.</div>
                </div>
              </div>
              <form onSubmit={(e) => { e.preventDefault(); if (!busy) doLogin(); }}>
                <label className="field"><span className="lab">Email</span><input type="email" autoComplete="username" autoFocus={!prelogintoken} value={email} onChange={(e) => setEmail(e.target.value)} /></label>
                <label className="field"><span className="lab">Password</span><input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} /></label>
                {prelogintoken && <label className="field"><span className="lab">2FA / TOTP code</span><input inputMode="numeric" autoComplete="one-time-code" autoFocus value={totp} onChange={(e) => setTotp(e.target.value)} /></label>}
                <button className="cta" type="submit" disabled={busy}>{prelogintoken ? "Submit 2FA code" : "Log in"}</button>
              </form>
              <div className="wall-themes">
                {THEMES.map((t) => (
                  <button key={t.id} className={"mini" + (theme === t.id ? " sel" : "")} onClick={() => setTheme(t.id)} title={t.desc}>{t.name}</button>
                ))}
              </div>
            </div>
          )}
        </div>
        <div className="toasts">
          {toasts.map((t) => (
            <div className="toast" key={t.id}>
              <span className="toast-msg">{t.ok !== false ? "\u2713 " : ""}{t.msg}</span>
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="shell">
      <WindowChrome />
      <div className="layout">
        <aside className="side">
          <div className="brand">
            <img src={gearIcon} className="brand-gear" alt="" />
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
          <nav>
            {NAV.map((n) => {
              const active = view === n.id || (n.id === "market" && (view === "pack" || view === "curator"));
              return (
                <button key={n.id} className={"navbtn" + (active ? " active" : "")} onClick={() => setView(n.id)}>
                  <span className="l">{n.label}</span>
                  <span className="c">{n.count ?? ""}</span>
                </button>
              );
            })}
          </nav>
          {appUpdate && !appUpdateDismissed && (
            <div className="upd-chip">
              <div className="upd-t">Translocator {appUpdate.version} is ready</div>
              {appUpdating === null ? (
                <div className="upd-acts">
                  <button className="cta" onClick={installLauncherUpdate}>Update and restart</button>
                  <button className="mini" onClick={() => setAppUpdateDismissed(true)}>Later</button>
                </div>
              ) : (
                <>
                  <div className="prog"><i className={appUpdating < 0 ? "indet" : ""} style={appUpdating >= 0 ? { width: `${appUpdating}%` } : { width: "40%" }} /></div>
                  <div className="upd-sub">Downloading update…</div>
                </>
              )}
            </div>
          )}
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
                  {!targetManaged && <button className="btn" disabled={busy || !target} onClick={checkUpdates}>Check for updates</button>}
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
                {targetManaged && (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>Managed by its pack</h3>
                    <p>
                      This installation is frozen to <b className="tab">{targetManaged}</b>, so every player stays matched to the pack.<br />
                      Updates arrive when the publisher ships a new pack version; per-mod updates are disabled here.
                    </p>
                  </div>
                )}
                {progress && (
                  <div className="checking">
                    <div className="checking-t">Consulting the ledger…</div>
                    <div className="prog"><i style={{ width: `${progress.total ? (progress.done / progress.total) * 100 : 0}%` }} /></div>
                    <div className="prog-n tab">{progress.done} / {progress.total} mods checked</div>
                  </div>
                )}

                {!targetManaged && !progress && updates.length === 0 && (
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
                      <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
                        <button className="btn" disabled={busy || !target} onClick={backupNow} title="Fast snapshot of just the Mods folder">Back up mods</button>
                        <button className="btn" disabled={busy || !target} onClick={backupFull} title="Compressed snapshot of the whole install (worlds, config, mods)">Back up everything</button>
                      </div>
                      {backingUp && (
                        <div className="checking" style={{ padding: "4px 0 10px" }}>
                          <div className="prog-n tab">Compressing the whole installation… (worlds can take a minute)</div>
                          <div className="prog"><i className="indet" style={{ width: "40%" }} /></div>
                        </div>
                      )}
                      {backups.length === 0 ? (
                        <p className="muted">No backups yet. A Mods backup is taken automatically before "Update all compatible".</p>
                      ) : (
                        <div className="list">
                          {backups.map((b) => (
                            <div className="li" key={b.id}>
                              <span>
                                <b className="nm">{b.created}</b>{" "}
                                <span className="meta">
                                  {b.kind === "full" ? `full install · ${fmtBytes(b.size)}` : `${b.mod_count} mod${b.mod_count === 1 ? "" : "s"} · ${fmtBytes(b.size)}`}
                                </span>
                              </span>
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
                <button className="btn" onClick={() => refreshInstalls()}>Refresh</button>
                <button className="cta" onClick={() => {
                  if (!availableVersions.length) fetchVersions();
                  setCreateVersion(availableVersions[0]?.version ?? gameVersion);
                  setCreateName("");
                  setCreating(true);
                }}>+ New installation</button>
              </div>
              <div className="view">
                {detected.length > 0 && (
                  <div className="import-banner">
                    <div className="ib-body">
                      <div className="ib-title">Found installations from another launcher</div>
                      {detected.map((d) => (
                        <div className="ib-row" key={d.installations_dir}>
                          <span>
                            <b>{d.count}</b> from <b>{d.launcher}</b>
                            {d.can_enrich ? " (settings carry over)" : ""}
                          </span>
                          <button className="cta" disabled={busy} onClick={() => importFromLauncher(d)}>Use these</button>
                        </div>
                      ))}
                      <div className="ib-note">Nothing is copied or moved. Translocator adopts them in place and stamps a fresh login at launch.</div>
                    </div>
                    <button className="ib-x" title="Dismiss" onClick={() => setDetected([])}>×</button>
                  </div>
                )}
                {installs.length === 0 ? (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>No installations found</h3>
                    <p>Point Translocator at your installations folder in Settings, or import from a launcher above.</p>
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
                            {inst.meta.version || "n/a"} · {inst.mod_count} mod{inst.mod_count === 1 ? "" : "s"}
                            {inst.has_session && <span style={{ color: "var(--ok)" }}> · ● session</span>}
                          </div>
                          {inst.meta.managed_by && (
                            <div className="inst-sub" title={inst.meta.pack_strict ? "Strict pack: the Mods folder is kept exactly matched to the pack manifest" : "Pack-managed: mod versions follow the pack publisher"}>
                              ⬡ {inst.meta.managed_by}
                            </div>
                          )}
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
                  <button className="btn" type="submit" disabled={searching}>{searching ? "Searching…" : "Search"}</button>
                </form>
                {searching && (
                  <div className="checking" style={{ padding: "0 0 10px" }}>
                    <div className="prog-n tab">{search.trim() ? `Searching ModDB for "${search}"…` : "Loading the most downloaded mods from ModDB…"}</div>
                    <div className="prog"><i className="indet" style={{ width: "40%" }} /></div>
                  </div>
                )}
                {!searching && searched && results.length === 0 && (
                  <p className="muted" style={{ marginBottom: 12 }}>No mods matched "{search}".</p>
                )}
                {!searching && results.length > 0 && !search.trim() && (
                  <p className="muted" style={{ marginBottom: 8 }}>Most downloaded on ModDB · search to find anything else</p>
                )}
                {results.length > 0 && (
                  <div className="list" style={{ marginBottom: 14 }}>
                    {results.map((m) => (
                      <div key={m.modid}>
                        <div className="li">
                          <span style={{ cursor: "pointer" }} onClick={() => toggleModDetail(m)}>
                            <span className="nm">{m.name}</span> <span className="meta">by {m.author} · {m.downloads.toLocaleString()} downloads</span>
                          </span>
                          <span style={{ display: "flex", gap: 8 }}>
                            <button className="mini" onClick={() => toggleModDetail(m)}>{openMod === m.modidstr ? "Hide ▴" : "Details ▾"}</button>
                            <button className="mini" onClick={() => doTip(m)} title="Show author donation link">♥ Tip</button>
                            <button className="cta" disabled={busy || !target || targetStrict} onClick={() => doInstall(m)}>Install</button>
                          </span>
                        </div>
                        {openMod === m.modidstr && (() => {
                          const det = modDetails[m.modidstr];
                          if (!det || det === "loading") return <div className="mod-detail meta">Loading details…</div>;
                          return (
                            <div className="mod-detail">
                              {m.summary && <div className="meta">{m.summary}</div>}
                              <div className="mod-desc">{det.text.trim() ? renderRichText(det.text) : "No description on ModDB."}</div>
                              <div className="meta">
                                {det.side && det.side !== "both" ? `${det.side}-side · ` : ""}
                                {det.releases[0] && <>latest {det.releases[0].modversion}{det.releases[0].tags.length ? ` for ${det.releases[0].tags[det.releases[0].tags.length - 1]}` : ""} · </>}
                                <button className="link" onClick={() => openModPage(m.modidstr)}>Open on ModDB ↗</button>
                              </div>
                            </div>
                          );
                        })()}
                      </div>
                    ))}
                  </div>
                )}
                {targetStrict && (
                  <p className="muted">
                    <b>{targetName}</b> is a strict pack install ({targetManaged}); extra mods would break the server match, so installing into it is disabled. Pick another installation above.
                  </p>
                )}
                <p className="muted">Installed in {targetName}: {installed.length} mod{installed.length === 1 ? "" : "s"}.</p>
              </div>
            </>
          )}

          {/* ---------------- WORLDS ---------------- */}
          {view === "worlds" && (
            <>
              <div className="topbar">
                <div><div className="eyebrow">Saves &amp; seeds</div><h1 className="title">Worlds</h1></div>
                <span className="grow" />
                <button className="btn" disabled={worldsBusy || !target} onClick={() => refreshWorlds(target)}>
                  {worldsBusy ? "Reading…" : "Refresh"}
                </button>
                <select value={target} onChange={(e) => setTarget(e.target.value)}>
                  {installs.map((i) => (<option key={i.path} value={i.path}>{i.meta.name}</option>))}
                </select>
              </div>
              <div className="view">
                {worlds.length === 0 ? (
                  <p className="muted">
                    {worldsBusy ? "Reading save files…" : `No worlds in ${targetName || "this installation"} yet. New worlds appear here once you create them in-game.`}
                  </p>
                ) : (
                  <>
                    <p className="muted" style={{ marginBottom: 12 }}>
                      {worlds.length} world{worlds.length === 1 ? "" : "s"} in {targetName} · newest first
                    </p>
                    <div className="worlds">
                      {worlds.map((w) => {
                        const saveDir = w.path.slice(0, w.path.length - w.filename.length);
                        return (
                          <div className="world" key={w.path}>
                            <div className="world-main">
                              <div className="world-name">{w.name}</div>
                              <div className="world-stats">
                                {w.seed != null && <span className="wstat"><b>Seed</b><span className="tab">{w.seed}</span></span>}
                                {w.playstyle && <span className="wstat"><b>Style</b>{playstyleLabel(w.playstyle)}</span>}
                                {w.world_height != null && <span className="wstat"><b>Height</b><span className="tab">{w.world_height}</span></span>}
                                {w.created_version && <span className="wstat"><b>Made on</b><span className="tab">{w.created_version}</span></span>}
                                {w.last_version && w.last_version !== w.created_version && (
                                  <span className="wstat"><b>Last on</b><span className="tab">{w.last_version}</span></span>
                                )}
                                <span className="wstat"><b>Size</b><span className="tab">{fmtBytes(w.size_bytes)}</span></span>
                                <span className="wstat"><b>Modified</b>{fmtLastPlayed(w.modified_ms)}</span>
                              </div>
                            </div>
                            <div className="world-acts">
                              <button className="mini" disabled={busy} onClick={() => backupWorld(w)} title="Copy this world into the installation's backups folder">Back up</button>
                              <button className="mini" onClick={() => openFolder(saveDir)} title="Open the Saves folder">Folder</button>
                              <button className="mini danger-mini" disabled={busy} onClick={() => setConfirmDeleteWorld(w)} title="Permanently delete this world">Delete</button>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                    <p className="muted" style={{ marginTop: 14 }}>
                      Back up copies the world into <span className="tab">.translocator-backups\worlds</span> inside the installation. Deleting a world is permanent. Back it up first if unsure.
                    </p>
                  </>
                )}
              </div>
            </>
          )}

          {/* ---------------- SERVERS ---------------- */}
          {view === "servers" && (
            <>
              <div className="topbar">
                <div><div className="eyebrow">Multiplayer</div><h1 className="title">Servers</h1></div>
                <span className="grow" />
                {serverTab === "public" && (
                  <button className="btn" disabled={serversBusy} onClick={() => loadPublicServers(true)}>{serversBusy ? "Loading…" : "Refresh"}</button>
                )}
              </div>
              <div className="subtabs">
                <button className={"subtab" + (serverTab === "public" ? " on" : "")} onClick={() => setServerTab("public")}>Public</button>
                <button className={"subtab" + (serverTab === "private" ? " on" : "")} onClick={() => setServerTab("private")}>Private</button>
              </div>
              <div className="view">
                {serverTab === "public" && (
                  <>
                    {serversError && (
                      <div className="empty">
                        <Gear size={40} />
                        <h3>Could not load the server list</h3>
                        <p>{serversError}</p>
                        <button className="btn" disabled={serversBusy} onClick={() => loadPublicServers(true)}>Try again</button>
                      </div>
                    )}
                    {!serversError && serversBusy && publicServers.length === 0 && <p className="muted">Loading the public server list…</p>}
                    {!serversError && publicServers.length > 0 && (
                      <>
                        <div className="srv-toolbar">
                          <input className="filterbox" placeholder="Search name, address, description…" value={serverSearch} onChange={(e) => setServerSearch(e.target.value)} />
                          <select value={serverVersionFilter} onChange={(e) => setServerVersionFilter(e.target.value)}>
                            <option value="">All versions</option>
                            {serverVersions.map((v) => (<option key={v} value={v}>{v}</option>))}
                          </select>
                          <select value={serverSort} onChange={(e) => setServerSort(e.target.value as "players" | "name")}>
                            <option value="players">Most players</option>
                            <option value="name">Name (A-Z)</option>
                          </select>
                          <span className="grow" />
                          <span className="counts">{filteredServers.length} server{filteredServers.length === 1 ? "" : "s"}</span>
                        </div>
                        <div className="srv-list">
                          {shownServers.map((s) => (
                            <div className="srv" key={s.address}>
                              <div className="srv-main">
                                <div className="srv-name">{s.name || s.address}</div>
                                <div className="srv-addr tab">{s.address}</div>
                                {s.description && <div className="srv-desc">{stripHtml(s.description)}</div>}
                                <div className="srv-badges">
                                  <span className="sbadge ver">VS {s.game_version || "?"}</span>
                                  <span className="sbadge"><b>{s.players}/{s.max_players || "?"}</b> online</span>
                                  {s.mod_count > 0 && <span className="sbadge">{s.mod_count} mods</span>}
                                  {s.playstyle && <span className="sbadge">{playstyleLabel(s.playstyle)}</span>}
                                  {s.whitelisted && <span className="sbadge warn">whitelisted</span>}
                                  {s.has_password && <span className="sbadge warn">password</span>}
                                </div>
                              </div>
                              <div className="srv-acts">
                                <button className="cta" disabled={busy} onClick={() => openJoin(s)}>Join</button>
                              </div>
                            </div>
                          ))}
                        </div>
                        {filteredServers.length > shownServers.length && (
                          <p className="muted" style={{ marginTop: 12 }}>
                            Showing the top {shownServers.length} of {filteredServers.length}. Narrow with search or the version filter.
                          </p>
                        )}
                        {filteredServers.length === 0 && <p className="muted">No servers match your filters.</p>}
                      </>
                    )}
                  </>
                )}
                {serverTab === "private" && (
                  <>
                    <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
                      <span className="muted">
                        {privateServers.length ? `${privateServers.length} saved server${privateServers.length === 1 ? "" : "s"}` : "No saved servers yet."}
                      </span>
                      <span className="grow" />
                      <button className="cta" onClick={openAddPrivate}>+ Add server</button>
                    </div>
                    {privateServers.length === 0 ? (
                      <div className="empty">
                        <Gear size={40} />
                        <h3>Save a server to join in one click</h3>
                        <p>Add a private, whitelisted, or unlisted server by address and pin it to an installation. It won't appear in the public list.</p>
                      </div>
                    ) : (
                      <div className="srv-list">
                        {privateServers.map((s) => {
                          const inst = installs.find((i) => i.path === s.install_path);
                          return (
                            <div className="srv" key={s.id}>
                              <div className="srv-main">
                                <div className="srv-name">{s.name || s.address}</div>
                                <div className="srv-addr tab">{s.address}</div>
                                <div className="srv-badges">
                                  {inst ? <span className="sbadge ver">{inst.meta.name}</span> : <span className="sbadge warn">no installation set</span>}
                                  {s.password && <span className="sbadge">password saved</span>}
                                </div>
                              </div>
                              <div className="srv-acts">
                                <button
                                  className="cta"
                                  disabled={!account || busy || !s.install_path}
                                  title={!account ? "Sign in first" : !s.install_path ? "Edit this server to set an installation" : undefined}
                                  onClick={() => connectServer(s.name, s.address, s.install_path, s.password)}
                                >
                                  Join
                                </button>
                                <button className="mini" onClick={() => setPrivDraft({ ...s })}>Edit</button>
                                <button className="mini danger-mini" onClick={() => removePrivateServer(s.id)}>Remove</button>
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </>
                )}
              </div>
            </>
          )}

          {/* ---------------- MODPACKS ---------------- */}
          {view === "market" && (
            <>
              <div className="topbar">
                <div><div className="eyebrow">Published packs</div><h1 className="title">Modpack Hub</h1></div>
                <span className="grow" />
                <button className="btn" disabled={packsBusy} onClick={() => loadPacks(true)}>{packsBusy ? "Loading…" : "Refresh"}</button>
                <button className="cta" onClick={openCurator}>Publish a pack</button>
              </div>
              <div className="view">
                {packsError && (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>Could not reach the Hub</h3>
                    <p>{packsError}</p>
                    <button className="btn" disabled={packsBusy} onClick={() => loadPacks(true)}>Try again</button>
                  </div>
                )}
                {!packsError && packsBusy && packs.length === 0 && <p className="muted">Loading packs…</p>}
                {!packsError && !packsBusy && packs.length === 0 && (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>No packs published yet</h3>
                    <p>Published modpacks appear here. Curate one from an installation to be the first.</p>
                  </div>
                )}
                {packs.length > 0 && (
                  <div className="pack-grid">
                    {packs.map((p) => (
                      <button className="pack-card" key={p.id} onClick={() => openPack(p.id)}>
                        <div className="pack-ico">{p.icon || p.name[0]?.toUpperCase() || "?"}</div>
                        <div className="pack-body">
                          <div className="pack-name">{p.name}</div>
                          {p.summary && <div className="pack-sum">{p.summary}</div>}
                          <div className="pack-meta tab">
                            {p.game_version ? `VS ${p.game_version}` : "any version"}
                            {p.latest_version ? ` · v${p.latest_version}` : " · unpublished"}
                          </div>
                          {p.tags && p.tags.length > 0 && (
                            <div className="pack-tags">
                              {p.tags.slice(0, 4).map((t) => (<span className="tagchip" key={t}>{t}</span>))}
                            </div>
                          )}
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </>
          )}

          {/* ---------------- PACK PAGE ---------------- */}
          {view === "pack" && (
            <>
              <div className="topbar">
                <button className="btn" onClick={() => setView("market")}>← Modpack Hub</button>
                <div style={{ marginLeft: 4 }}>
                  <div className="eyebrow">Pack</div>
                  <h1 className="title">{packManifest?.pack.name ?? packDetail?.name ?? selectedPack}</h1>
                </div>
                <span className="grow" />
                {selectedPack && <button className="btn" disabled={packBusy} onClick={() => openPack(selectedPack)}>Refresh</button>}
                {packManifest && account?.playername === packManifest.pack.author && (
                  <button className="btn" disabled={packBusy} title="Start a new version of this pack with every published field prefilled" onClick={startPackUpdate}>New version</button>
                )}
                {packManifest && <button className="cta" disabled={packBusy || packInstalling} onClick={openInstallPack}>Install this pack</button>}
              </div>
              <div className="view">
                {packError && (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>Could not open this pack</h3>
                    <p>{packError}</p>
                  </div>
                )}
                {!packError && packBusy && !packManifest && <p className="muted">Loading pack…</p>}
                {!packError && packManifest && (
                  <>
                    <div className="pack-head">
                      <div className="pack-head-ico">{packManifest.pack.icon || packManifest.pack.name[0]?.toUpperCase() || "?"}</div>
                      <div>
                        <div className="pack-head-meta tab">
                          v{packManifest.pack.version} · VS {packManifest.pack.game_version} · by {packManifest.pack.author}
                        </div>
                        {(packManifest.pack.summary || packDetail?.summary) && (
                          <p className="pack-head-sum">{packManifest.pack.summary || packDetail?.summary}</p>
                        )}
                        {packManifest.pack.tags && packManifest.pack.tags.length > 0 && (
                          <div className="pack-tags">
                            {packManifest.pack.tags.map((t) => (<span className="tagchip" key={t}>{t}</span>))}
                          </div>
                        )}
                        {(() => {
                          const l = packManifest.links ?? packDetail?.links ?? undefined;
                          const entries = ([
                            l?.website && (["Website", l.website] as const),
                            l?.discord && (["Discord", l.discord] as const),
                            l?.source && (["Source", l.source] as const),
                            l?.donate && (["Donate", l.donate] as const),
                          ].filter(Boolean)) as (readonly [string, string])[];
                          return entries.length ? (
                            <div className="pack-links">
                              {entries.map(([label, url]) => (
                                <button className="link" key={label} onClick={() => invoke("open_url", { url })}>{label} ↗</button>
                              ))}
                            </div>
                          ) : null;
                        })()}
                      </div>
                    </div>

                    {packManifest.pack.description && <p className="pack-desc">{packManifest.pack.description}</p>}

                    {packManifest.server && (
                      <div className="pack-server">
                        <b>Server</b> <span className="tab">{packManifest.server.address}</span>
                        {packManifest.server.auto_add ? " · added to your multiplayer list on install" : ""}
                      </div>
                    )}

                    <div className="pack-mods-hd">
                      <h3>{packManifest.mods.length} mod{packManifest.mods.length === 1 ? "" : "s"}</h3>
                      {packManifest.overrides && packManifest.overrides.length > 0 && (
                        <span className="muted">· {packManifest.overrides.length} config override{packManifest.overrides.length === 1 ? "" : "s"}</span>
                      )}
                    </div>
                    <div className="list">
                      {packManifest.mods.map((m) => (
                        <div className="li" key={m.modidstr}>
                          <span>
                            <span className="nm">{m.name}</span>
                            {!m.required && <span className="meta"> optional</span>}
                          </span>
                          <span style={{ display: "flex", gap: 8, alignItems: "center" }}>
                            <span className="tab">{m.modversion}</span>
                            <button className="mini" title="Open this mod's ModDB page" onClick={() => openModPage(m.modidstr)}>ModDB ↗</button>
                            <button className="mini" title="Open the author's donation link" onClick={() => tipModAuthor(m.modidstr, m.name)}>♥ Tip the author</button>
                          </span>
                        </div>
                      ))}
                    </div>
                    {(() => {
                      const existing = installs.filter((i) => (i.meta.managed_by || "").startsWith(`${packManifest.pack.id}@`));
                      return (
                        <p className="muted" style={{ marginTop: 12 }}>
                          {existing.length > 0 && <>Installed as <b>{existing.map((i) => i.meta.name).join(", ")}</b> ({existing.map((i) => i.meta.managed_by?.split("@")[1]).join(", ")}). </>}
                          Installing creates a fresh installation frozen to v{packManifest.pack.version}: every mod is downloaded from ModDB and verified against the publisher's pins.
                          {packManifest.pack.strict ? " This is a strict pack: the launcher keeps its Mods folder exactly matched to the manifest." : ""}
                        </p>
                      );
                    })()}
                  </>
                )}
              </div>
            </>
          )}

          {/* ---------------- CURATOR (publish a pack) ---------------- */}
          {view === "curator" && (
            <>
              <div className="topbar">
                <button className="btn" onClick={() => setView("market")}>← Modpack Hub</button>
                <div style={{ marginLeft: 4 }}>
                  <div className="eyebrow">Curator</div>
                  <h1 className="title">Publish a pack</h1>
                </div>
              </div>
              <div className="view" style={{ maxWidth: 720 }}>
                {installs.length === 0 ? (
                  <div className="empty">
                    <Gear size={40} />
                    <h3>Nothing to publish yet</h3>
                    <p>A pack is curated from an installation's Mods folder. Set one up first.</p>
                    <button className="btn" onClick={() => setView("installations")}>Open Installations</button>
                  </div>
                ) : (
                  <>
                    <label className="field">
                      <span className="lab">Source installation <span className="lab-hint">(its Mods folder becomes the pack)</span></span>
                      <select value={curInstall} onChange={(e) => pickCuratorInstall(e.target.value)}>
                        {installs.map((i) => (
                          <option key={i.path} value={i.path}>{i.meta.name}{i.meta.version ? ` (${i.meta.version})` : ""} · {i.mod_count} mods</option>
                        ))}
                      </select>
                    </label>

                    <div className="cur-grid">
                      <label className="field"><span className="lab">Pack name<span className="req">*</span></span>
                        <input value={curMeta.name} placeholder="The Quire" onChange={(e) => setCurName(e.target.value)} /></label>
                      <label className="field"><span className="lab">Pack id<span className="req">*</span> <span className="lab-hint">(permanent slug)</span></span>
                        <input value={curMeta.id} placeholder="the-quire" onChange={(e) => { setCurIdEdited(true); setCurMeta({ ...curMeta, id: slugify(e.target.value) }); }} /></label>
                      <label className="field"><span className="lab">Pack version<span className="req">*</span></span>
                        <input value={curMeta.version} placeholder="1.0.0" onChange={(e) => setCurMeta({ ...curMeta, version: e.target.value })} /></label>
                      <label className="field"><span className="lab">Author</span>
                        <input value={curMeta.author} placeholder="Venah" onChange={(e) => setCurMeta({ ...curMeta, author: e.target.value })} /></label>
                      <label className="field"><span className="lab">Game version<span className="req">*</span></span>
                        <input value={curMeta.game_version} placeholder="1.21.1" onChange={(e) => setCurMeta({ ...curMeta, game_version: e.target.value })} /></label>
                      <label className="field"><span className="lab">Tags <span className="lab-hint">(comma-separated)</span></span>
                        <input value={curMeta.tags} placeholder="survival, hardcore, rp" onChange={(e) => setCurMeta({ ...curMeta, tags: e.target.value })} /></label>
                    </div>
                    <label className="field"><span className="lab">Summary <span className="lab-hint">(one line, shown on the pack card)</span></span>
                      <input value={curMeta.summary} onChange={(e) => setCurMeta({ ...curMeta, summary: e.target.value })} /></label>
                    <label className="field"><span className="lab">Description</span>
                      <textarea rows={4} value={curMeta.description} onChange={(e) => setCurMeta({ ...curMeta, description: e.target.value })} /></label>

                    <div className="cur-grid">
                      <label className="field"><span className="lab">Server address <span className="lab-hint">(optional)</span></span>
                        <input value={curServer.address} placeholder="play.example.com:42420" onChange={(e) => setCurServer({ ...curServer, address: e.target.value })} /></label>
                      <label className="field row-check" style={{ alignSelf: "end" }}>
                        <input type="checkbox" checked={curServer.auto_add} disabled={!curServer.address.trim()} onChange={(e) => setCurServer({ ...curServer, auto_add: e.target.checked })} />
                        <span>Add to the in-game server list on install</span>
                      </label>
                      <label className="field row-check">
                        <input type="checkbox" checked={curMeta.strict} onChange={(e) => setCurMeta({ ...curMeta, strict: e.target.checked })} />
                        <span>Self-contained pack <span className="muted">(the pack defines the whole Mods folder; no user-added mods)</span></span>
                      </label>
                      <label className="field"><span className="lab">Website</span>
                        <input value={curLinks.website} onChange={(e) => setCurLinks({ ...curLinks, website: e.target.value })} /></label>
                      <label className="field"><span className="lab">Discord</span>
                        <input value={curLinks.discord} onChange={(e) => setCurLinks({ ...curLinks, discord: e.target.value })} /></label>
                      <label className="field"><span className="lab">Source <span className="lab-hint">(repo, if public)</span></span>
                        <input value={curLinks.source} onChange={(e) => setCurLinks({ ...curLinks, source: e.target.value })} /></label>
                      <label className="field"><span className="lab">Donate <span className="lab-hint">(tips go straight to you)</span></span>
                        <input value={curLinks.donate} onChange={(e) => setCurLinks({ ...curLinks, donate: e.target.value })} /></label>
                    </div>

                    {curConfigs.length > 0 && (
                      <div className="field">
                        <span className="lab">Config overrides <span className="lab-hint">(shipped with the pack, applied on install)</span></span>
                        <div className="cur-configs">
                          {curConfigs.map((c) => (
                            <label className="row-check" key={c}>
                              <input type="checkbox" checked={curSelected.has(c)} onChange={(e) => {
                                const n = new Set(curSelected);
                                if (e.target.checked) n.add(c); else n.delete(c);
                                setCurSelected(n);
                              }} />
                              <span className="tab" style={{ fontSize: 12 }}>{c}</span>
                            </label>
                          ))}
                        </div>
                      </div>
                    )}

                    <div style={{ display: "flex", gap: 8, alignItems: "center", margin: "6px 0 14px" }}>
                      <button
                        className="cta"
                        disabled={curBusy || !curInstall || !curMeta.id.trim() || !curMeta.name.trim() || !curMeta.version.trim() || !curMeta.game_version.trim()}
                        onClick={buildManifest}
                      >
                        {curBusy ? "Resolving against ModDB…" : curPreview ? "Rebuild manifest" : "Build manifest"}
                      </button>
                      {!curMeta.id.trim() && <span className="muted">Name, id, version, and game version are required.</span>}
                    </div>
                    {curBusy && (
                      <div className="checking" style={{ padding: "0 0 10px" }}>
                        <div className="prog-n tab">Hashing mods and matching each one to a ModDB release…</div>
                        <div className="prog"><i className="indet" style={{ width: "40%" }} /></div>
                      </div>
                    )}

                    {curPreview && (
                      <>
                        {curPreview.unresolved.length > 0 && (
                          <div className="warn-note" style={{ marginBottom: 12 }}>
                            <b>{curPreview.unresolved.length} mod{curPreview.unresolved.length === 1 ? "" : "s"} can't be pinned.</b> Packs are ModDB-only, so these stay out of the manifest until fixed:
                            <ul className="cur-unres">
                              {curPreview.unresolved.map((u) => (
                                <li key={u.filename}><span className="tab">{u.modid}@{u.version}</span>: {u.reason}</li>
                              ))}
                            </ul>
                          </div>
                        )}
                        <div className="pack-mods-hd">
                          <h3>{curPreview.resolved_count} mod{curPreview.resolved_count === 1 ? "" : "s"} pinned</h3>
                          {curSelected.size > 0 && <span className="muted">· {curSelected.size} config override{curSelected.size === 1 ? "" : "s"}</span>}
                        </div>
                        <div className="list" style={{ marginBottom: 14, maxHeight: 260, overflowY: "auto" }}>
                          {curPreview.manifest.mods.map((m) => (
                            <div className="li" key={m.modidstr}>
                              <span><span className="nm">{m.name}</span> <span className="meta">{m.modidstr} · {m.side}</span></span>
                              <span style={{ display: "flex", gap: 10, alignItems: "baseline" }}>
                                <span className="tab">{m.modversion}</span>
                                {m.sha256 && <span className="meta tab" title={m.sha256}>{m.sha256.slice(0, 10)}…</span>}
                              </span>
                            </div>
                          ))}
                        </div>

                        <div className="field">
                          <span className="lab">Publishing as</span>
                          {!pubStatus?.signed_in ? (
                            <p className="muted" style={{ margin: 0 }}>
                              Not signed in. Publishing is tied to your Vintage Story account; sign in on the Account screen first.
                            </p>
                          ) : (
                            <p className="muted" style={{ display: "flex", gap: 10, alignItems: "center", margin: 0, flexWrap: "wrap" }}>
                              <span className="dotv" style={{ background: "var(--ok)" }} />
                              <b style={{ color: "var(--fg)" }}>{pubStatus.playername}</b>
                              {pubStatus.has_key && pubStatus.fingerprint && (
                                <span className="tab" title="This machine's signing key">{pubStatus.fingerprint.slice(0, 18)}…</span>
                              )}
                              <button className="link" disabled={registering} onClick={registerDevice}
                                title="Bind this machine's signing key to your VS account on the Hub (needed once per machine)">
                                {registering ? "Registering…" : pubStatus.has_key ? "Re-register this device" : "Register this device"}
                              </button>
                            </p>
                          )}
                        </div>
                        <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
                          <button className="cta" disabled={pubBusy || !pubStatus?.signed_in || curPreview.resolved_count === 0} onClick={publishManifest}>
                            {pubBusy ? "Publishing…" : `Publish v${curPreview.manifest.pack.version} to the Hub`}
                          </button>
                          <span className="muted">Signed with this device's key as your VS account. Players update only when you publish a new version.</span>
                        </div>
                      </>
                    )}
                  </>
                )}
              </div>
            </>
          )}

          {/* ---------------- ACCOUNT ---------------- */}
          {view === "account" && (
            <>
              <div className="topbar"><div><div className="eyebrow">Vintage Story</div><h1 className="title">Account</h1></div></div>
              <div className="view" style={{ maxWidth: 420 }}>
                <p>Signed in as <b>{account.playername}</b>.</p>
                <p className="muted">Your session is saved and carried into every installation at launch. No re-login.</p>
                <button className="btn" onClick={doLogout}>Log out</button>
              </div>
            </>
          )}

          {/* ---------------- SETTINGS ---------------- */}
          {view === "settings" && (
            <>
              <div className="topbar"><div><div className="eyebrow">Configuration</div><h1 className="title">Settings</h1></div></div>
              <div className="view" style={{ maxWidth: 640 }}>
                <label className="field">
                  <span className="lab">Game executable <span className="lab-hint">(fallback; pinned installs launch from downloaded versions)</span></span>
                  <div className="path-row">
                    <input value={gameExe} onChange={(e) => setGameExe(e.target.value)} placeholder="Vintagestory.exe" />
                    <button className="btn" type="button" onClick={browseGameExe}>Browse…</button>
                  </div>
                </label>
                <label className="field">
                  <span className="lab">Installations folder</span>
                  <div className="path-row">
                    <input value={installationsDir} onChange={(e) => setInstallationsDir(e.target.value)} onBlur={() => refreshInstalls(installationsDir)} placeholder="Where your installations live" />
                    <button className="btn" type="button" onClick={browseInstallsDir}>Browse…</button>
                    <button className="btn" type="button" disabled={busy} onClick={scanLaunchers} title="Look for VS Launcher / StoryForge installations to import">Import from launcher…</button>
                  </div>
                </label>
                <label className="field"><span className="lab">Default game version (for newly imported installs)</span><input style={{ width: 120 }} value={gameVersion} onChange={(e) => setGameVersion(e.target.value)} /></label>

                <div className="field">
                  <span className="lab">Downloaded game versions (shared across installs)</span>
                  {cachedVersions.length === 0 ? (
                    <p className="muted">None yet. Versions download on demand when an installation needs one.</p>
                  ) : (
                    <div className="list">
                      {cachedVersions.map((v) => {
                        const used = installs.filter((i) => i.meta.version === v).length;
                        return (
                          <div className="li" key={v}>
                            <span><span className="nm">{v}</span> <span className="meta">{used ? `used by ${used} install${used === 1 ? "" : "s"}` : "not used by any install"}</span></span>
                            <button className="mini" disabled={used > 0} title={used > 0 ? "Still used by an installation" : "Remove downloaded binaries"} onClick={() => removeVersion(v)}>Remove</button>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>

                <div className="field">
                  <span className="lab">Optimized client <span className="lab-hint">(Optimum by Zaldaryon, built on this machine)</span></span>
                  {!optimum ? (
                    <p className="muted">Checking…</p>
                  ) : (
                    <>
                      <label className="field row-check">
                        <input type="checkbox" checked={optimum.use_optimum} onChange={(e) => toggleOptimum(e.target.checked)} />
                        <span>Launch the optimized client when a package is ready for the installation's version</span>
                      </label>
                      <p className="muted">
                        Optimum decompiles your own copy of the game, applies performance patches, and recompiles it locally.
                        Nothing is downloaded from us and nothing leaves your machine. Installations always fall back to the
                        vanilla client if anything goes wrong.
                      </p>
                      <div className="list">
                        <div className="li">
                          <span><span className="nm">Prerequisites</span> <span className="meta">
                            {[
                              `${optimum.prereqs.dotnet ? "✓" : "✗"} .NET 10 SDK`,
                              `${optimum.prereqs.git ? "✓" : "✗"} Git`,
                              `${optimum.prereqs.ilspycmd ? "✓" : "✗"} ilspycmd`,
                            ].join("   ")}
                          </span></span>
                          {!(optimum.prereqs.dotnet && optimum.prereqs.git && optimum.prereqs.ilspycmd) && (
                            <button className="mini" disabled={busy} onClick={installToolchain} title="Installs into Translocator's own folder. No admin rights, no changes to your system PATH.">Install toolchain</button>
                          )}
                        </div>
                        <div className="li">
                          <span><span className="nm">End-user notice</span> <span className="meta">
                            {optimum.eula_accepted ? `accepted (${optimum.eula_release})` : "not accepted yet"}
                          </span></span>
                          <button className="mini" disabled={eulaBusy || !gameVersion} onClick={showEula}>
                            {optimum.eula_accepted ? "Read again" : "Read and accept"}
                          </button>
                        </div>
                        <div className="li">
                          <span><span className="nm">{gameVersion || "No version selected"}</span> <span className="meta">
                            {optimum.package_state === "ready" && "optimized package ready"}
                            {optimum.package_state === "building" && "building…"}
                            {optimum.package_state === "none" && "not built yet"}
                            {optimum.package_state === "failed" && "last build failed"}
                            {optimum.package_state === "unsupported" && "no matching Optimum release yet"}
                          </span></span>
                          {optimum.package_state !== "ready" && optimum.package_state !== "building" && (
                            <button
                              className="mini"
                              disabled={busy || !gameVersion || !optimum.eula_accepted || !(optimum.prereqs.dotnet && optimum.prereqs.git && optimum.prereqs.ilspycmd)}
                              title={!optimum.eula_accepted ? "Read and accept Optimum's notice first" : "Takes several minutes and needs a network connection"}
                              onClick={optimizeNow}
                            >
                              {optimum.package_state === "failed" ? "Retry" : "Optimize now"}
                            </button>
                          )}
                        </div>
                      </div>
                      {optimum.detail && <p className="muted">{optimum.detail}</p>}
                      {optimumPhase && <pre className="logbox">{optimumPhase}</pre>}
                    </>
                  )}
                </div>

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

      {/* Optimum end-user notice. The silent installer skips its own dialog, so
          this is the only place consent can be given, and no build runs without it. */}
      {eula && (
        <div className="overlay" onClick={() => setEula(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>Optimum {eula.release} end-user notice</h3>
            <pre className="logbox" style={{ maxHeight: 320, whiteSpace: "pre-wrap" }}>{eula.text}</pre>
            <label className="field row-check">
              <input type="checkbox" checked={eulaAgreed} onChange={(e) => setEulaAgreed(e.target.checked)} />
              <span>I have read and agree to these terms</span>
            </label>
            <div className="acts">
              <button className="btn" onClick={() => setEula(null)}>Cancel</button>
              <button className="cta" disabled={!eulaAgreed} onClick={acceptEula}>Accept and continue</button>
            </div>
          </div>
        </div>
      )}

      {/* confirm restore modal */}
      {confirmRestore && (
        <div className="overlay" onClick={() => setConfirmRestore(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Restore this snapshot?</h3>
            <p>
              {confirmRestore.kind === "full" ? (
                <>The whole installation (worlds, config, mods) will be restored to the snapshot from <b>{confirmRestore.created}</b> ({fmtBytes(confirmRestore.size)}).</>
              ) : (
                <>Your Mods folder will be replaced with the backup from <b>{confirmRestore.created}</b> ({confirmRestore.mod_count} mod{confirmRestore.mod_count === 1 ? "" : "s"}).</>
              )}{" "}
              A fresh backup is taken first, so this can be undone.
            </p>
            <div className="acts">
              <button className="btn" onClick={() => setConfirmRestore(null)}>Cancel</button>
              <button className="danger" onClick={() => doRestore(confirmRestore)}>Restore snapshot</button>
            </div>
          </div>
        </div>
      )}

      {/* confirm delete world modal */}
      {confirmDeleteWorld && (
        <div className="overlay" onClick={() => setConfirmDeleteWorld(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Delete this world?</h3>
            <p>
              <b>{confirmDeleteWorld.name}</b> ({fmtBytes(confirmDeleteWorld.size_bytes)}) will be permanently deleted from {targetName}. This cannot be undone.
              {confirmDeleteWorld.seed != null && <> Its seed is <span className="tab">{confirmDeleteWorld.seed}</span>.</>}
            </p>
            <div className="acts">
              <button className="btn" onClick={() => setConfirmDeleteWorld(null)}>Cancel</button>
              <button className="btn" disabled={busy} onClick={() => { const w = confirmDeleteWorld; setConfirmDeleteWorld(null); backupWorld(w); }}>Back up first</button>
              <button className="danger" disabled={busy} onClick={() => deleteWorld(confirmDeleteWorld)}>Delete world</button>
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
              <select value={draft.version} onChange={(e) => setDraft({ ...draft, version: e.target.value })}>
                {draft.version && !availableVersions.some((v) => v.version === draft.version) && (
                  <option value={draft.version}>{draft.version} (current)</option>
                )}
                {availableVersions.map((v) => (
                  <option key={v.version} value={v.version}>
                    {v.version}{v.cached ? "  ✓ downloaded" : `  ·  ${v.filesize}`}
                  </option>
                ))}
              </select>
            </label>
            {draft.version !== editing.meta.version && !availableVersions.find((v) => v.version === draft.version)?.cached && (
              <div className="warn-note" style={{ marginTop: -4 }}>
                Changing to {draft.version} downloads it (your saves and mods stay). VS's installer will ask to uninstall your existing base game install from the Anego Studios installer, click <b>No</b> so this process does not affect your base game install.
              </div>
            )}
            <label className="field"><span className="lab">Start parameters</span>
              <input value={draft.start_params} placeholder="e.g. --openWorld ..." onChange={(e) => setDraft({ ...draft, start_params: e.target.value })} /></label>
            <label className="field"><span className="lab">Environment variables</span>
              <input value={draft.env_vars} placeholder="KEY=value, KEY2=value2" onChange={(e) => setDraft({ ...draft, env_vars: e.target.value })} /></label>
            <label className="field row-check">
              <input type="checkbox" checked={draft.auto_backup} onChange={(e) => setDraft({ ...draft, auto_backup: e.target.checked })} />
              <span>Back up the whole installation before playing <span className="muted">(can be slow for large worlds)</span></span>
            </label>
            <div style={{ display: "flex", gap: 18, flexWrap: "wrap" }}>
              <label className="field" style={{ flex: "0 0 auto" }}><span className="lab">Backup compression (0-9)</span>
                <input type="number" min={0} max={9} style={{ width: 80 }} value={draft.compression}
                  onChange={(e) => setDraft({ ...draft, compression: Math.max(0, Math.min(9, Number(e.target.value) || 0)) })} /></label>
              <label className="field" style={{ flex: "0 0 auto" }}><span className="lab">Keep last N backups</span>
                <input type="number" min={1} max={20} style={{ width: 80 }} value={draft.backups_limit}
                  onChange={(e) => setDraft({ ...draft, backups_limit: Math.max(1, Math.min(20, Number(e.target.value) || 1)) })} /></label>
            </div>
            {versionProgress && (
              <div className="checking" style={{ padding: "6px 0 10px" }}>
                <div className="prog-n tab">
                  {versionProgress.phase === "install" ? `Installing game ${versionProgress.version}. Click No if it asks to uninstall your existing game` : `Downloading game ${versionProgress.version}…`}
                </div>
                <div className="prog"><i className={versionProgress.pct < 0 ? "indet" : ""} style={versionProgress.pct >= 0 ? { width: `${versionProgress.pct}%` } : { width: "40%" }} /></div>
              </div>
            )}
            <div className="acts" style={{ justifyContent: "space-between" }}>
              <button className="danger" onClick={() => { setConfirmDelete(editing); setEditing(null); }}>Delete…</button>
              <span style={{ display: "flex", gap: 8 }}>
                <button className="btn" onClick={() => setEditing(null)}>Cancel</button>
                <button className="cta" disabled={!!versionProgress} onClick={async () => {
                  if (draft.version && draft.version !== editing.meta.version) {
                    const ok = await ensureVersion(draft.version);
                    if (!ok) return;
                  }
                  await saveInstallation(editing.path, draft);
                  setEditing(null);
                  toast(`Saved ${draft.name}`);
                }}>Save</button>
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
              <b>{confirmDelete.meta.name}</b> and its entire folder, including mods, saves, and config
              ({confirmDelete.mod_count} mod{confirmDelete.mod_count === 1 ? "" : "s"}), will be permanently deleted.
              This cannot be undone.
            </p>
            <div className="acts">
              <button className="btn" onClick={() => setConfirmDelete(null)}>Cancel</button>
              <button className="danger" onClick={() => deleteInstallation(confirmDelete)}>Delete permanently</button>
            </div>
          </div>
        </div>
      )}

      {/* create installation modal */}
      {creating && (
        <div className="overlay" onClick={() => !versionProgress && setCreating(false)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>New installation</h3>
            <label className="field"><span className="lab">Name</span>
              <input autoFocus value={createName} placeholder="e.g. Test World" onChange={(e) => setCreateName(e.target.value)} /></label>
            <label className="field"><span className="lab">Game version</span>
              <select value={createVersion} onChange={(e) => setCreateVersion(e.target.value)}>
                {availableVersions.length === 0 && <option value="">Loading versions…</option>}
                {availableVersions.map((v) => (
                  <option key={v.version} value={v.version}>
                    {v.version}{v.cached ? "  ✓ downloaded" : `  ·  ${v.filesize}`}
                  </option>
                ))}
              </select>
            </label>
            {createVersion && !availableVersions.find((v) => v.version === createVersion)?.cached && (
              <>
                <p className="muted" style={{ margin: "-6px 0 8px" }}>
                  {createVersion} will be downloaded once ({availableVersions.find((v) => v.version === createVersion)?.filesize}) and shared with any other installation on this version.
                </p>
                <div className="warn-note">
                  <b>Heads up:</b> Vintage Story's installer will ask <i>"An old version was detected. Uninstall it first?"</i>,
                  click <b>No</b>. Your existing Anego Studios game installation stays untouched; the new version installs alongside it.
                </div>
              </>
            )}
            <label className="field row-check">
              <input type="checkbox" checked={seedSettings} onChange={(e) => { setSeedSettings(e.target.checked); localStorage.setItem("tl-seed-settings", e.target.checked ? "1" : "0"); }} />
              <span>Copy my game settings (keybinds, graphics) from <b>{seedSource().label}</b></span>
            </label>
            {versionProgress && (
              <div className="checking" style={{ padding: "6px 0 10px" }}>
                <div className="prog-n tab">
                  {versionProgress.phase === "install" ? `Installing game ${versionProgress.version} (this can take a minute). Click No if Windows asks to uninstall your existing Anego Studios base game` : `Downloading game ${versionProgress.version}…`}
                </div>
                <div className="prog"><i className={versionProgress.pct < 0 ? "indet" : ""} style={versionProgress.pct >= 0 ? { width: `${versionProgress.pct}%` } : { width: "40%" }} /></div>
              </div>
            )}
            <div className="acts">
              <button className="btn" disabled={!!versionProgress} onClick={() => setCreating(false)}>Cancel</button>
              <button className="cta" disabled={!createName.trim() || !createVersion || !!versionProgress} onClick={doCreate}>Create</button>
            </div>
          </div>
        </div>
      )}

      {/* first-launch Optimum card */}
      {!optimumOnboarded && (
        <div className="overlay">
          <div className="modal wide">
            <h3>A faster Vintage Story, built on your machine</h3>
            <p className="muted" style={{ marginTop: -4 }}>
              Translocator can use <b>Optimum</b>, a community project by Zaldaryon, to rebuild the game
              with performance patches. The optimized client is built locally from your own game files,
              one build per game version, and your installations then launch through it automatically,
              with an automatic fall back to the standard client if a build ever misbehaves.
            </p>
            <p className="muted">
              You'll review Optimum's own notice before its first build, and the build tools it needs are
              set up automatically at that point, privately inside Translocator's folder (a one-time
              download of roughly 250 MB), no admin rights, nothing system-wide. You can change this
              choice anytime in Settings.
            </p>
            <div className="acts">
              <button className="btn" onClick={() => chooseOptimumDefault(false)}>Keep the standard client</button>
              <button className="cta" onClick={() => chooseOptimumDefault(true)}>Use the optimized client (Recommended)</button>
            </div>
          </div>
        </div>
      )}

      {/* install-pack modal */}
      {packInstallOpen && packManifest && (() => {
        const clientMods = packManifest.mods.filter((m) => m.side !== "server");
        const optionals = clientMods.filter((m) => !m.required);
        const requiredCount = clientMods.length - optionals.length;
        const chosen = requiredCount + optionals.filter((m) => packOptionals[m.modidstr]).length;
        const gv = packManifest.pack.game_version;
        const gvCached = cachedVersions.includes(gv) || availableVersions.find((v) => v.version === gv)?.cached;
        return (
          <div className="overlay" onClick={() => !packInstalling && setPackInstallOpen(false)}>
            <div className="modal wide" onClick={(e) => e.stopPropagation()}>
              <h3>Install {packManifest.pack.name}</h3>
              <p className="muted" style={{ marginTop: -4 }}>
                v{packManifest.pack.version} · VS {gv} · {chosen} mod{chosen === 1 ? "" : "s"} will install
                {packManifest.server ? <> · server <span className="tab">{packManifest.server.address}</span></> : null}
              </p>
              <label className="field"><span className="lab">Installation name</span>
                <input autoFocus value={packInstallName} onChange={(e) => setPackInstallName(e.target.value)} /></label>
              <label className="field"><span className="lab">Copy game settings from</span>
                <select value={packSeed} onChange={(e) => setPackSeed(e.target.value)}>
                  <option value="">Nothing (fresh settings)</option>
                  <option value="__base__">Base Vintage Story install</option>
                  {installs.map((i) => (<option key={i.path} value={i.path}>{i.meta.name}</option>))}
                </select>
              </label>
              {optionals.length > 0 && (
                <div className="field">
                  <span className="lab">Optional mods</span>
                  {optionals.map((m) => (
                    <label className="row-check" key={m.modidstr}>
                      <input
                        type="checkbox"
                        checked={!!packOptionals[m.modidstr]}
                        onChange={(e) => setPackOptionals((o) => ({ ...o, [m.modidstr]: e.target.checked }))}
                      />
                      <span>{m.name} <span className="meta">{m.modversion}</span></span>
                    </label>
                  ))}
                </div>
              )}
              {!gvCached && (
                <div className="warn-note">
                  <b>Heads up:</b> VS {gv} downloads first (shared with other installations on this version). Its installer may ask
                  <i> "An old version was detected. Uninstall it first?"</i>, click <b>No</b>.
                </div>
              )}
              {versionProgress && (
                <div className="checking" style={{ padding: "6px 0 10px" }}>
                  <div className="prog-n tab">
                    {versionProgress.phase === "install" ? `Installing game ${versionProgress.version}…` : `Downloading game ${versionProgress.version}…`}
                  </div>
                  <div className="prog"><i className={versionProgress.pct < 0 ? "indet" : ""} style={versionProgress.pct >= 0 ? { width: `${versionProgress.pct}%` } : { width: "40%" }} /></div>
                </div>
              )}
              {packProgress && packProgress.phase === "download" && (
                <div className="checking" style={{ padding: "6px 0 10px" }}>
                  <div className="prog-n tab">
                    Verifying and staging {packProgress.detail} ({Math.min(packProgress.done + 1, packProgress.total)} / {packProgress.total})
                  </div>
                  <div className="prog"><i style={{ width: `${packProgress.total ? ((packProgress.done + (packProgress.bytesTotal > 0 ? Math.min(1, packProgress.received / packProgress.bytesTotal) : 0)) / packProgress.total) * 100 : 0}%` }} /></div>
                </div>
              )}
              {packProgress && packProgress.phase === "place" && (
                <div className="checking" style={{ padding: "6px 0 10px" }}>
                  <div className="prog-n tab">All mods verified. Placing files and applying config defaults…</div>
                  <div className="prog"><i className="indet" style={{ width: "40%" }} /></div>
                </div>
              )}
              <p className="muted" style={{ fontSize: 12 }}>
                Nothing lands in the installation until every mod has downloaded and passed verification.
                Mod versions are then managed by the pack publisher; updates arrive as whole-pack updates.
              </p>
              <div className="acts">
                <button className="btn" disabled={packInstalling} onClick={() => setPackInstallOpen(false)}>Cancel</button>
                <button className="cta" disabled={packInstalling || !packInstallName.trim()} onClick={doInstallPack}>
                  {packInstalling ? "Installing…" : "Install"}
                </button>
              </div>
            </div>
          </div>
        );
      })()}

      {/* join server modal */}
      {joinServer && (() => {
        const im = installs.find((i) => i.path === joinInstall)?.meta;
        const mismatch = !!(im?.version && joinServer.game_version && im.version !== joinServer.game_version);
        return (
          <div className="overlay" onClick={() => setJoinServer(null)}>
            <div className="modal" onClick={(e) => e.stopPropagation()}>
              <h3>Join {joinServer.name || joinServer.address}</h3>
              <p className="muted" style={{ marginTop: -4 }}>
                <span className="tab">{joinServer.address}</span> · VS {joinServer.game_version || "?"} · {joinServer.players}/{joinServer.max_players || "?"} online
                {joinServer.mod_count > 0 ? ` · ${joinServer.mod_count} mods` : ""}
              </p>
              {installs.length === 0 ? (
                <div className="warn-note">You need an installation to join with. Create one on the Installations screen first.</div>
              ) : (
                <>
                  <label className="field">
                    <span className="lab">Join with installation <span className="lab-hint">(its mods must match the server)</span></span>
                    <select value={joinInstall} onChange={(e) => setJoinInstall(e.target.value)}>
                      {installs.map((i) => (
                        <option key={i.path} value={i.path}>{i.meta.name}{i.meta.version ? ` (${i.meta.version})` : ""}</option>
                      ))}
                    </select>
                  </label>
                  {mismatch && (
                    <div className="warn-note" style={{ marginTop: -4 }}>
                      This installation is on {im?.version}, the server is on {joinServer.game_version}. They may refuse to connect.
                    </div>
                  )}
                  {joinServer.has_password && (
                    <label className="field">
                      <span className="lab">Server password</span>
                      <input type="password" autoComplete="off" value={joinPassword} onChange={(e) => setJoinPassword(e.target.value)} />
                    </label>
                  )}
                  {!account && <div className="warn-note">Sign in on the Account screen to connect and play.</div>}
                </>
              )}
              <div className="acts" style={{ justifyContent: "space-between" }}>
                <button className="mini" disabled={!joinInstall} title="Add this server to the installation's in-game multiplayer list" onClick={() => addServerToInstall(joinServer.name, joinServer.address, joinInstall, joinPassword)}>Add to installation</button>
                <span style={{ display: "flex", gap: 8 }}>
                  <button className="btn" onClick={() => setJoinServer(null)}>Cancel</button>
                  <button className="cta" disabled={!account || !joinInstall || busy} onClick={() => connectServer(joinServer.name, joinServer.address, joinInstall, joinPassword)}>Connect &amp; play</button>
                </span>
              </div>
            </div>
          </div>
        );
      })()}

      {/* add / edit private server modal */}
      {privDraft && (
        <div className="overlay" onClick={() => setPrivDraft(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>{privateServers.some((s) => s.id === privDraft.id) ? "Edit server" : "Add server"}</h3>
            <label className="field"><span className="lab">Name</span>
              <input autoFocus value={privDraft.name} placeholder="My server" onChange={(e) => setPrivDraft({ ...privDraft, name: e.target.value })} /></label>
            <label className="field"><span className="lab">Address <span className="lab-hint">(host or host:port)</span></span>
              <input value={privDraft.address} placeholder="123.45.67.89:42420" onChange={(e) => setPrivDraft({ ...privDraft, address: e.target.value })} /></label>
            <label className="field"><span className="lab">Installation to join with</span>
              <select value={privDraft.install_path} onChange={(e) => setPrivDraft({ ...privDraft, install_path: e.target.value })}>
                <option value="">none yet</option>
                {installs.map((i) => (<option key={i.path} value={i.path}>{i.meta.name}{i.meta.version ? ` (${i.meta.version})` : ""}</option>))}
              </select></label>
            <label className="field"><span className="lab">Password <span className="lab-hint">(optional, stored sealed on this PC)</span></span>
              <input type="password" autoComplete="off" value={privDraft.password} onChange={(e) => setPrivDraft({ ...privDraft, password: e.target.value })} /></label>
            <div className="acts">
              <button className="btn" onClick={() => setPrivDraft(null)}>Cancel</button>
              <button className="cta" disabled={!privDraft.address.trim()} onClick={() => savePrivateServer(privDraft)}>Save server</button>
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
