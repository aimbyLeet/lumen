import { useCallback, useEffect, useState } from "react";
import { isLang, lang, setLang, words, type Lang } from "../../client/src/i18n";
import { holdToSee, Icon, LangToggle, type IconName } from "../../client/src/ui";
import logo from "../../client/src/assets/lumen.png";
import { api, Unauthorized, type State } from "./api";
import { Clients } from "./Clients";
import { type Act } from "./common";
import { Machines } from "./Machines";
import { Requests } from "./Requests";
import { Settings } from "./Settings";

const POLL_MS = 3000;

/**
 * This browser's choice of language; `?lang=` in the address (the stand-ins) comes first. Nothing chosen yet: Russian
 * for a browser whose own language is Russian, English for any other (as on the landing page, deploy/server/site).
 */
const LANG_KEY = "lumen.lang";
if (!new URLSearchParams(location.search).has("lang")) {
  const saved = localStorage.getItem(LANG_KEY);
  setLang(isLang(saved) ? saved : /^ru\b/i.test(navigator.language || "") ? "ru" : "en");
}

type Tab = "clients" | "machines" | "settings";
const TABS: Tab[] = ["clients", "machines", "settings"];

/// The open tab lives in the address (`#settings`): it survives a reload.
function tabFromHash(): Tab {
  const t = location.hash.slice(1) as Tab;
  return TABS.includes(t) ? t : "clients";
}

const W = words(
  {
    clients: "Clients",
    machines: "Machines",
    settings: "Settings",
    tooManyPasswords: "Too many wrong passwords. Wait a minute.",
    wrongLogin: "Wrong login or password.",
    noConnection: "No connection to the server.",
    admin: "Lumen · Administrator",
    pageTitle: "Lumen — administrator",
    logOut: "Log out",
    lost: "No connection to the server. The screen shows the last thing it reported.",
    hide: "Hide",
    status: (sessions: number, clientsOn: number, clients: number, hostsOn: number, hosts: number) =>
      `sessions ${sessions} · clients online ${clientsOn} of ${clients} · machines online ${hostsOn} of ${hosts}`,
    login: "Login",
    password: "Password",
    holdToSee: "Hold to see the password",
    signIn: "Sign in",
  },
  {
    clients: "Клиенты",
    machines: "Машины",
    settings: "Настройки",
    tooManyPasswords: "Слишком много неверных паролей. Подождите минуту.",
    wrongLogin: "Неверный логин или пароль.",
    noConnection: "Нет связи с сервером.",
    admin: "Lumen · Администратор",
    pageTitle: "Lumen — администратор",
    logOut: "Выйти",
    lost: "Нет связи с сервером. На экране последнее, что он сообщил.",
    hide: "Скрыть",
    status: (sessions: number, clientsOn: number, clients: number, hostsOn: number, hosts: number) =>
      `сессий ${sessions} · клиентов в сети ${clientsOn} из ${clients} · машин в сети ${hostsOn} из ${hosts}`,
    login: "Логин",
    password: "Пароль",
    holdToSee: "Удерживайте, чтобы увидеть пароль",
    signIn: "Войти",
  },
);

/** The server speaks English to programs. */
function errorWords(e: unknown): string {
  const w = W();
  const text = String((e as Error).message);
  if (text.startsWith("too many wrong passwords")) return w.tooManyPasswords;
  if (text.startsWith("wrong login")) return w.wrongLogin;
  if (text.includes("Failed to fetch")) return w.noConnection;
  return text;
}

export function App() {
  const [s, setS] = useState<State | null>(null);
  const [needLogin, setNeedLogin] = useState(false);
  const [lost, setLost] = useState(false);
  const [error, setError] = useState("");
  const [tab, setTabState] = useState<Tab>(tabFromHash);
  const setTab = (t: Tab) => {
    // Replace, not push: "Back" leaves the admin page rather than walking the tabs.
    history.replaceState(null, "", t === "clients" ? location.pathname + location.search : `#${t}`);
    setTabState(t);
  };
  const [query, setQuery] = useState("");
  // The language lives in i18n.ts; this only renders the page anew when it changes.
  const [shownLang, setShownLang] = useState<Lang>(lang());
  const w = W();

  const chooseLang = (l: Lang) => {
    setLang(l);
    localStorage.setItem(LANG_KEY, l);
    setShownLang(l);
  };
  useEffect(() => {
    document.title = W().pageTitle;
  }, [shownLang]);

  const refresh = useCallback(async () => {
    try {
      setS(await api.state());
      setNeedLogin(false);
      setLost(false);
    } catch (e) {
      if (e instanceof Unauthorized) setNeedLogin(true);
      else setLost(true);
    }
  }, []);
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  const act: Act = useCallback(
    async (f) => {
      setError("");
      try {
        await f();
      } catch (e) {
        if (e instanceof Unauthorized) setNeedLogin(true);
        else setError(errorWords(e));
      }
      await refresh();
    },
    [refresh],
  );

  if (needLogin) return <Login onDone={refresh} onLang={chooseLang} />;
  if (!s) return <div className="window" />;
  const tabs: [Tab, IconName, string][] = [
    ["clients", "person", w.clients],
    ["machines", "desktop", w.machines],
    ["settings", "settings", w.settings],
  ];
  const inSession = s.sessions.length;
  const clientsOn = s.clients.filter((c) => c.online).length;
  const hostsOn = s.hosts.filter((h) => h.online).length;
  return (
    <div className="window">
      <header className="titlebar">
        <div className="titlebar-name">
          <img src={logo} alt="" width={16} height={16} />
          <span>{w.admin}</span>
        </div>
        <div className="titlebar-user">
          <span>{s.user}</span>
          <button className="ghost-icon" title={w.logOut} aria-label={w.logOut} onClick={() => act(api.logout)}>
            <Icon name="logout" size={16} />
          </button>
        </div>
      </header>
      <nav className="tabs" role="tablist">
        {tabs.map(([id, icon, title]) => (
          <button key={id} role="tab" aria-selected={tab === id} aria-label={title} title={title} className={tab === id ? "on" : ""} onClick={() => setTab(id)}>
            <Icon name={icon} size={20} />
            {id === "clients" && s.pending > 0 && <span className="tab-badge">{s.pending}</span>}
          </button>
        ))}
        <LangToggle onLang={chooseLang} />
      </nav>
      <main>
        {(lost || error) && (
          <div className="page">
            {lost && <div className="banner bad">{w.lost}</div>}
            {error && (
              <div className="banner bad">
                <span className="selectable">{error}</span>
                <button className="ghost-icon" title={w.hide} onClick={() => setError("")}>
                  <Icon name="close" size={16} />
                </button>
              </div>
            )}
          </div>
        )}
        {tab !== "settings" && <Requests s={s} act={act} />}
        {/* All the tabs stay alive: what is open survives a look at another tab. */}
        <div className="tab-page" hidden={tab !== "clients"}>
          <Clients s={s} act={act} query={query} onQuery={setQuery} />
        </div>
        <div className="tab-page" hidden={tab !== "machines"}>
          <Machines s={s} act={act} query={query} onQuery={setQuery} />
        </div>
        <div className="tab-page" hidden={tab !== "settings"}>
          <Settings s={s} act={act} />
        </div>
      </main>
      <footer className="statusbar">
        <span>
          {w.status(inSession, clientsOn, s.clients.length, hostsOn, s.hosts.length)} · v{__VERSION__}
        </span>
      </footer>
    </div>
  );
}

function Login({ onDone, onLang }: { onDone: () => void; onLang: (l: Lang) => void }) {
  const w = W();
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // The password is seen while the eye is held.
  const [shown, setShown] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    setBusy(true);
    try {
      await api.login(user, password);
      onDone();
    } catch (err) {
      setError(errorWords(err));
    }
    setBusy(false);
  };
  return (
    <div className="login-page">
      <form className="card login" onSubmit={submit}>
        <div className="login-head">
          <img src={logo} alt="" width={32} height={32} />
          <h2>{w.admin}</h2>
          <LangToggle className="ghost-icon lang-button" onLang={onLang} />
        </div>
        <label className="field">
          <span>{w.login}</span>
          <input type="text" autoComplete="username" value={user} onChange={(e) => setUser(e.target.value)} autoFocus />
        </label>
        <label className="field">
          <span>{w.password}</span>
          <span className={`in-field${shown ? " shown" : ""}`}>
            <input type={shown ? "text" : "password"} autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
            <button type="button" className="in-field-button" tabIndex={-1} title={w.holdToSee} aria-label={w.holdToSee} {...holdToSee(setShown)}>
              <Icon name="visibility" />
            </button>
          </span>
        </label>
        <div className="dialog-error login-error">{error}</div>
        <button className="btn primary wide" type="submit" disabled={busy || !user || !password}>
          {w.signIn}
        </button>
      </form>
    </div>
  );
}
