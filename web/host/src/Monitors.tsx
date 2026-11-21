import { Component, useState, type ReactNode } from "react";
import { words } from "../../client/src/i18n";
import { Confirm, Icon } from "../../client/src/ui";
import { ErrorBanner, useAct } from "./MonitorAct";
import { api, type MonitorsInfo, type Virtual } from "./api";
import { looseRects, sameLayout, sameRect, withAdded, withChanged, withRemoved } from "./layout";
import { MonitorDesk, type Mark } from "./MonitorDesk";
import { RealMonitorPanel, VirtualMonitorPanel } from "./MonitorPanel";
import { useDesk } from "./useDesk";
import { ClientMonitors, Head } from "./ClientMonitors";

const REAL_UNKNOWN = [{ x: 0, y: 0, width: 1920, height: 1080, name: "" }];

const W = words(
  {
    fault: "Section error:",
    showAgain: "Show again",
    of: "of",
    creating: "Creating…",
    create: "Create",
    enabling: "Turning on…",
    enable: "Turn on",
    applying: "Applying…",
    apply: "Apply",
    disabling: "Turning off…",
    disable: "Turn off",
    cancel: "Cancel",
    removeAll: "Remove all virtual monitors",
    removeAllTitle: "Remove all virtual monitors?",
    remove: "Remove",
    removing: "Removing…",
    removeAllText: "They will disappear, and their settings too.",
    noPhysical: "No physical monitor: the machine's virtual monitors are not made. Lumen Client gets its own monitors when it connects.",
  },
  {
    fault: "Ошибка раздела:",
    showAgain: "Показать снова",
    of: "из",
    creating: "Создаю…",
    create: "Создать",
    enabling: "Включаю…",
    enable: "Включить",
    applying: "Применяю…",
    apply: "Применить",
    disabling: "Выключаю…",
    disable: "Выключить",
    cancel: "Отменить",
    removeAll: "Удалить все виртуальные мониторы",
    removeAllTitle: "Удалить все виртуальные мониторы?",
    remove: "Удалить",
    removing: "Удаляю…",
    removeAllText: "Они пропадут, их настройки — тоже.",
    noPhysical: "Нет физического монитора: виртуальные мониторы машины не создаются. Lumen Client получит свои мониторы при подключении.",
  },
);

/** A fault in this section must not blank the whole window. */
export class MonitorsGuard extends Component<{ children: ReactNode }, { fault: string }> {
  state = { fault: "" };
  static getDerivedStateFromError(e: unknown) {
    return { fault: String(e) };
  }
  render() {
    if (!this.state.fault) return this.props.children;
    const w = W();
    return (
      <section className="view">
        <Head />
        <p className="banner bad selectable">
          {w.fault} {this.state.fault}
        </p>
        <button className="btn" onClick={() => this.setState({ fault: "" })}>
          {w.showAgain}
        </button>
      </section>
    );
  }
}

type Props = {
  info: MonitorsInfo;
  /** The client in session, as the host's status tells it. */
  clientName: string;
  serviceError: string | null;
  refresh: () => Promise<void>;
};

/** The machine's virtual monitors; without a physical monitor, in session — the client's own. */
export function Monitors({ info, clientName, serviceError, refresh }: Props) {
  const client = info.state.client;
  if (client) return <ClientMonitors info={info} client={client} name={clientName} serviceError={serviceError} refresh={refresh} />;
  return <MachineMonitors info={info} serviceError={serviceError} refresh={refresh} />;
}

function MachineMonitors({ info, serviceError, refresh }: Omit<Props, "clientName">) {
  const w = W();
  const st = info.state;
  const real = info.real.length > 0 ? info.real : REAL_UNKNOWN;
  const realCount = real.length;
  /** The monitors as moved here, not applied yet. */
  const [form, setForm] = useState<Virtual[] | null>(null);
  const [selected, setSelected] = useState(() => (st.settings.virtuals.length ? realCount : 0));
  const [confirmDelete, setConfirmDelete] = useState(false);
  const { error, busy, act: run } = useAct(refresh);

  const off = !!serviceError;
  // Without a physical monitor (another remote access on its own virtual
  // display): the service makes none of these and refuses changes.
  const noPhysical = !off && !st.physical;
  const disabled = off || noPhysical || !!busy;
  const { enabled, virtuals: saved } = st.settings;
  const isUp = st.up.length > 0;
  const virtuals = form ?? saved;
  const savedNone = saved.length === 0;
  const dirty = !sameLayout(real, virtuals, saved);
  const canApply = !disabled && virtuals.length <= info.allowed;
  const canAdd = !noPhysical && virtuals.length < info.allowed;
  const rects = looseRects(real, virtuals);
  const pick = Math.min(selected, rects.length - 1);

  const desk = useDesk({
    real,
    virtuals,
    rects,
    locked: disabled,
    onPick: setSelected,
    onDrop: (list, index) => {
      setForm(list);
      setSelected(realCount + index);
    },
  });

  const savedRects = looseRects(real, saved);
  const marks: Mark[] = virtuals.map((v, i) => {
    const over = i >= info.allowed;
    const unchanged = i < saved.length && sameRect(savedRects[realCount + i], rects[realCount + i]) && saved[i].scale === v.scale;
    // All of them are on or off together; one not created yet is not on either.
    return { over, pending: !over && !unchanged, asleep: !enabled || !isUp || i >= saved.length };
  });

  const act = (what: string, f: () => Promise<unknown>) => run(what, f, () => setForm(null));
  // All of them taken away: the same as deleting them.
  const apply = (what: string) => act(what, () => (virtuals.length === 0 ? api.resetMonitors() : api.setMonitors({ enabled: true, virtuals })));

  const add = () => {
    setForm(withAdded(rects, virtuals));
    setSelected(rects.length);
  };
  const remove = (i: number) => {
    setForm(withRemoved(real, virtuals, i));
    setSelected(Math.max(0, realCount + i - 1));
  };
  const change = (i: number, patch: Partial<Virtual>) => {
    const done = withChanged(real, virtuals, i, patch);
    setForm(done.virtuals);
    setSelected(done.picked);
  };

  const pickedVirtual = pick - realCount;
  return (
    <section className="view fill">
      <Head />
      {noPhysical && <p className="banner">{w.noPhysical}</p>}
      <ErrorBanner off={off} error={error} reported={st.error} />
      <div className="mon-work">
        <MonitorDesk
          desk={desk}
          rects={rects}
          real={real}
          scales={virtuals.map((v) => v.scale)}
          marks={marks}
          pick={pick}
          on={!off && enabled && isUp}
          off={off}
          disabled={disabled}
          count={`${info.shown + virtuals.length} ${w.of} ${info.shown + info.allowed}`}
          canAdd={canAdd}
          onAdd={add}
          onRemove={remove}
        />
        <div className="mon-side">
          {pickedVirtual >= 0 ? (
            <VirtualMonitorPanel
              number={pick + 1}
              v={virtuals[pickedVirtual]}
              attach={desk.attachOf(pickedVirtual)}
              resolutions={info.catalog.resolutions}
              scales={info.catalog.scales}
              disabled={disabled}
              over={marks[pickedVirtual].over}
              onChange={(patch) => change(pickedVirtual, patch)}
            />
          ) : (
            <RealMonitorPanel number={pick + 1} name={real[pick].name} width={real[pick].width} height={real[pick].height} />
          )}
          <div className="mon-actions">
            {savedNone ? (
              <button className="btn primary grow" disabled={!canApply || virtuals.length === 0} onClick={() => apply(w.creating)}>
                {busy || w.create}
              </button>
            ) : !enabled ? (
              <button className="btn primary grow" disabled={!canApply || virtuals.length === 0} onClick={() => apply(w.enabling)}>
                {busy || w.enable}
              </button>
            ) : dirty || st.error ? (
              <button className="btn primary grow" disabled={!canApply} onClick={() => apply(w.applying)}>
                {busy || w.apply}
              </button>
            ) : (
              <button className="btn grow" disabled={disabled} onClick={() => act(w.disabling, () => api.setMonitors({ enabled: false, virtuals: saved }))}>
                {busy || w.disable}
              </button>
            )}
            {dirty && !savedNone && (
              <button className="btn" disabled={disabled} onClick={() => setForm(null)}>
                {w.cancel}
              </button>
            )}
            <button
              className="btn square danger"
              title={w.removeAll}
              aria-label={w.removeAll}
              disabled={disabled || virtuals.length + saved.length === 0}
              onClick={() => setConfirmDelete(true)}
            >
              <Icon name="delete" />
            </button>
          </div>
        </div>
      </div>
      {confirmDelete && (
        <Confirm
          title={w.removeAllTitle}
          action={w.remove}
          onClose={() => setConfirmDelete(false)}
          onConfirm={() => {
            setSelected(0);
            if (savedNone) setForm(null);
            else act(w.removing, api.resetMonitors);
          }}
        >
          <p>{w.removeAllText}</p>
        </Confirm>
      )}
    </section>
  );
}
