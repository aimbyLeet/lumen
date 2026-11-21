import { useEffect, useState } from "react";
import { words, type Lang } from "../../client/src/i18n";
import { CardLine, Confirm, Icon, nameProblem, Pill, WelcomeHead, WelcomeHero } from "../../client/src/ui";
import { cap, fps, hoursMinutes, locale, serverField, serverName } from "../../client/src/units";
import { api, type AppState, type Settings } from "./api";
import { errorText } from "../../client/src/errors";
import { certProblem, TrustDialog, type CertProblem } from "../../client/src/TrustDialog";
import { hostStatus } from "./status";
import { useRun, type Run } from "./useRun";

/** How often the server in the form is looked at while it matters. */

const W = words(
  {
    fps: "Frame rate",
    cap: "Cap",
    since: "Since",
    turnOn: "Turn on the host",
    token: "Token",
    hideToken: "Hide the token",
    showToken: "Show the token",
    giveToken: "Give it to those who will connect.",
    copied: "Copied",
    copy: "Copy",
    newToken: "Change the token",
    newTokenTitle: "Change the token?",
    change: "Change",
    newTokenText: "Nobody can connect to this machine with the old token any more: everyone who connected will have to enter the new one.",
    noApproval: "The administrator does not need to approve again.",
    removeToChangeAll: "To change the name or the server, remove the registration.",
    removeToChangeServer: "To change the server, remove the registration.",
    checking: "Checking…",
    serverSays: (server: string, what: string) => `Server ${server}: ${what}`,
    connection: "Connection to the server",
    machineName: "Machine name",
    server: "Server",
    register: "Register",
    unregister: "Remove registration",
    unregisterTitle: "Remove the registration?",
    remove: "Remove",
    unregisterText: "Nobody can connect to the machine until it is registered again. Then clients will need the administrator's approval.",
    unregisterServer: "If the server is reachable, the record is removed there too. The token stays the same.",
    machine: "Machine",
    welcome: "Welcome",
    intro: "Enter this machine's name and your organisation's server.",
    later: "After the registration the token appears here: give it to those who will connect.",
    registering: "Registering…",
    allGood: "All good, here is your token!",
    tokenLater: "It can always be seen and copied on the Machine tab.",
  },
  {
    fps: "Частота",
    cap: "Потолок",
    since: "С",
    turnOn: "Включить хост",
    token: "Токен",
    hideToken: "Скрыть токен",
    showToken: "Показать токен",
    giveToken: "Дайте его тем, кто будет подключаться.",
    copied: "Скопировано",
    copy: "Скопировать",
    newToken: "Сменить токен",
    newTokenTitle: "Сменить токен?",
    change: "Сменить",
    newTokenText: "Со старым токеном к этой машине больше не подключиться: всем, кто подключался, придётся ввести новый.",
    noApproval: "Повторное одобрение администратора не нужно.",
    removeToChangeAll: "Чтобы сменить имя или сервер, удалите регистрацию.",
    removeToChangeServer: "Чтобы сменить сервер, удалите регистрацию.",
    checking: "Проверяю…",
    serverSays: (server: string, what: string) => `Сервер ${server}: ${what}`,
    connection: "Подключение к серверу",
    machineName: "Имя машины",
    server: "Сервер",
    register: "Зарегистрировать",
    unregister: "Удалить регистрацию",
    unregisterTitle: "Удалить регистрацию?",
    remove: "Удалить",
    unregisterText: "Подключиться к машине будет нельзя, пока её не зарегистрируют снова. Тогда клиентам понадобится одобрение администратора.",
    unregisterServer: "Если сервер доступен, запись удалится и на нём. Токен останется прежним.",
    machine: "Машина",
    welcome: "Добро пожаловать",
    intro: "Введите имя этой машины и сервер организации.",
    later: "После регистрации здесь появится токен: его дают тем, кто будет подключаться.",
    registering: "Регистрирую…",
    allGood: "Всё готово, вот ваш токен!",
    tokenLater: "Его всегда можно посмотреть и скопировать на вкладке «Машина».",
  },
);

/** The left half of the tab: what is happening on this machine right now. */
function Now({ s, run }: { s: AppState; run: Run }) {
  const w = W();
  const st = hostStatus(s);
  const session = !s.host_off && !s.service_error && s.registered && s.status.state === "session";
  const since = s.status.since * 1000;
  return (
    <section className="now">
      <div className="now-mark">
        <Icon name="desktop" size={28} />
      </div>
      <Pill tone={st.tone}>{st.title}</Pill>
      {session ? (
        <>
          <h3 className="now-title selectable" title={s.status.client}>
            {s.status.client}
          </h3>
          <dl className="now-facts">
            <div>
              <dt>{w.fps}</dt>
              <dd>
                {fps(s.status.fps)}
              </dd>
            </div>
            <div>
              <dt>{w.cap}</dt>
              <dd>{cap(s.status.mbps)}</dd>
            </div>
            <div>
              <dt>
                {w.since} {new Date(since).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" })}
              </dt>
              <dd>{hoursMinutes((Date.now() - since) / 1000)}</dd>
            </div>
          </dl>
        </>
      ) : (
        <>
          {st.text && <p className="now-text selectable">{st.text}</p>}
          {s.host_off && (
            <button className="btn primary now-action" onClick={() => run(api.startHost)}>
              <Icon name="power" />
              {w.turnOn}
            </button>
          )}
        </>
      )}
    </section>
  );
}

/** The token in one line: dots and dashes until a click on it, a click again hides it. */
function TokenValue({ token }: { token: string }) {
  const w = W();
  const [shown, setShown] = useState(false);
  return (
    <button className={`token-value ${shown ? "selectable" : ""}`} title={shown ? w.hideToken : w.showToken} aria-label={shown ? w.hideToken : w.showToken} onClick={() => setShown(!shown)}>
      {shown ? token : token.replace(/[^-]/g, "•")}
    </button>
  );
}

function TokenCard({ s, run }: { s: AppState; run: Run }) {
  const w = W();
  const [copied, setCopied] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const off = !!s.service_error;
  const taken = s.status.state === "refused" && s.status.detail === "TokenTaken";
  const copy = async () => {
    await navigator.clipboard.writeText(s.token ?? "");
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <section className="card">
      <header className="card-head">
        <Icon name="key" />
        <h3>{w.token}</h3>
      </header>
      <TokenValue token={s.token!} />
      <p className="card-text">{w.giveToken}</p>
      <div className="card-actions">
        <button className="btn primary grow" disabled={off} onClick={copy}>
          <Icon name="copy" />
          {copied ? w.copied : w.copy}
        </button>
        <button className={`btn square ${taken ? "attention" : ""}`} title={w.newToken} aria-label={w.newToken} disabled={off} onClick={() => setConfirm(true)}>
          <Icon name="refresh" size={18} />
        </button>
      </div>
      {confirm && (
        <Confirm title={w.newTokenTitle} action={w.change} onClose={() => setConfirm(false)} onConfirm={() => run(api.newToken)}>
          <p>{w.newTokenText}</p>
          <p>{w.noApproval}</p>
        </Confirm>
      )}
    </section>
  );
}

/** `onLang`: the card is the welcome page's form (Welcome): its own head with the globe, the words under it. */
function ServerCard({ s, run, error, note, setNote, onLang }: { s: AppState; run: Run; error: string; note: string; setNote: (n: string) => void; onLang?: (l: Lang) => void }) {
  const w = W();
  const [machine, setMachine] = useState(s.settings.machine);
  const [server, setServer] = useState(serverField(s.settings.server));
  const [confirm, setConfirm] = useState(false);
  const [trusting, setTrusting] = useState<CertProblem | null>(null);
  const off = !!s.service_error;
  const refused = s.registered && s.status.state === "refused";
  const unknown = refused && s.status.detail === "BadCredentials";
  const badName = refused && s.status.detail === "BadName";
  // The name is the server's once registered; it changes only while the server does not hold it.
  const nameLocked = s.registered && !unknown && !badName;
  const canRegister = !off && !s.host_off && (!s.registered || unknown);

  // What the service keeps comes back as it was checked (the server with its port); a new start of the
  // service or a new registration begins with that.
  useEffect(() => {
    setMachine(s.settings.machine);
    setServer(serverField(s.settings.server));
  }, [s.registered, off, s.settings.machine, s.settings.server]);

  const shownServer = s.registered ? s.settings.server : server;
  const shownMachine = nameLocked ? s.settings.machine : machine;
  const wanted = (): Settings => ({ ...s.settings, server: shownServer, machine: shownMachine });
  // What is wrong with the name, as the server takes it (Latin only, 64 bytes), in Lumen Client's words: said at
  // once, Register waits.
  const problem = nameLocked ? "" : nameProblem(shownMachine);
  // A new registration trusts anew: the key of the server in the form, once the user has seen it.
  const pinned = s.registered ? s.settings.server_fingerprint : "";
  const [checking, setChecking] = useState(false);
  const ready = canRegister && !problem && !checking && !!shownServer.trim();

  const enroll = async (fingerprint: string) => {
    const settings = { ...wanted(), server_fingerprint: fingerprint };
    if (!s.registered) return api.save(settings);
    if (machine.trim() !== s.settings.machine || fingerprint !== s.settings.server_fingerprint) await api.save(settings);
    return api.reregister();
  };
  // Register looks at the server once, as Lumen Client's Continue (owner's decision 2026-11-09): it answers — the
  // registration; its own certificate — its key first (TrustDialog); else what the server did, in the line.
  const register = async () => {
    setChecking(true);
    await run(async () => {
      try {
        await api.probe(shownServer, s.settings.server_ca, pinned);
      } catch (e) {
        const cert = certProblem(e);
        if (cert) return setTrusting(cert);
        throw w.serverSays(serverName(shownServer.trim()), errorText(e));
      }
      await enroll(pinned);
    });
    setChecking(false);
  };

  const line = error ? (
    <CardLine tone="field-error selectable" title={error}>
      {error}
    </CardLine>
  ) : note ? (
    <CardLine tone="selectable" title={note}>
      {note}
    </CardLine>
  ) : problem ? (
    <CardLine tone="field-error">{problem}</CardLine>
  ) : onLang && s.registered ? (
    <CardLine />
  ) : !canRegister ? (
    s.registered ? (
      <CardLine lock>{nameLocked ? w.removeToChangeAll : w.removeToChangeServer}</CardLine>
    ) : (
      <CardLine />
    )
  ) : (
    <CardLine />
  );

  return (
    <section className={onLang ? "welcome-form" : "card"}>
      {onLang ? (
        <>
          <WelcomeHead title={w.welcome} onLang={onLang} />
          <p className="welcome-intro">{w.intro}</p>
        </>
      ) : (
        <header className="card-head">
          <Icon name="server" />
          <h3>{w.connection}</h3>
        </header>
      )}
      <label className="field">
        <span>{w.machineName}</span>
        <input
          type="text"
          value={shownMachine}
          placeholder={s.defaults.machine}
          disabled={off || nameLocked}
          onChange={(e) => {
            setNote("");
            setMachine(e.target.value);
          }}
          onBlur={() => {
            if (badName && !problem && machine.trim() !== s.settings.machine) run(() => api.save(wanted()));
          }}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />
      </label>
      <label className="field">
        <span>{w.server}</span>
        <input
          className="mono"
          type="text"
          value={s.registered ? serverName(shownServer) : shownServer}
          placeholder="lumen.example.com"
          disabled={s.registered || off}
          onChange={(e) => {
            setNote("");
            setServer(e.target.value);
          }}
          onKeyDown={(e) => e.key === "Enter" && ready && register()}
        />
      </label>
      {line}
      <div className="card-actions">
        {canRegister && (
          <button
            className="btn primary grow"
            disabled={!ready}
            onClick={register}
          >
            {checking ? w.checking : w.register}
          </button>
        )}
        {onLang && s.registered && (
          <button className="btn primary grow" disabled>
            {w.registering}
          </button>
        )}
        {!onLang && s.registered && (
          <button className="btn danger grow" disabled={off} onClick={() => setConfirm(true)}>
            <Icon name="delete" />
            {w.unregister}
          </button>
        )}
      </div>
      {onLang && <p className="welcome-later">{w.later}</p>}
      {trusting && (
        <TrustDialog
          server={shownServer.trim()}
          problem={trusting}
          onCancel={() => setTrusting(null)}
          onTrust={() => {
            setTrusting(null);
            run(() => enroll(trusting.fingerprint));
          }}
        />
      )}
      {confirm && (
        <Confirm title={w.unregisterTitle} action={w.remove} onClose={() => setConfirm(false)} onConfirm={() => run(async () => setNote(errorText(await api.unregister())))}>
          <p>{w.unregisterText}</p>
          <p>{w.unregisterServer}</p>
        </Confirm>
      )}
    </section>
  );
}

/** The token once the server took the machine: copied, the usual window opens (`onDone`). */
function TokenWelcome({ token, onLang, onDone }: { token: string; onLang: (l: Lang) => void; onDone: () => void }) {
  const w = W();
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(token);
    } finally {
      onDone();
    }
  };
  return (
    <section className="welcome-form">
      <WelcomeHead title={w.allGood} onLang={onLang} />
      <p className="welcome-intro">{w.giveToken}</p>
      <TokenValue token={token} />
      <div className="card-actions">
        <button className="btn primary grow" autoFocus onClick={copy}>
          <Icon name="copy" />
          {w.copy}
        </button>
      </div>
      <p className="welcome-later">{w.tokenLater}</p>
    </section>
  );
}

/** The whole window on a clean machine's first start, as Lumen Client's (web/client/src/Welcome.tsx): the app's
 * mark on the left, the server card's fields on the right; `token` — the server took the machine (App), the token
 * instead of the fields. */
export function Welcome({ s, refresh, onLang, token, onDone }: { s: AppState; refresh: () => Promise<void>; onLang: (l: Lang) => void; token?: string; onDone: () => void }) {
  const { error, run } = useRun(refresh);
  const [note, setNote] = useState("");
  return (
    <section className="welcome">
      <WelcomeHero name="Lumen Host" />
      {token ? <TokenWelcome token={token} onLang={onLang} onDone={onDone} /> : <ServerCard s={s} run={run} error={error} note={note} setNote={setNote} onLang={onLang} />}
    </section>
  );
}

export function Machine({ s, refresh }: { s: AppState; refresh: () => Promise<void> }) {
  const w = W();
  const { error, run: runCore } = useRun(refresh);
  const [note, setNote] = useState("");
  const run: Run = (f) => {
    setNote("");
    return runCore(f);
  };
  return (
    <section className="view">
      <header className="view-head">
        <Icon name="desktop" size={19} />
        <h2>{w.machine}</h2>
      </header>
      <div className="machine-grid">
        <Now s={s} run={run} />
        <div className="machine-side">
          <ServerCard s={s} run={run} error={error} note={note} setNote={setNote} />
          {s.token && s.registered && <TokenCard s={s} run={run} />}
        </div>
      </div>
    </section>
  );
}
