// The admin's main tab: all the clients by name, each with the machines it may reach.
import { useRef, useState } from "react";
import { words } from "../../client/src/i18n";
import { Dialog, Icon, nameProblem } from "../../client/src/ui";
import { api, clientLabel, hostLabel, type Client, type Limits, type State } from "./api";
import { fps, hoursMinutes } from "../../client/src/units";
import { AccessRow, ago, alphabetically, Block, cap, CapSelect, EntryFoot, FpsSelect, ListPage, matches, useConfirm, useOpen, type Act, type Ask } from "./common";

const T = words(
  {
    clients: "Clients",
    none: "No clients yet. They appear here when someone adds a machine in Lumen Client.",
    sessionWith: (machine: string) => `Session with ${machine}`,
    onlineNoSession: "Online, no session",
    offlineSeen: (ago: string) => `Offline, last seen ${ago}`,
    settings: "Settings",
    fps: "Frame rate",
    clientChooses: "The client chooses",
    asEveryone: (what: string) => `As for everyone: ${what}`,
    cap: "Bitrate cap",
    files: "File transfer",
    filesLabel: "Files",
    allowed: "allowed",
    notAllowed: "not allowed",
    allow: "Allowed",
    forbid: "Not allowed",
    connections: "Connections",
    unlinkAllTitle: "Unlink from all machines?",
    unlink: "Unlink",
    unlinkAllText: (name: string, n: number) => `${name}: ${n} machines. The session in progress will be cut off; after that — only a new request.`,
    unlinkAll: "Unlink from all",
    noAccess: "No access to any machine.",
    what: "the client",
    removeText: (name: string) => `${name}: all access will be gone. To come back, the client will have to register and be approved again.`,
    rename: "Name the client",
    renameTitle: "Client's name",
    renameHint: "Seen only in the admin panel, to make users easier to identify.",
    save: "Save",
    cancel: "Cancel",
    clearField: "Clear: the client's own name is shown again",
    nameLong: "The name is too long",
  },
  {
    clients: "Клиенты",
    none: "Клиентов пока нет. Они появятся здесь, когда кто-нибудь добавит машину в Lumen Client.",
    sessionWith: (machine: string) => `Сессия с ${machine}`,
    onlineNoSession: "В сети, сессии нет",
    offlineSeen: (ago: string) => `Не в сети, был ${ago}`,
    settings: "Настройки",
    fps: "Частота",
    clientChooses: "Клиент выбирает сам",
    asEveryone: (what: string) => `Как у всех: ${what}`,
    cap: "Потолок битрейта",
    files: "Передача файлов",
    filesLabel: "Файлы",
    allowed: "можно",
    notAllowed: "нельзя",
    allow: "Можно",
    forbid: "Нельзя",
    connections: "Подключения",
    unlinkAllTitle: "Отвязать от всех машин?",
    unlink: "Отвязать",
    unlinkAllText: (name: string, n: number) => `${name}: машин — ${n}. Идущая сессия прервётся, дальше — только новая заявка.`,
    unlinkAll: "Отвязать от всех",
    noAccess: "Нет доступа ни к одной машине.",
    what: "клиента",
    removeText: (name: string) => `${name}: все доступы пропадут. Чтобы вернуться, ему придётся зарегистрироваться и получить одобрение заново.`,
    rename: "Задать ник",
    renameTitle: "Ник клиента",
    renameHint: "Виден только в панели администратора для упрощения идентификации пользователей",
    save: "Сохранить",
    cancel: "Отмена",
    clearField: "Стереть: снова будет ник, который указал сам клиент",
    nameLong: "Слишком длинный ник",
  },
);

export function Clients({ s, act, query, onQuery }: { s: State; act: Act; query: string; onQuery: (q: string) => void }) {
  const t = T();
  const { ask, dialog } = useConfirm();
  const { isOpen, toggle } = useOpen();
  const machines = (c: Client) => s.access.filter((a) => a.client === c.id).map((a) => hostLabel(a));
  const names = (c: Client) => [c.name, c.admin_name ?? ""];
  const shown = alphabetically(s.clients, clientLabel).filter((c) => matches(query, ...names(c), ...machines(c)));
  // Found by a machine: open, or the reason would not be visible.
  const foundInside = (c: Client) => !!query.trim() && !matches(query, ...names(c));
  return (
    <ListPage icon="person" title={t.clients} total={s.clients.length} shown={shown.length} none={t.none} query={query} onQuery={onQuery} dialog={dialog}>
      {shown.map((c) => (
        <ClientEntry key={c.id} s={s} c={c} open={isOpen(c.id) || foundInside(c)} onToggle={() => toggle(c.id)} act={act} ask={ask} />
      ))}
    </ListPage>
  );
}

function ClientEntry({ s, c, open, onToggle, act, ask }: { s: State; c: Client; open: boolean; onToggle: () => void; act: Act; ask: Ask }) {
  const mine = alphabetically(
    s.access.filter((a) => a.client === c.id),
    hostLabel,
  );
  const t = T();
  const approved = mine.filter((a) => a.status === "approved").length;
  const session = s.sessions.find((x) => x.client === c.id);
  const on = session && s.hosts.find((h) => h.id === session.host);
  const now = session
    ? `${t.sessionWith(on ? hostLabel(on) : `#${session.host}`)} · ${fps(session.fps)} · ${cap(session.mbps)} · ${hoursMinutes(session.seconds)}`
    : c.online
      ? t.onlineNoSession
      : t.offlineSeen(ago(s.now, c.last_seen));
  const g = s.global_override;
  const setLimits = (l: Limits) => act(() => api.override(c.id, l));
  const label = clientLabel(c);
  // Something of its own, not as everyone: the gear on its icon says so before the card is opened.
  const own = c.override.fps !== null || c.override.mbps !== null || c.files !== null;
  const [naming, setNaming] = useState(false);
  const tools = (
    <button className="btn small quiet icon-only" title={t.rename} aria-label={t.rename} onClick={() => setNaming(true)}>
      <Icon name="edit" size={16} />
    </button>
  );
  return (
    <Block icon={own ? "personSettings" : "person"} name={label} session={!!session} online={c.online} open={open} onToggle={onToggle} tools={tools}>
      {naming && <NameDialog c={c} onClose={() => setNaming(false)} onSave={(name) => act(() => api.name(c.id, name))} />}
      <p className="entry-now selectable">{now}</p>
      <h4 className="section-title">{t.settings}</h4>
      <div className="panel fields">
        <label>
          <span>{t.fps}</span>
          <FpsSelect s={s} value={c.override} onChange={setLimits} off={g.fps === null ? t.clientChooses : t.asEveryone(fps(g.fps))} />
        </label>
        <label>
          <span>{t.cap}</span>
          <CapSelect s={s} value={c.override} onChange={setLimits} off={g.mbps === null ? t.clientChooses : t.asEveryone(cap(g.mbps))} />
        </label>
        <label>
          <span>{t.files}</span>
          <select
            aria-label={t.filesLabel}
            className={c.files === null ? "" : "own"}
            value={c.files === null ? "" : c.files ? "on" : "off"}
            onChange={(e) => act(() => api.files(c.id, e.target.value === "" ? null : e.target.value === "on"))}
          >
            <option value="">{t.asEveryone(s.files.allowed ? t.allowed : t.notAllowed)}</option>
            <option value="on">{t.allow}</option>
            <option value="off">{t.forbid}</option>
          </select>
        </label>
      </div>
      <h4 className="section-title">
        {t.connections}
        {approved >= 2 && (
          <button
            className="btn small quiet risky wide-text"
            onClick={() =>
              ask({
                title: t.unlinkAllTitle,
                action: t.unlink,
                text: t.unlinkAllText(label, approved),
                run: () => act(() => api.unlinkAll(c.id)),
              })
            }
          >
            {t.unlinkAll}
          </button>
        )}
      </h4>
      <ul className="panel sub">
        {mine.length === 0 ? <li className="sub-empty">{t.noAccess}</li> : mine.map((a) => <AccessRow key={a.host} s={s} a={a} side="client" act={act} ask={ask} />)}
      </ul>
      <EntryFoot version={c.version} id={c.id} what={t.what} text={t.removeText(label)} act={act} ask={ask} />
    </Block>
  );
}

/** The admin's name for a client: saved as typed (trimmed); empty (the cross in the field) clears it. */
function NameDialog({ c, onClose, onSave }: { c: Client; onClose: () => void; onSave: (name: string | null) => void }) {
  const t = T();
  const [name, setName] = useState(c.admin_name ?? "");
  const input = useRef<HTMLInputElement>(null);
  const problem = nameProblem(name, t.nameLong);
  const save = () => {
    if (problem) return;
    onSave(name.trim() || null);
    onClose();
  };
  return (
    <Dialog
      title={t.renameTitle}
      onClose={onClose}
      footer={
        <>
          <button className="btn plain" onClick={onClose}>
            {t.cancel}
          </button>
          <button className="btn primary" disabled={!!problem} onClick={save}>
            {t.save}
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <label className="field">
          <span>{t.renameTitle}</span>
          <span className="in-field">
            <input ref={input} type="text" value={name} placeholder={c.name} onChange={(e) => setName(e.target.value)} />
            {name && (
              <button
                type="button"
                className="in-field-button"
                title={t.clearField}
                aria-label={t.clearField}
                onClick={(e) => {
                  e.preventDefault();
                  setName("");
                  input.current?.focus();
                }}
              >
                <Icon name="close" size={16} />
              </button>
            )}
          </span>
        </label>
        {problem && <p className="dialog-error">{problem}</p>}
        <button type="submit" hidden />
      </form>
      <p>{t.renameHint}</p>
    </Dialog>
  );
}
