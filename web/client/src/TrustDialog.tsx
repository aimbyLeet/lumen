// A server with its own certificate (a local network, no domain): the first time the user sees its key's
// fingerprint and decides whether to trust it; later only if the key changed. The core reports
// `err_cert_untrusted|<fingerprint>` or `err_cert_changed|<fingerprint>` (crates/signal/src/pin.rs).
// Used by Lumen Client and Lumen Host.
import { words } from "./i18n";
import { Dialog, Icon } from "./ui";
import { serverName } from "./units";

export type CertProblem = { kind: "untrusted" | "changed"; fingerprint: string };

/** The certificate question in an error from the core, if that is what it is. */
export function certProblem(e: unknown): CertProblem | null {
  const m = /^(err_cert_untrusted|err_cert_changed)\|(.+)$/.exec(String(e).trim());
  if (!m) return null;
  return { kind: m[1] === "err_cert_untrusted" ? "untrusted" : "changed", fingerprint: m[2].trim() };
}

const W = words(
  {
    untrustedTitle: "Trust this server?",
    changedTitle: "The server's certificate has changed",
    untrusted: (server: string) => (
      <>
        The server <code>{server}</code> has its own certificate (a server without a domain, in a local network). Make sure
        it is yours: its fingerprint must be the same as the one the server's installation showed.
      </>
    ),
    changed: (server: string) => (
      <>
        The server <code>{server}</code> now has another key. This is expected only if the server was reinstalled. If it was
        not, someone may be pretending to be your server: do not trust it, ask the administrator.
      </>
    ),
    fingerprint: "Fingerprint",
    where: "The administrator sees the fingerprint at the end of the server's installation and with install.sh → Status.",
    cancel: "Cancel",
    trust: "Trust",
    trustNew: "Trust the new key",
  },
  {
    untrustedTitle: "Доверять этому серверу?",
    changedTitle: "Сертификат сервера изменился",
    untrusted: (server: string) => (
      <>
        У сервера <code>{server}</code> собственный сертификат (сервер без домена, в локальной сети). Убедитесь, что это ваш
        сервер: отпечаток должен совпадать с тем, который показала установка сервера.
      </>
    ),
    changed: (server: string) => (
      <>
        У сервера <code>{server}</code> теперь другой ключ. Так бывает, только если сервер переустановили. Если нет — кто-то
        может выдавать себя за ваш сервер: не доверяйте, спросите администратора.
      </>
    ),
    fingerprint: "Отпечаток",
    where: "Администратор видит отпечаток в конце установки сервера и в install.sh → Состояние.",
    cancel: "Отмена",
    trust: "Доверять",
    trustNew: "Доверять новому ключу",
  },
);

/** The fingerprint in two lines of eight groups. */
function Fingerprint({ value }: { value: string }) {
  const groups = value.split(/\s+/);
  return (
    <code className="fingerprint selectable">
      {groups.slice(0, 8).join(" ")}
      <br />
      {groups.slice(8).join(" ")}
    </code>
  );
}

/** «Cancel» is the default: trusting is a decision. */
export function TrustDialog({
  server,
  problem,
  onCancel,
  onTrust,
}: {
  server: string;
  problem: CertProblem;
  onCancel: () => void;
  onTrust: () => void;
}) {
  const w = W();
  const changed = problem.kind === "changed";
  return (
    <Dialog
      title={changed ? w.changedTitle : w.untrustedTitle}
      onClose={onCancel}
      footer={
        <>
          <button className="btn plain dialog-default" onClick={onCancel}>
            {w.cancel}
          </button>
          <button className={`btn ${changed ? "solid-danger" : "primary"}`} onClick={onTrust}>
            {changed ? w.trustNew : w.trust}
          </button>
        </>
      }
    >
      <p>{changed ? w.changed(serverName(server)) : w.untrusted(serverName(server))}</p>
      <div className="field">
        <span>{w.fingerprint}</span>
        <Fingerprint value={problem.fingerprint} />
      </div>
      <p className="notice">
        <Icon name="shield" size={16} />
        {w.where}
      </p>
    </Dialog>
  );
}
