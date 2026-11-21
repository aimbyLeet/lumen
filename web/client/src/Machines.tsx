import { useState } from "react";
import { api, type AppState, type Card, type Run } from "./api";
import { errorText } from "./errors";
import { words } from "./i18n";
import { buttonLabel, cardStatus, versionDetail, videoFailed, videoStatus, videoTitle } from "./status";
import { certProblem, TrustDialog, type CertProblem } from "./TrustDialog";
import { Confirm, Dialog, Icon, Pill, TokenInput } from "./ui";
import { serverName } from "./units";

const W = words(
  {
    hideToken: "Hide the token",
    showToken: "Show the token",
    machine: "Machine",
    token: "Token",
    disconnect: "Disconnect",
    checkCert: "Check the certificate",
    removeMachine: "Remove the machine",
    options: "Options",
    hideOptions: "Hide the options",
    restart: "Restart",
    restartTitle: (machine: string) => `Restart ${machine || "the machine"}?`,
    restartText: "Windows restarts right away: whoever is connected is disconnected, open programs are closed without saving.",
    restartBack: "In a minute or two the machine is online again, at the Windows sign-in screen.",
    addMachine: "Add a machine",
    cancel: "Cancel",
    adding: "Adding…",
    add: "Add",
    tokenField: "The machine's token (Lumen Host shows it)",
    firstApproval: "The server's administrator approves the first connection to a machine.",
    removeTitle: (machine: string) => `Remove ${machine || "the machine"}?`,
    remove: "Remove",
    removeRequest: "The request to connect to this machine is removed from the server too.",
    removeLast: (server: string) => (
      <>
        This is the last machine: this computer is removed from the server <code>{server}</code> too.
      </>
    ),
    machines: "Machines",
    newConnection: "New connection",
    newHint: "The token that Lumen Host shows.",
    firstHint: "Add your first machine: you need the token that Lumen Host shows.",
  },
  {
    hideToken: "Скрыть токен",
    showToken: "Показать токен",
    machine: "Машина",
    token: "Токен",
    disconnect: "Отключиться",
    checkCert: "Проверить сертификат",
    removeMachine: "Удалить машину",
    options: "Опции",
    hideOptions: "Скрыть опции",
    restart: "Перезагрузить",
    restartTitle: (machine: string) => `Перезагрузить ${machine || "машину"}?`,
    restartText: "Windows перезагрузится сразу: все подключённые будут отключены, открытые программы закроются без сохранения.",
    restartBack: "Через минуту-другую машина снова будет в сети, на экране входа Windows.",
    addMachine: "Добавить машину",
    cancel: "Отмена",
    adding: "Добавляю…",
    add: "Добавить",
    tokenField: "Токен машины (показывает Lumen Host)",
    firstApproval: "Первое подключение к машине одобряет администратор сервера.",
    removeTitle: (machine: string) => `Удалить ${machine || "машину"}?`,
    remove: "Удалить",
    removeRequest: "Заявка на подключение к этой машине на сервере тоже удалится.",
    removeLast: (server: string) => (
      <>
        Это последняя машина: этот компьютер удалится и с сервера <code>{server}</code>.
      </>
    ),
    machines: "Машины",
    newConnection: "Новое подключение",
    newHint: "Токен, который показывает Lumen Host.",
    firstHint: "Добавьте первую машину: нужен токен, который показывает Lumen Host.",
  },
);

function Token({ token, on }: { token: string; on: boolean }) {
  const [shown, setShown] = useState(false);
  const w = W();
  const masked = token.replace(/[^-]/g, "•");
  return (
    <button
      className={`token ${on ? "on" : ""} ${shown ? "shown selectable" : ""}`}
      title={shown ? w.hideToken : w.showToken}
      onClick={() => setShown(!shown)}
    >
      {shown ? token : masked}
    </button>
  );
}

function Row({ c, s, busy, run, onRemove, onRestart }: { c: Card; s: AppState; busy: boolean; run: Run; onRemove: () => void; onRestart: () => void }) {
  const w = W();
  const session = s.video?.card === c.id ? s.video.status : null;
  const failed = s.last_video?.card === c.id && !!s.last_video.error;
  const st = session ? videoStatus(session) : failed ? videoFailed() : cardStatus(c, s.version);
  const b = c.button;
  const usable = !session && b.enabled && (b.op === "connect" || b.op === "update");
  // The server's key changed (or is not trusted yet): the user decides, the button asks.
  const cert = session ? null : certProblem(c.server_error);
  const [trusting, setTrusting] = useState(false);
  // The options take the main button's place: restart and remove.
  const [options, setOptions] = useState(false);
  const restartable = c.access === "approved" && ["online", "busy", "inactive"].includes(c.presence);
  const title = c.machine ? (c.account ? `${c.machine} / ${c.account}` : c.machine) : w.machine;
  return (
    <li className={`machine ${st.tone === "off" ? "dim" : ""}`}>
      <div className="machine-main">
        <div className="machine-id">
          <div className="machine-title">
            <span className={`machine-icon ${usable ? "on" : ""}`}>
              <Icon name="desktop" size={18} />
            </span>
            <h3 title={title}>{title}</h3>
          </div>
          <dl className="machine-facts">
            <dt>{w.token}</dt>
            <dd>
              <Token token={c.token} on={usable} />
            </dd>
          </dl>
        </div>
        <div className="machine-status">
          <Pill tone={st.tone} detail={versionDetail(c, s.version)}>
            {st.pill}
          </Pill>
        </div>
        <div className="machine-actions">
          {options ? (
            <>
              <button
                className="btn half"
                title={w.restart}
                aria-label={w.restart}
                disabled={!restartable || busy}
                onClick={() => {
                  setOptions(false);
                  onRestart();
                }}
              >
                <Icon name="refresh" />
              </button>
              <button
                className="btn half danger"
                title={w.removeMachine}
                aria-label={w.removeMachine}
                disabled={busy}
                onClick={() => {
                  setOptions(false);
                  onRemove();
                }}
              >
                <Icon name="delete" />
              </button>
            </>
          ) : session ? (
            <button className="btn" disabled={busy} onClick={() => run(api.disconnect)}>
              <Icon name="stop" />
              {w.disconnect}
            </button>
          ) : cert ? (
            <button className="btn attention" disabled={busy} onClick={() => setTrusting(true)}>
              <Icon name="shield" />
              {w.checkCert}
            </button>
          ) : (
            <button
              className={`btn ${b.enabled ? "primary" : ""}`}
              disabled={!b.enabled || busy}
              onClick={() => run(() => (b.op === "request" ? api.requestAccess(c.id) : api.connect(c.id, videoTitle(c.machine, c.account))))}
            >
              {usable && <Icon name="play" />}
              {buttonLabel(b.op)}
            </button>
          )}
          <button
            className="btn square"
            title={options ? w.hideOptions : w.options}
            aria-label={options ? w.hideOptions : w.options}
            aria-expanded={options}
            onClick={() => setOptions(!options)}
          >
            <Icon name={options ? "close" : "more"} />
          </button>
        </div>
      </div>
      {trusting && cert && (
        <TrustDialog
          server={s.settings.server}
          problem={cert}
          onCancel={() => setTrusting(false)}
          onTrust={() => {
            setTrusting(false);
            run(() => api.trustServer(s.settings.server, cert.fingerprint));
          }}
        />
      )}
    </li>
  );
}

/** Only the token: the server is the client's one. */
function AddDialog({ server, onClose, onAdded }: { server: string; onClose: () => void; onAdded: () => void }) {
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const w = W();
  const [adding, setAdding] = useState(false);
  // A server with its own certificate: its key, before the machine is added.
  const [cert, setCert] = useState<CertProblem | null>(null);
  const ready = !adding && !!token.trim();
  const add = async () => {
    setAdding(true);
    setError("");
    try {
      await api.addMachine(token);
      onAdded();
    } catch (e) {
      const p = certProblem(e);
      if (p) setCert(p);
      else setError(errorText(e));
      setAdding(false);
    }
  };
  if (cert)
    return (
      <TrustDialog
        server={server}
        problem={cert}
        onCancel={() => setCert(null)}
        onTrust={async () => {
          setCert(null);
          try {
            await api.trustServer(server, cert.fingerprint);
          } catch (e) {
            setError(errorText(e));
            return;
          }
          add();
        }}
      />
    );
  return (
    <Dialog
      title={w.addMachine}
      onClose={onClose}
      footer={
        <>
          <button className="btn plain" onClick={onClose}>
            {w.cancel}
          </button>
          <button className="btn primary" disabled={!ready} onClick={() => ready && add()}>
            {adding ? w.adding : w.add}
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) add();
        }}
      >
        <label className="field">
          <span>{w.tokenField}</span>
          <TokenInput value={token} onChange={setToken} />
        </label>
        <button type="submit" hidden />
      </form>
      <p className="notice">
        <Icon name="shield" size={16} />
        {w.firstApproval}
      </p>
      {error && <p className="dialog-error selectable">{error}</p>}
    </Dialog>
  );
}

function RemoveDialog({ card, server, last, onClose, onRemove }: { card: Card; server: string; last: boolean; onClose: () => void; onRemove: () => void }) {
  const w = W();
  return (
    <Confirm title={w.removeTitle(card.machine)} action={w.remove} onClose={onClose} onConfirm={onRemove}>
      <p>{w.removeRequest}</p>
      {last && <p>{w.removeLast(serverName(server))}</p>}
    </Confirm>
  );
}

function RestartDialog({ card, onClose, onRestart }: { card: Card; onClose: () => void; onRestart: () => void }) {
  const w = W();
  return (
    <Confirm title={w.restartTitle(card.machine)} action={w.restart} onClose={onClose} onConfirm={onRestart}>
      <p>{w.restartText}</p>
      <p>{w.restartBack}</p>
    </Confirm>
  );
}

export function Machines({ s, busy, run, onNote }: { s: AppState; busy: boolean; run: Run; onNote: (n: string) => void }) {
  const w = W();
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<Card | null>(null);
  const [restarting, setRestarting] = useState<Card | null>(null);
  return (
    <section className="view">
      <header className="view-head">
        <Icon name="devices" size={19} />
        <h2>{w.machines}</h2>
        <span className="count">{s.cards.length}</span>
      </header>
      <ul className="rows">
        {s.cards.map((c) => (
          <Row key={c.id} c={c} s={s} busy={busy} run={run} onRemove={() => setRemoving(c)} onRestart={() => setRestarting(c)} />
        ))}
        <li className="machine add" onClick={() => setAdding(true)}>
          <div className="machine-main">
            <div className="machine-id">
              <h3>{w.newConnection}</h3>
              <p>{s.cards.length ? w.newHint : w.firstHint}</p>
            </div>
            <div className="machine-status" />
            <div className="machine-actions">
              <button className="btn dashed">
                <Icon name="add" />
                {w.add}
              </button>
              <span className="square-space" />
            </div>
          </div>
        </li>
      </ul>
      {adding && (
        <AddDialog
          server={s.settings.server}
          onClose={() => setAdding(false)}
          onAdded={() => {
            setAdding(false);
            // At once, not at the next poll.
            run(async () => {});
          }}
        />
      )}
      {restarting && (
        <RestartDialog
          card={restarting}
          onClose={() => setRestarting(null)}
          onRestart={() => {
            const id = restarting.id;
            run(() => api.rebootMachine(id));
          }}
        />
      )}
      {removing && (
        <RemoveDialog
          card={removing}
          server={s.settings.server}
          last={!s.cards.some((c) => c.id !== removing.id)}
          onClose={() => setRemoving(null)}
          onRemove={() => {
            const id = removing.id;
            run(async () => onNote(await api.removeMachine(id)));
          }}
        />
      )}
    </section>
  );
}
