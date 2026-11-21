// A host without monitors asks for ours: the resolution and scale of the virtual
// monitor it makes. Chosen once per machine — the host remembers; changed later in Lumen Host there.
import { useState, type ReactNode } from "react";
import type { MonitorChoice, Resolution } from "./api";
import { words } from "./i18n";
import { byAspect, Dialog, Icon } from "./ui";

type Question = { suggest: MonitorChoice; resolutions: Resolution[]; scales: number[] };

const key = (r: { width: number; height: number }) => `${r.width}x${r.height}`;

const W = words(
  {
    title: "The remote computer's monitor",
    disconnect: "Disconnect",
    create: "Create",
    question: (machine: ReactNode) => (
      <>{machine ?? "The remote computer"} has no monitors. It will create a virtual monitor — choose its resolution and scale.</>
    ),
    resolution: "Resolution",
    scale: "Scale",
    remembered:
      "The choice is remembered for this computer. The resolution can be changed and monitors added in Lumen Host on the remote computer.",
  },
  {
    title: "Монитор удалённого компьютера",
    disconnect: "Отключиться",
    create: "Создать",
    question: (machine: ReactNode) => (
      <>
        У {machine ?? "удалённого компьютера"} нет мониторов. Он создаст виртуальный монитор — выберите его разрешение и
        масштаб.
      </>
    ),
    resolution: "Разрешение",
    scale: "Масштаб",
    remembered:
      "Выбор запомнится для этого компьютера. Поменять разрешение и добавить мониторы можно в Lumen Host на удалённом компьютере.",
  },
);

export function MonitorDialog({
  machine,
  question,
  onCancel,
  onChoose,
}: {
  machine: string;
  question: Question;
  onCancel: () => void;
  onChoose: (m: MonitorChoice) => void;
}) {
  const w = W();
  const [size, setSize] = useState(key(question.suggest));
  const [scale, setScale] = useState(question.suggest.scale);
  const choose = () => {
    const [width, height] = size.split("x").map(Number);
    onChoose({ width, height, scale });
  };
  return (
    <Dialog
      title={w.title}
      onClose={onCancel}
      footer={
        <>
          <button className="btn plain" onClick={onCancel}>
            {w.disconnect}
          </button>
          <button className="btn primary dialog-default" onClick={choose}>
            {w.create}
          </button>
        </>
      }
    >
      <p>{w.question(machine ? <b>{machine}</b> : null)}</p>
      <div className="field-row">
        <label className="field">
          <span>{w.resolution}</span>
          <select aria-label={w.resolution} value={size} onChange={(e) => setSize(e.target.value)}>
            {byAspect(question.resolutions).map(([aspect, rs]) => (
              <optgroup key={aspect} label={aspect}>
                {rs.map((r) => (
                  <option key={key(r)} value={key(r)}>
                    {r.width} × {r.height}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </label>
        <label className="field">
          <span>{w.scale}</span>
          <select aria-label={w.scale} value={scale} onChange={(e) => setScale(Number(e.target.value))}>
            {question.scales.map((sc) => (
              <option key={sc} value={sc}>
                {sc} %
              </option>
            ))}
          </select>
        </label>
      </div>
      <p className="notice">
        <Icon name="devices" size={16} />
        {w.remembered}
      </p>
    </Dialog>
  );
}
