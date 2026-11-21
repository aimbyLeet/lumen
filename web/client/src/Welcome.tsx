// The first start: the name and the organisation's one server. «Continue» checks the server
// at once (a server with its own certificate: its key first, TrustDialog); one that does not answer is not kept.
// The settings change them the same way (useIdentity), while there are no machines.
import { useState } from "react";
import { api } from "./api";
import { errorText } from "./errors";
import { words } from "./i18n";
import { certProblem, TrustDialog, type CertProblem } from "./TrustDialog";
import type { Lang } from "./i18n";
import { CardLine, nameProblem, WelcomeHead, WelcomeHero } from "./ui";
import { serverName } from "./units";

const W = words(
  {
    title: "Welcome",
    intro: "Enter your name and your organisation's server.",
    name: "Your name",
    enterName: "Enter your name",
    server: "Server",
    later: "The name and the server can be changed in the settings while the list of machines is empty.",
    checking: "Checking…",
    proceed: "Continue",
    serverSays: (server: string, what: string) => `Server ${server}: ${what}`,
  },
  {
    title: "Добро пожаловать",
    intro: "Введите своё имя и сервер организации.",
    name: "Ваше имя",
    enterName: "Введите имя",
    server: "Сервер",
    later: "Имя и сервер можно поменять в настройках, пока список машин пуст.",
    checking: "Проверяю…",
    proceed: "Продолжить",
    serverSays: (server: string, what: string) => `Сервер ${server}: ${what}`,
  },
);

/** Codes about the fields themselves; anything else is what the server did. */
const FIELD_CODES = ["err_name_required", "err_name_invalid", "err_name_locked", "err_server_address", "err_server_has_machines", "err_server_required"];

/** Keeping the name and the server: the server checked, its key asked about. `dialog` — the question, when there is one. */
export function useIdentity(onDone: () => void) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [cert, setCert] = useState<{ name: string; server: string; problem: CertProblem } | null>(null);
  const save = async (name: string, server: string) => {
    setSaving(true);
    setError("");
    try {
      await api.setIdentity(name, server);
      onDone();
    } catch (e) {
      const p = certProblem(e);
      const code = String(e).split("|")[0];
      if (p) setCert({ name, server, problem: p });
      else setError(FIELD_CODES.includes(code) ? errorText(e) : W().serverSays(serverName(server.trim()), errorText(e)));
    } finally {
      setSaving(false);
    }
  };
  const dialog = cert && (
    <TrustDialog
      server={cert.server.trim()}
      problem={cert.problem}
      onCancel={() => setCert(null)}
      onTrust={async () => {
        setCert(null);
        try {
          await api.trustServer(cert.server.trim(), cert.problem.fingerprint);
        } catch (e) {
          setError(errorText(e));
          return;
        }
        save(cert.name, cert.server);
      }}
    />
  );
  return { save, saving, error, setError, dialog };
}

/** The whole window on the first start, a landing page rather than a dialog (the owner's wish 2026-11-09): the
 * app's mark on the left, the two fields on the right. The tabs stay hidden meanwhile (App), so the globe is here,
 * at the right of the form's head (as at the admin's sign-in). */
export function Welcome({ onDone, onLang }: { onDone: () => void; onLang: (l: Lang) => void }) {
  const w = W();
  const [name, setName] = useState("");
  const [server, setServer] = useState("");
  const id = useIdentity(onDone);
  const problem = nameProblem(name);
  const ready = !id.saving && !!name.trim() && !!server.trim() && !problem;
  return (
    <section className="welcome">
      <WelcomeHero name="Lumen Client" />
      <form
        className="welcome-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) id.save(name, server);
        }}
      >
        <WelcomeHead title={w.title} onLang={onLang} />
        <p className="welcome-intro">{w.intro}</p>
        <label className="field">
          <span>{w.name}</span>
          <input type="text" value={name} placeholder={w.enterName} autoFocus onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="field">
          <span>{w.server}</span>
          <input className="mono" type="text" value={server} placeholder="lumen.example.com" onChange={(e) => setServer(e.target.value)} />
        </label>
        {/* One line under the fields, as Lumen Host's: what the server said, else what is wrong with the name. */}
        {id.error ? (
          <CardLine tone="field-error selectable" title={id.error}>
            {id.error}
          </CardLine>
        ) : problem ? (
          <CardLine tone="field-error">{problem}</CardLine>
        ) : (
          <CardLine />
        )}
        <button type="submit" className="btn primary wide" disabled={!ready}>
          {id.saving ? w.checking : w.proceed}
        </button>
        <p className="welcome-later">{w.later}</p>
      </form>
      {id.dialog}
    </section>
  );
}
