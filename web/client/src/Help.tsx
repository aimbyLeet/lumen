import { words } from "./i18n";
import { Icon, Keys } from "./ui";

const W = words(
  {
    title: "Keyboard shortcuts",
    shortcuts: [
      ["Video window with or without a frame", "Over the whole monitor or in a window. With a window per host monitor: all at once, each on its own monitor.", "Ctrl+Alt+Shift+F"],
      ["Close the video window", "Ends the session and gives the keyboard and mouse back to this computer. The host then locks, unless that is turned off in the settings.", "Ctrl+Alt+Shift+Q"],
      ["Ctrl+Alt+Del on the remote machine", "The Windows security screen on the host. A plain Ctrl+Alt+Del stays on this computer.", "Ctrl+Alt+Shift+Del"],
      ["How monitors are shown", "If the host has 2–3 monitors: all in one window, one monitor in a window with a switcher, a window per monitor.", "Ctrl+Alt+Shift+L"],
      ["Connection status", "An overlay over the picture: the state, latency, video parameters.", "Ctrl+Alt+Shift+O"],
      ["File transfers", "Show or hide the list of this session's transfers.", "Ctrl+Alt+Shift+D"],
    ] as [string, string, string][],
  },
  {
    title: "Горячие клавиши",
    shortcuts: [
      ["Окно видео с рамкой или без", "На весь монитор или в окне. Если окно на каждый монитор хоста: все сразу, каждое на своём мониторе.", "Ctrl+Alt+Shift+F"],
      ["Закрыть окно видео", "Завершает сессию и возвращает клавиатуру и мышь этому компьютеру. Хост после этого блокируется, если это не отключено в настройках.", "Ctrl+Alt+Shift+Q"],
      ["Ctrl+Alt+Del на удалённой машине", "Экран безопасности Windows на хосте. Обычный Ctrl+Alt+Del остаётся на этом компьютере.", "Ctrl+Alt+Shift+Del"],
      ["Режим показа мониторов", "Если у хоста 2–3 монитора: все в одном окне, один монитор в окне с переключателем, окно на каждый монитор.", "Ctrl+Alt+Shift+L"],
      ["Состояние связи", "Оверлей поверх картинки: состояние, задержка, параметры видео.", "Ctrl+Alt+Shift+O"],
      ["Передачи файлов", "Показать или спрятать список передач за эту сессию.", "Ctrl+Alt+Shift+D"],
    ],
  },
);

export function Help() {
  const w = W();
  return (
    <section className="view">
      <header className="view-head">
        <Icon name="help" size={19} />
        <h2>{w.title}</h2>
      </header>
      <ul className="rows">
        {w.shortcuts.map(([title, text, combo]) => (
          <li className="setting" key={combo}>
            <div className="setting-text">
              <h3>{title}</h3>
              <p>{text}</p>
            </div>
            <div className="setting-control">
              <Keys combo={combo} />
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
