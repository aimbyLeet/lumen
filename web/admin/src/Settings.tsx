// What holds for everyone: rows, as on the settings tabs of Lumen Client and Lumen Host. Applied at once.
import { useState } from "react";
import { words } from "../../client/src/i18n";
import { Icon, Setting, Toggle } from "../../client/src/ui";
import { api, type Limits, type State } from "./api";
import { CapSelect, FpsSelect, type Act } from "./common";

const T = words(
  {
    count: (n: number) => `${n} ${n === 1 ? "client" : "clients"}`,
    settings: "Settings",
    fps: "Frame rate for everyone",
    clientChooses: "The client chooses",
    ownFps: (who: string) => `Own for ${who}`,
    fpsText: "If set, the client cannot change it. The client's own setting comes first. Takes effect from the next connection.",
    cap: "Bitrate cap for everyone",
    ownCap: (who: string) => `Own for ${who}`,
    capText: "If set, the client cannot change it. The client's own setting comes first. Auto — by the server's table. Takes effect from the next connection.",
    files: "File transfer",
    ownFiles: (who: string) => `Own switch for ${who}`,
    filesText: "Files and folders through the clipboard, both ways. Turning it off takes effect at once, transfers in progress are cut off. Text and pictures always work.",
    speed: "File speed cap",
    speedText: "Mbit/s, for each side of a session: 1 to 1000.",
    speedLabel: "File speed cap, Mbit/s",
  },
  {
    count: (n: number) => {
      const t = n % 100;
      const u = n % 10;
      const word = t >= 11 && t <= 14 ? "клиентов" : u === 1 ? "клиента" : "клиентов";
      return `${n} ${word}`;
    },
    settings: "Настройки",
    fps: "Частота для всех",
    clientChooses: "Клиент выбирает сам",
    ownFps: (who: string) => `Своя у ${who}`,
    fpsText: "Если задана, клиент не может менять её сам. Настройка клиента главнее. Действует со следующего подключения.",
    cap: "Потолок битрейта для всех",
    ownCap: (who: string) => `Свой у ${who}`,
    capText: "Если задан, клиент не может менять его сам. Настройка клиента главнее. Авто — по таблице сервера. Действует со следующего подключения.",
    files: "Передача файлов",
    ownFiles: (who: string) => `Свой выключатель у ${who}`,
    filesText: "Файлы и папки через буфер обмена, в обе стороны. Выключение действует сразу, идущие передачи прерываются. Текст и картинки работают всегда.",
    speed: "Потолок скорости файлов",
    speedText: "Мбит/с, для каждой стороны сессии: от 1 до 1000.",
    speedLabel: "Потолок скорости файлов, Мбит/с",
  },
);

export function Settings({ s, act }: { s: State; act: Act }) {
  const t = T();
  const ownFps = s.clients.filter((c) => c.override.fps !== null).length;
  const ownCap = s.clients.filter((c) => c.override.mbps !== null).length;
  const limits = s.global_override;
  const ownFiles = s.clients.filter((c) => c.files !== null).length;
  const setLimits = (l: Limits) => act(() => api.override(null, l));
  return (
    <div className="page narrow">
      <div className="admin-head">
        <Icon name="settings" size={18} />
        <h2>{t.settings}</h2>
      </div>
      <ul className="rows">
        <Setting
          title={t.fps}
          control={
            <div className="stack">
              <FpsSelect s={s} value={limits} onChange={setLimits} off={t.clientChooses} />
              {ownFps > 0 && <span className="own-note">{t.ownFps(t.count(ownFps))}</span>}
            </div>
          }
        >
          <p>{t.fpsText}</p>
        </Setting>
        <Setting
          title={t.cap}
          control={
            <div className="stack">
              <CapSelect s={s} value={limits} onChange={setLimits} off={t.clientChooses} />
              {ownCap > 0 && <span className="own-note">{t.ownCap(t.count(ownCap))}</span>}
            </div>
          }
        >
          <p>{t.capText}</p>
        </Setting>
        <Setting
          title={t.files}
          control={
            <div className="stack center">
              <Toggle label={t.files} checked={s.files.allowed} onChange={(v) => act(() => api.files(null, v))} />
              {ownFiles > 0 && <span className="own-note">{t.ownFiles(t.count(ownFiles))}</span>}
            </div>
          }
        >
          <p>{t.filesText}</p>
        </Setting>
        <Setting title={t.speed} control={<Speed key={s.files.mbps} mbps={s.files.mbps} act={act} />}>
          <p>{t.speedText}</p>
        </Setting>
      </ul>
    </div>
  );
}

function Speed({ mbps, act }: { mbps: number; act: Act }) {
  const [text, setText] = useState(String(mbps));
  const value = Number(text);
  const valid = Number.isInteger(value) && value >= 1 && value <= 1000;
  const apply = () => {
    if (!valid) setText(String(mbps));
    else if (value !== mbps) act(() => api.filesSpeed(value));
  };
  return (
    <input
      type="text"
      inputMode="numeric"
      aria-label={T().speedLabel}
      className={valid ? "" : "invalid"}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={apply}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
    />
  );
}
