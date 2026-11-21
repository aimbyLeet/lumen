// The other way round: all the machines by name, each with the clients that may reach it.
import { words } from "../../client/src/i18n";
import { clientLabelOf, hostLabel, type Host, type State } from "./api";
import { fps, hoursMinutes } from "../../client/src/units";
import { errorText } from "../../client/src/errors";
import { AccessRow, ago, alphabetically, Block, cap, EntryFoot, ListPage, matches, useConfirm, useOpen, type Act, type Ask } from "./common";

const T = words(
  {
    machines: "Machines",
    none: "No machines yet. They appear here when Lumen Host is registered on a computer.",
    session: (who: string) => `Session: ${who}`,
    onlineNoSession: "Online, no session",
    offlineSeen: (ago: string) => `Offline, last seen ${ago}`,
    updating: "Installing a newer Lumen…",
    updateFailed: (why: string) => `The update failed: ${why}`,
    connections: "Connections",
    noAccess: "Nobody has access.",
    what: "the machine",
    removeText: (label: string, session: boolean) => `${label}: its registration and all access to it will be gone.${session ? " The session in progress will be cut off." : ""}`,
  },
  {
    machines: "Машины",
    none: "Машин пока нет. Они появятся здесь, когда на компьютере зарегистрируют Lumen Host.",
    session: (who: string) => `Сессия: ${who}`,
    onlineNoSession: "В сети, сессии нет",
    offlineSeen: (ago: string) => `Не в сети, была ${ago}`,
    updating: "Устанавливает новую версию Lumen…",
    updateFailed: (why: string) => `Обновление не удалось: ${why}`,
    connections: "Подключения",
    noAccess: "Ни у кого нет доступа.",
    what: "машину",
    removeText: (label: string, session: boolean) => `${label}: регистрация и все доступы к ней пропадут.${session ? " Идущая сессия прервётся." : ""}`,
  },
);

export function Machines({ s, act, query, onQuery }: { s: State; act: Act; query: string; onQuery: (q: string) => void }) {
  const t = T();
  const { ask, dialog } = useConfirm();
  const { isOpen, toggle } = useOpen();
  const clients = (h: Host) => s.access.filter((a) => a.host === h.id).map((a) => clientLabelOf(s, a.client, a.client_name));
  const shown = alphabetically(s.hosts, hostLabel).filter((h) => matches(query, hostLabel(h), ...clients(h)));
  // Found by a client: open, or the reason would not be visible.
  const foundInside = (h: Host) => !!query.trim() && !matches(query, hostLabel(h));
  return (
    <ListPage icon="desktop" title={t.machines} total={s.hosts.length} shown={shown.length} none={t.none} query={query} onQuery={onQuery} dialog={dialog}>
      {shown.map((h) => (
        <MachineEntry key={h.id} s={s} h={h} open={isOpen(h.id) || foundInside(h)} onToggle={() => toggle(h.id)} act={act} ask={ask} />
      ))}
    </ListPage>
  );
}

function MachineEntry({ s, h, open, onToggle, act, ask }: { s: State; h: Host; open: boolean; onToggle: () => void; act: Act; ask: Ask }) {
  const t = T();
  const label = hostLabel(h);
  const session = s.sessions.find((x) => x.host === h.id);
  const access = alphabetically(
    s.access.filter((a) => a.host === h.id),
    (a) => clientLabelOf(s, a.client, a.client_name),
  );
  const now = session
    ? `${t.session(clientLabelOf(s, session.client, session.client_name))} · ${fps(session.fps)} · ${cap(session.mbps)} · ${hoursMinutes(session.seconds)}`
    : h.updating
      ? t.updating
      : h.online
        ? t.onlineNoSession
        : t.offlineSeen(ago(s.now, h.last_seen));
  return (
    <Block icon="desktop" name={label} session={!!session} online={h.online} open={open} onToggle={onToggle}>
      <p className="entry-now selectable">{now}</p>
      {h.update_failed && <p className="entry-now bad selectable">{t.updateFailed(errorText(h.update_failed))}</p>}
      <h4 className="section-title">{t.connections}</h4>
      <ul className="panel sub">
        {access.length === 0 ? <li className="sub-empty">{t.noAccess}</li> : access.map((a) => <AccessRow key={a.client} s={s} a={a} side="machine" act={act} ask={ask} />)}
      </ul>
      <EntryFoot version={h.version} id={h.id} what={t.what} text={t.removeText(label, !!session)} act={act} ask={ask} />
    </Block>
  );
}
