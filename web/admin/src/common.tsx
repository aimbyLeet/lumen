// What the admin's tabs share: words for time and quality, one access in a nested list, the search box, the question before a loss.
import { useCallback, useState, type ReactNode } from "react";
import { words } from "../../client/src/i18n";
import { Confirm, Icon, Pill, type IconName } from "../../client/src/ui";
import { fps, hoursMinutes, mbps, U } from "../../client/src/units";
import { accessId, api, clientLabelOf, hostLabel, type Access, type Limits, type State } from "./api";

const W = words(
  {
    never: "never",
    justNow: "just now",
    minAgo: (n: number) => `${n} min ago`,
    hAgo: (n: number) => `${n} h ago`,
    dAgo: (n: number) => `${n} d ago`,
    auto: "auto",
    fps: "Frame rate",
    cap: "Bitrate cap",
    session: "Session",
    online: "Online",
    offline: "Offline",
    request: "Request",
    requested: (ago: string) => `request ${ago}`,
    rejected: "Rejected",
    rejectedAgo: (ago: string) => `rejected ${ago}`,
    unban: "Lift the ban",
    unbanHint: "The client can request access again",
    busy: "Busy",
    busyHint: (who: string) => `Connected: ${who}`,
    version: "Version",
    versionHint: (client: string, host: string) => `The versions differ: client ${client}, machine ${host}. A connection is not possible.`,
    approvedAgo: (ago: string) => `approved ${ago}`,
    unlinkTitle: "Unlink from the machine?",
    unlink: "Unlink",
    unlinkText: (client: string, machine: string, session: boolean) =>
      `${client} → ${machine}.${session ? " The session in progress will be cut off." : ""} After that — only a new request.`,
    nothingFound: "Nothing found.",
    remove: "Remove",
    removeWhat: (what: string) => `Remove ${what}`,
    removeWhatTitle: (what: string) => `Remove ${what}?`,
    search: "Find a client or a machine",
  },
  {
    never: "никогда",
    justNow: "только что",
    minAgo: (n: number) => `${n} мин назад`,
    hAgo: (n: number) => `${n} ч назад`,
    dAgo: (n: number) => `${n} д назад`,
    auto: "авто",
    fps: "Частота",
    cap: "Потолок битрейта",
    session: "Сессия",
    online: "В сети",
    offline: "Не в сети",
    request: "Заявка",
    requested: (ago: string) => `заявка ${ago}`,
    rejected: "Отклонена",
    rejectedAgo: (ago: string) => `отклонена ${ago}`,
    unban: "Снять запрет",
    unbanHint: "Клиент сможет подать заявку снова",
    busy: "Занята",
    busyHint: (who: string) => `Подключён ${who}`,
    version: "Версия",
    versionHint: (client: string, host: string) => `Версии не совпадают: клиент ${client}, машина ${host}. Подключиться нельзя.`,
    approvedAgo: (ago: string) => `одобрено ${ago}`,
    unlinkTitle: "Отвязать от машины?",
    unlink: "Отвязать",
    unlinkText: (client: string, machine: string, session: boolean) =>
      `${client} → ${machine}.${session ? " Идущая сессия прервётся." : ""} Дальше — только новая заявка.`,
    nothingFound: "Ничего не найдено.",
    remove: "Удалить",
    removeWhat: (what: string) => `Удалить ${what}`,
    removeWhatTitle: (what: string) => `Удалить ${what}?`,
    search: "Найти клиента или машину",
  },
);

/** Runs an action, then shows the new state (or the error). */
export type Act = (f: () => Promise<unknown>) => Promise<void>;

export function ago(now: number, t: number | null): string {
  const w = W();
  if (!t) return w.never;
  const s = Math.max(0, now - t);
  if (s < 60) return w.justNow;
  if (s < 3600) return w.minAgo(Math.floor(s / 60));
  if (s < 86400) return w.hAgo(Math.floor(s / 3600));
  return w.dAgo(Math.floor(s / 86400));
}

/** A session's cap in a line of text: "auto" in lower case there, unlike the lists' "Auto". */
export const cap = (n: number) => (n > 0 ? mbps(n) : W().auto);

/** The two versions differ, and both are here now: such a connection would be refused. */
function versionsClash(a: { online: boolean; version: string }, b: { online: boolean; version: string }): boolean {
  return a.online && b.online && !!a.version && !!b.version && a.version !== b.version;
}

export function matches(query: string, ...names: string[]): boolean {
  const q = query.trim().toLocaleLowerCase();
  return !q || names.some((n) => n.toLocaleLowerCase().includes(q));
}

/** By name, as a person reads them: WS-2 before WS-10. */
export function alphabetically<T>(list: T[], name: (x: T) => string): T[] {
  return [...list].sort((a, b) => name(a).localeCompare(name(b), "ru", { numeric: true, sensitivity: "base" }));
}

/** `off`: what a list says while its limit is not set here. */
type LimitList = { s: State; value: Limits; onChange: (l: Limits) => void; off: string };

export function FpsSelect({ s, value, onChange, off }: LimitList) {
  return (
    <select
      aria-label={W().fps}
      className={value.fps === null ? "" : "own"}
      value={value.fps === null ? "off" : String(value.fps)}
      onChange={(e) => onChange({ ...value, fps: e.target.value === "off" ? null : Number(e.target.value) })}
    >
      <option value="off">{off}</option>
      {s.fps_choices.map((f) => (
        <option key={f} value={f}>
          {fps(f)}
        </option>
      ))}
    </select>
  );
}

export function CapSelect({ s, value, onChange, off }: LimitList) {
  return (
    <select
      aria-label={W().cap}
      className={value.mbps === null ? "" : "own"}
      value={value.mbps === null ? "off" : String(value.mbps)}
      onChange={(e) => onChange({ ...value, mbps: e.target.value === "off" ? null : Number(e.target.value) })}
    >
      <option value="off">{off}</option>
      <option value="0">{U().auto}</option>
      {s.mbps_presets.map((m) => (
        <option key={m} value={m}>
          {mbps(m)}
        </option>
      ))}
    </select>
  );
}

/** Which blocks are open: survives the polls, every block on its own. */
export function useOpen(): { isOpen: (id: number) => boolean; toggle: (id: number) => void } {
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const toggle = useCallback((id: number) => {
    setOpen((was) => {
      const next = new Set(was);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }, []);
  return { isOpen: (id) => open.has(id), toggle };
}

/** A client or a machine: closed, only its name and status; `children` when opened. The name takes the status's colour. */
/** `tools`: buttons right after the name, only while the block is open (closed, the list stays clean). */
export function Block({ icon, name, session, online, open, onToggle, tools, children }: { icon: IconName; name: string; session: boolean; online: boolean; open: boolean; onToggle: () => void; tools?: ReactNode; children: ReactNode }) {
  const w = W();
  const [tone, word] = session ? ["session", w.session] : online ? ["ok", w.online] : ["", w.offline];
  return (
    <li className={`entry tone ${tone}${open ? " open" : ""}`}>
      {/* Not a <button>: the tools inside it are buttons. */}
      <div
        className="entry-head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <Icon name={icon} size={18} />
        <span className="entry-name">
          <h3 title={name}>{name}</h3>
          {open && tools && (
            <span className="entry-tools" onClick={(e) => e.stopPropagation()}>
              {tools}
            </span>
          )}
        </span>
        <span className="entry-status">
          <Pill tone={tone}>{word}</Pill>
        </span>
        <span className="entry-arrow">
          <Icon name="chevron" size={20} />
        </span>
      </div>
      {open && <div className="entry-body">{children}</div>}
    </li>
  );
}

function LinkRow({ icon, title, sub, tone, word, hint, action }: { icon: IconName; title: string; sub: string; tone: string; word: string; hint?: string; action?: ReactNode }) {
  return (
    <li className={`link tone ${tone}`}>
      <span className="link-icon">
        <Icon name={icon} size={16} />
      </span>
      <div className="link-title selectable" title={title}>
        {title}
      </div>
      <div className="link-sub selectable">{sub}</div>
      <div className="link-status" title={hint}>
        <Pill tone={tone}>{word}</Pill>
      </div>
      <div className="link-action">{action}</div>
    </li>
  );
}

/** One access in a nested list: the machine under a client (`side` "client"), the client under a machine. */
export function AccessRow({ s, a, side, act, ask }: { s: State; a: Access; side: "client" | "machine"; act: Act; ask: Ask }) {
  const w = W();
  const machine = hostLabel(a);
  const icon: IconName = side === "client" ? "desktop" : "person";
  const who = clientLabelOf(s, a.client, a.client_name);
  const title = side === "client" ? machine : who;
  const id = `ID ${accessId(a)}`;
  if (a.status === "pending") {
    return <LinkRow icon={icon} title={title} sub={`${id} · ${w.requested(ago(s.now, a.requested))}`} tone="wait" word={w.request} />;
  }
  if (a.status === "rejected") {
    return (
      <LinkRow
        icon={icon}
        title={title}
        sub={`${id} · ${w.rejectedAgo(ago(s.now, a.decided))}`}
        tone="bad"
        word={w.rejected}
        action={
          <button className="btn small quiet" title={w.unbanHint} onClick={() => act(() => api.unlink(a.client, a.host))}>
            {w.unban}
          </button>
        }
      />
    );
  }
  const client = s.clients.find((c) => c.id === a.client);
  const host = s.hosts.find((h) => h.id === a.host);
  const session = s.sessions.find((x) => x.host === a.host);
  const mine = session?.client === a.client ? session : undefined;
  const other = side === "client" ? host : client;
  const [tone, word, hint]: [string, string, string?] = mine
    ? ["session", w.session]
    : session && side === "client"
      ? ["wait", w.busy, w.busyHint(clientLabelOf(s, session.client, session.client_name))]
      : client && host && versionsClash(client, host)
        ? ["bad", w.version, w.versionHint(client.version, host.version)]
        : other?.online
          ? ["ok", w.online]
          : ["", w.offline];
  const sub = mine ? `${fps(mine.fps)} · ${cap(mine.mbps)} · ${hoursMinutes(mine.seconds)}` : `${id} · ${w.approvedAgo(ago(s.now, a.decided))}`;
  return (
    <LinkRow
      icon={icon}
      title={title}
      sub={sub}
      tone={tone}
      word={word}
      hint={hint}
      action={
        <button
          className="btn small quiet risky"
          onClick={() =>
            ask({
              title: w.unlinkTitle,
              action: w.unlink,
              text: w.unlinkText(who, machine, !!mine),
              run: () => act(() => api.unlink(a.client, a.host)),
            })
          }
        >
          {w.unlink}
        </button>
      }
    />
  );
}

/** A list of blocks with its head: a tab of clients or of machines. `shown`: how many the search left. */
export function ListPage({ icon, title, total, shown, none, query, onQuery, dialog, children }: { icon: IconName; title: string; total: number; shown: number; none: string; query: string; onQuery: (q: string) => void; dialog: ReactNode; children: ReactNode }) {
  return (
    <div className="page">
      <div className="admin-head">
        <Icon name={icon} size={18} />
        <h2>{title}</h2>
        <span className="count">{total}</span>
        <span className="grow" />
        {total > 0 && <Search value={query} onChange={onQuery} />}
      </div>
      {total === 0 ? <p className="empty">{none}</p> : shown === 0 ? <p className="empty">{W().nothingFound}</p> : <ul className="entries">{children}</ul>}
      {dialog}
    </div>
  );
}

/** The foot of a block: its version and ID, and deleting it. */
export function EntryFoot({ version, id, what, text, act, ask }: { version: string; id: number; what: string; text: string; act: Act; ask: Ask }) {
  const w = W();
  return (
    <footer className="entry-foot">
      <p className="entry-sub selectable">
        {version && `v${version} · `}ID {id}
      </p>
      <button
        className="btn small quiet risky"
        title={w.removeWhat(what)}
        onClick={() =>
          ask({
            title: w.removeWhatTitle(what),
            action: w.remove,
            text,
            run: () => act(() => api.remove(id)),
          })
        }
      >
        {w.remove}
      </button>
    </footer>
  );
}

/** One search for both lists: the same words, and what is typed stays when the tab changes. */
function Search({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const text = W().search;
  return (
    <label className="search">
      <Icon name="search" size={16} />
      <input type="text" value={value} placeholder={text} aria-label={text} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}

export type Ask = (q: Question) => void;
type Question = { title: string; action: string; text: ReactNode; run: () => void };

/** `ask` opens "Sure?"; `dialog` goes somewhere in the tab. */
export function useConfirm(): { ask: Ask; dialog: ReactNode } {
  const [q, setQ] = useState<Question | null>(null);
  return {
    ask: setQ,
    dialog: q && (
      <Confirm title={q.title} action={q.action} onClose={() => setQ(null)} onConfirm={q.run}>
        <p>{q.text}</p>
      </Confirm>
    ),
  };
}
