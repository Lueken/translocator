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

// VS Launcher's default folders on this machine (editable in the UI).
const DEFAULT_INSTALLS = "C:\\Users\\31686\\AppData\\Roaming\\VSLInstallations";
const DEFAULT_GAME_EXE =
  "C:\\Users\\31686\\AppData\\Roaming\\VSLGameVersions\\1.22.3\\Vintagestory.exe";

function App() {
  const [gameExe, setGameExe] = useState(DEFAULT_GAME_EXE);
  const [installationsDir, setInstallationsDir] = useState(DEFAULT_INSTALLS);

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [prelogintoken, setPrelogintoken] = useState<string | null>(null);

  const [account, setAccount] = useState<Account | null>(null);
  const [installs, setInstalls] = useState<InstallInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<string[]>([]);

  // Mods panel
  const [target, setTarget] = useState<string>("");
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<ModSummary[]>([]);
  const [installed, setInstalled] = useState<string[]>([]);

  const say = (line: string) =>
    setLog((l) => [`${new Date().toLocaleTimeString()}  ${line}`, ...l].slice(0, 200));

  // Restore persisted account + load installs on startup.
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

  // Default the mod target to the first install once we have them.
  useEffect(() => {
    if (!target && installs.length) setTarget(installs[0].path);
  }, [installs, target]);

  useEffect(() => {
    if (target) refreshInstalled(target);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target]);

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
      const targetName = installs.find((i) => i.path === target)?.name ?? target;
      say(`Installing "${m.name}" into ${targetName} ...`);
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

  // Install missing required deps transitively (self-reliant, from modinfo.json).
  async function resolveDeps(installDir: string, filename: string, seen: Set<string>) {
    const missing = await invoke<MissingDep[]>("check_deps", { installDir, filename });
    for (const dep of missing) {
      if (seen.has(dep.modid)) continue;
      seen.add(dep.modid);
      say(`  ↳ missing dependency ${dep.modid}${dep.version ? ` (${dep.version})` : ""} — installing...`);
      try {
        const f = await invoke<string>("install_mod", { installDir, modidstr: dep.modid });
        say(`  ✓ installed dependency ${f}`);
        await resolveDeps(installDir, f, seen); // transitive
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

  const box: React.CSSProperties = {
    border: "1px solid #ccc",
    borderRadius: 8,
    padding: 16,
    marginBottom: 16,
  };
  const input: React.CSSProperties = { width: "100%", padding: 6, marginTop: 4 };

  return (
    <main style={{ maxWidth: 720, margin: "0 auto", padding: 24, fontFamily: "system-ui" }}>
      <h1 style={{ marginBottom: 4 }}>⚙ Translocator</h1>
      <p style={{ color: "#666", marginTop: 0 }}>
        Prototype — reliable carryover + ModDB install
      </p>

      <section style={box}>
        <strong>Paths</strong>
        <label style={{ display: "block", marginTop: 8 }}>
          Game executable
          <input style={input} value={gameExe} onChange={(e) => setGameExe(e.target.value)} />
        </label>
        <label style={{ display: "block", marginTop: 8 }}>
          Installations folder
          <input
            style={input}
            value={installationsDir}
            onChange={(e) => setInstallationsDir(e.target.value)}
          />
        </label>
      </section>

      <section style={box}>
        <strong>Account</strong>
        {account ? (
          <div style={{ marginTop: 8 }}>
            Logged in as <b>{account.playername}</b>{" "}
            <button onClick={doLogout}>Log out</button>
          </div>
        ) : (
          <div style={{ marginTop: 8 }}>
            <input
              style={input}
              placeholder="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
            <input
              style={input}
              type="password"
              placeholder="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            {prelogintoken && (
              <input
                style={input}
                placeholder="2FA / TOTP code"
                value={totp}
                onChange={(e) => setTotp(e.target.value)}
              />
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

      <section style={box}>
        <strong>Mods (ModDB)</strong>
        <div style={{ marginTop: 8 }}>
          <label>
            Target installation{" "}
            <select value={target} onChange={(e) => setTarget(e.target.value)}>
              {installs.map((i) => (
                <option key={i.path} value={i.path}>
                  {i.name}
                </option>
              ))}
            </select>
          </label>
        </div>
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
          <small style={{ color: "#666" }}>
            Installed in target ({installed.length}):
          </small>
          {installed.length > 0 && (
            <ul style={{ margin: "4px 0 0", paddingLeft: 18, fontSize: 12, color: "#444" }}>
              {installed.map((f) => (
                <li key={f}>{f}</li>
              ))}
            </ul>
          )}
        </div>
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
