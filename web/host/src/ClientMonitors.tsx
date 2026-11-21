// «Monitors» on a host without a physical monitor, in session: the set of the client
// connected. All of its monitors are virtual, monitor 1 too: it stays at its place, the others stand next to it.
import { useState } from "react";
import { words } from "../../client/src/i18n";
import { Icon } from "../../client/src/ui";
import { ErrorBanner, useAct } from "./MonitorAct";
import { api, type ClientSet, type MonitorsInfo, type Virtual } from "./api";
import { itemsOf, looseRects, sameLayout, sameRect, settle, withAdded, withChanged, withRemoved } from "./layout";
import { MonitorDesk, type Mark } from "./MonitorDesk";
import { VirtualMonitorPanel } from "./MonitorPanel";
import { useDesk } from "./useDesk";

/** Monitors a session shows at most (vdd::layout::MAX_MONITORS): monitor 1 and two more. */
const MOST = 3;

const W = words(
  {
    monitors: "Monitors",
    choosing: (name: string) => `No physical monitor: ${name ? `the client ${name}` : "the connected client"} is choosing a monitor in Lumen Client.`,
    shown: (name: string) => `No physical monitor: the monitors of ${name ? `the client ${name}` : "the connected client"} are shown. Changes are kept for this client.`,
    parked: "No physical monitor, nobody connected: the monitors of the last Lumen client are shown, kept for its next connection. Changes are kept for it.",
    of: "of",
    applying: "Applying…",
    apply: "Apply",
    cancel: "Cancel",
  },
  {
    monitors: "Мониторы",
    choosing: (name: string) => `Физического монитора нет: ${name ? `клиент ${name}` : "подключённый клиент"} выбирает монитор в Lumen Client.`,
    shown: (name: string) => `Физического монитора нет: показаны мониторы ${name ? `клиента ${name}` : "подключённого клиента"}. Изменения сохраняются для этого клиента.`,
    parked: "Физического монитора нет, через Lumen никто не подключён: показаны мониторы последнего клиента Lumen, они ждут его следующего подключения. Изменения сохраняются для него.",
    of: "из",
    applying: "Применяю…",
    apply: "Применить",
    cancel: "Отменить",
  },
);

export function Head() {
  return (
    <header className="view-head">
      <Icon name="monitors" size={19} />
      <h2>{W().monitors}</h2>
    </header>
  );
}

/** Monitor 1 as a monitor of the desk: at (0, 0). */
const firstRect = (s: ClientSet) => [{ x: 0, y: 0, width: s.primary.width, height: s.primary.height, name: "" }];

const sameSet = (a: ClientSet, b: ClientSet) =>
  a.primary.width === b.primary.width && a.primary.height === b.primary.height && a.primary.scale === b.primary.scale && sameLayout(firstRect(a), a.virtuals, b.virtuals);

type Props = {
  info: MonitorsInfo;
  client: { key: string; set: ClientSet | null; parked: boolean };
  /** The client's name, as the host's status tells it. */
  name: string;
  serviceError: string | null;
  refresh: () => Promise<void>;
};

export function ClientMonitors({ info, client, name, serviceError, refresh }: Props) {
  if (!client.set) {
    return (
      <section className="view">
        <Head />
        <p className="banner">{W().choosing(name)}</p>
      </section>
    );
  }
  // A new client: what was moved for the previous one does not carry over.
  return (
    <ClientSetView key={client.key} info={info} clientKey={client.key} saved={client.set} parked={client.parked} name={name} serviceError={serviceError} refresh={refresh} />
  );
}

type ViewProps = {
  info: MonitorsInfo;
  clientKey: string;
  saved: ClientSet;
  /** Nobody in session: the last client's set, kept (on or off as Windows has it). */
  parked: boolean;
  name: string;
  serviceError: string | null;
  refresh: () => Promise<void>;
};

function ClientSetView({ info, clientKey, saved, parked, name, serviceError, refresh }: ViewProps) {
  /** The set as changed here, not applied yet. */
  const w = W();
  const [form, setForm] = useState<ClientSet | null>(null);
  const [selected, setSelected] = useState(0);
  const { error, busy, act } = useAct(refresh);

  const st = info.state;
  const off = !!serviceError;
  const disabled = off || !!busy;
  const set = form ?? saved;
  const real = firstRect(set);
  const { virtuals } = set;
  const allowed = MOST - 1;
  const dirty = !sameSet(set, saved);
  const rects = looseRects(real, virtuals);
  const pick = Math.min(selected, rects.length - 1);

  const desk = useDesk({
    real,
    virtuals,
    rects,
    locked: disabled,
    onPick: setSelected,
    onDrop: (list, index) => {
      setForm({ ...set, virtuals: list });
      setSelected(1 + index);
    },
  });

  // Unchanged: the same size and scale in the same place as applied.
  const savedRects = looseRects(firstRect(saved), saved.virtuals);
  const firstPending = set.primary.width !== saved.primary.width || set.primary.height !== saved.primary.height || set.primary.scale !== saved.primary.scale;
  const marks: Mark[] = virtuals.map((v, i) => {
    const unchanged = i < saved.virtuals.length && sameRect(savedRects[1 + i], rects[1 + i]) && saved.virtuals[i].scale === v.scale;
    return { over: false, pending: !unchanged, asleep: false };
  });

  const apply = () =>
    act(
      w.applying,
      () => api.setClientMonitors(clientKey, set),
      () => setForm(null),
    );

  const add = () => {
    setForm({ ...set, virtuals: withAdded(rects, virtuals) });
    setSelected(rects.length);
  };
  const remove = (i: number) => {
    setForm({ ...set, virtuals: withRemoved(real, virtuals, i) });
    setSelected(i);
  };
  /** Monitor 1 resized: the others keep their neighbours; what no longer fits moves to the nearest snap point. */
  const changeFirst = (patch: Partial<ClientSet["primary"]>) => {
    const primary = { ...set.primary, ...patch };
    const next = firstRect({ ...set, primary });
    setForm({ primary, virtuals: settle(next, itemsOf(next, virtuals)).virtuals });
  };
  const change = (i: number, patch: Partial<Virtual>) => {
    const done = withChanged(real, virtuals, i, patch);
    setForm({ ...set, virtuals: done.virtuals });
    setSelected(done.picked);
  };

  const picked = pick - 1;
  return (
    <section className="view fill">
      <Head />
      <p className="banner">{parked ? w.parked : w.shown(name)}</p>
      <ErrorBanner off={off} error={error} reported={st.error} />
      <div className="mon-work">
        <MonitorDesk
          desk={desk}
          rects={rects}
          real={real}
          scales={virtuals.map((v) => v.scale)}
          marks={marks}
          pick={pick}
          on={!off && st.up.length > 0 && (!parked || info.lumen_on)}
          off={off}
          disabled={disabled}
          count={`${1 + virtuals.length} ${w.of} ${MOST}`}
          canAdd={virtuals.length < allowed}
          onAdd={add}
          onRemove={remove}
          first={{ scale: set.primary.scale, pending: firstPending }}
        />
        <div className="mon-side">
          {picked >= 0 ? (
            <VirtualMonitorPanel
              number={pick + 1}
              v={virtuals[picked]}
              attach={desk.attachOf(picked)}
              resolutions={info.catalog.resolutions}
              scales={info.catalog.scales}
              disabled={disabled}
              over={false}
              onChange={(patch) => change(picked, patch)}
            />
          ) : (
            <VirtualMonitorPanel
              number={1}
              v={set.primary}
              attach={null}
              resolutions={info.catalog.resolutions}
              scales={info.catalog.scales}
              disabled={disabled}
              over={false}
              onChange={changeFirst}
            />
          )}
          <div className="mon-actions">
            <button className="btn primary grow" disabled={disabled || !dirty} onClick={apply}>
              {busy || w.apply}
            </button>
            {dirty && (
              <button className="btn" disabled={disabled} onClick={() => setForm(null)}>
                {w.cancel}
              </button>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
