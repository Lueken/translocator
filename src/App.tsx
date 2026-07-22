import { useState } from "react";
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

  const say = (line: string) =>
    setLog((l) => [`${new Date().toLocaleTimeString()}  ${line}`, ...l].slice(0, 200));

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
        say(`Logged in as ${res.account.playername} (uid ${res.account.uid.slice(0, 6)}...).`);
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

  async function refreshInstalls() {
    try {
      const list = await invoke<InstallInfo[]>("list_installs", { installationsDir });
      setInstalls(list);
      say(`Found ${list.length} installation(s).`);
    } catch (e) {
      say(`Error listing installs: ${e}`);
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
          say("↻ Game rotated the session — captured the new key (this is the fix).");
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
        Prototype — reliable login carryover (validate → stamp → launch → read-back)
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
            <button onClick={() => setAccount(null)}>Log out</button>
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
