// Small pieces every screen of both apps uses: Lumen Host takes them from here as they are.
// Icons: Material Symbols (Apache-2.0), only the paths in use, so the apps need no icon font.
import { useEffect, useRef, useState, type ReactNode } from "react";
import logo from "./assets/lumen.png";
import { lang, LANGS, words, type Lang } from "./i18n";

const W = words(
  {
    minimize: "Minimize",
    restore: "Restore",
    maximize: "Maximize",
    close: "Close",
    cancel: "Cancel",
    holdToSee: "Hold to see the token",
    showToken: "Show the token",
    log: "Log",
    openFolder: "Open folder",
    logText: "Useful if something went wrong.",
    nameLong: "The name is too long",
    nameLatin: "Latin letters, digits and signs only",
    tagline: "Remote access to work machines",
  },
  {
    minimize: "Свернуть",
    restore: "Восстановить",
    maximize: "Развернуть",
    close: "Закрыть",
    cancel: "Отмена",
    holdToSee: "Удерживайте, чтобы увидеть токен",
    showToken: "Показать токен",
    log: "Журнал",
    openFolder: "Открыть папку",
    logText: "Пригодится, если что-то пошло не так.",
    nameLong: "Слишком длинное имя",
    nameLatin: "Только латинские буквы, цифры и знаки",
    tagline: "Удалённый доступ к рабочим машинам",
  },
);

const PATHS = {
  add: "M450-450H200v-60h250v-250h60v250h250v60H510v250h-60v-250Z",
  chevron: "M480-345 240-585l56-56 184 184 184-184 56 56-240 240Z",
  close: "m249-207-42-42 231-231-231-231 42-42 231 231 231-231 42 42-231 231 231 231-42 42-231-231-231 231Z",
  copy: "M300-200q-24 0-42-18t-18-42v-560q0-24 18-42t42-18h440q24 0 42 18t18 42v560q0 24-18 42t-42 18H300Zm0-60h440v-560H300v560ZM180-80q-24 0-42-18t-18-42v-620h60v620h500v60H180Zm120-180v-560 560Z",
  delete:
    "M261-120q-24.75 0-42.37-17.63Q201-155.25 201-180v-570h-41v-60h188v-30h264v30h188v60h-41v570q0 24-18 42t-42 18H261Zm438-630H261v570h438v-570ZM367-266h60v-399h-60v399Zm166 0h60v-399h-60v399ZM261-750v570-570Z",
  desktop:
    "M334-120v-60h86v-100H140q-24 0-42-18t-18-42v-440q0-24 18-42t42-18h680q24 0 42 18t18 42v440q0 24-18 42t-42 18H540v100h86v60H334ZM140-340h680v-440H140v440Zm0 0v-440 440Z",
  devices:
    "M501-540ZM80-160v-60h421v60H80Zm100-120q-24 0-42-18t-18-42v-400q0-24 18-42t42-18h600q24 0 42 18t18 42H180v400h321v60H180Zm640 60v-400H621v400h199Zm-214 60q-18 0-31.5-13.5T561-205v-430q0-18 13.5-31.5T606-680h229q18 0 31.5 13.5T880-635v430q0 18-13.5 31.5T835-160H606Zm115-360q12 0 21-9t9-21q0-12-9-21t-21.48-9Q709-580 700-571t-9 21.48q0 11.52 9 20.52t21 9Zm0 100Z",
  download: "M480-320 280-520l56-58 104 104v-326h80v326l104-104 56 58-200 200ZM240-160q-33 0-56.5-23.5T160-240v-120h80v120h480v-120h80v120q0 33-23.5 56.5T720-160H240Z",
  edit: "M200-200h57l391-391-57-57-391 391v57Zm-80 80v-170l528-527q12-11 26.5-17t30.5-6q16 0 31 6t26 18l55 56q12 11 17.5 26t5.5 30q0 16-5.5 30.5T817-647L290-120H120Zm640-584-56-56 56 56Zm-141 85-28-29 57 57-29-28Z",
  folder:
    "M140-160q-23 0-41.5-18.5T80-220v-520q0-23 18.5-41.5T140-800h281l60 60h339q23 0 41.5 18.5T880-680H455l-60-60H140v520l102-400h698L833-206q-6 24-22 35t-41 11H140Zm63-60h572l84-340H287l-84 340Zm0 0 84-340-84 340Zm-63-460v-60 60Z",
  help: "M511-258q11-11 11-27t-11-27q-11-11-27-11t-27 11q-11 11-11 27t11 27q11 11 27 11t27-11Zm-62-135h59q0-26 6.5-47.5T555-490q31-26 44-51t13-55q0-53-34.5-85T486-713q-49 0-86.5 24.5T345-621l53 20q11-28 33-43.5t52-15.5q34 0 55 18.5t21 47.5q0 22-13 41.5T508-512q-30 26-44.5 51.5T449-393Zm31 313q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-83 31.5-156t86-127Q252-817 325-848.5T480-880q83 0 156 31.5T763-763q54 54 85.5 127T880-480q0 82-31.5 155T763-197.5q-54 54.5-127 86T480-80Zm0-60q142 0 241-99.5T820-480q0-142-99-241t-241-99q-141 0-240.5 99T140-480q0 141 99.5 240.5T480-140Zm0-340Z",
  key: "M232-432q-20-20-20-48t20-48q20-20 48-20t48 20q20 20 20 48t-20 48q-20 20-48 20t-48-20Zm48 192q-100 0-170-70T40-480q0-100 70-170t170-70q72 0 126 34t85 103h356l113 113-167 153-88-64-88 64-75-60h-51q-25 60-78.5 98.5T280-240Zm0-60q58 0 107-38.5t63-98.5h114l54 45 88-63 82 62 85-79-51-51H450q-12-56-60-96.5T280-660q-75 0-127.5 52.5T100-480q0 75 52.5 127.5T280-300Z",
  lock: "M220-80q-24.75 0-42.37-17.63Q160-115.25 160-140v-434q0-24.75 17.63-42.38Q195.25-634 220-634h70v-96q0-78.85 55.61-134.42Q401.21-920 480.11-920q78.89 0 134.39 55.58Q670-808.85 670-730v96h70q24.75 0 42.38 17.62Q800-598.75 800-574v434q0 24.75-17.62 42.37Q764.75-80 740-80H220Zm0-60h520v-434H220v434Zm314.5-162.03Q557-324.06 557-355q0-30-22.67-54.5t-54.5-24.5q-31.83 0-54.33 24.5t-22.5 55q0 30.5 22.67 52.5t54.5 22q31.83 0 54.33-22.03ZM350-634h260v-96q0-54.17-37.88-92.08-37.88-37.92-92-37.92T388-822.08q-38 37.91-38 92.08v96ZM220-140v-434 434Z",
  logout: "M200-120q-33 0-56.5-23.5T120-200v-560q0-33 23.5-56.5T200-840h280v80H200v560h280v80H200Zm440-160-55-58 102-102H360v-80h327L585-622l55-58 200 200-200 200Z",
  /** Material's more_horiz: a card's options (restart, remove). */
  more: "M240-400q-33 0-56.5-23.5T160-480q0-33 23.5-56.5T240-560q33 0 56.5 23.5T320-480q0 33-23.5 56.5T240-400Zm240 0q-33 0-56.5-23.5T400-480q0-33 23.5-56.5T480-560q33 0 56.5 23.5T560-480q0 33-23.5 56.5T480-400Zm240 0q-33 0-56.5-23.5T640-480q0-33 23.5-56.5T720-560q33 0 56.5 23.5T800-480q0 33-23.5 56.5T720-400Z",
  monitors:
    "M80-327v-493q0-24.75 17.63-42.38Q115.25-880 140-880h574v60H140v493H80ZM448-80v-80H300q-24 0-42-18t-18-42v-440q0-24 18-42t42-18h560q24 0 42 18t18 42v440q0 24-18 42t-42 18H712v80H448ZM300-220h560v-440H300v440Zm280-220Z",
  person:
    "M480-480q-66 0-113-47t-47-113q0-66 47-113t113-47q66 0 113 47t47 113q0 66-47 113t-113 47ZM160-160v-112q0-34 17.5-62.5T224-378q62-31 126-46.5T480-440q66 0 130 15.5T736-378q29 15 46.5 43.5T800-272v112H160Zm80-80h480v-32q0-11-5.5-20T700-306q-54-27-109-40.5T480-360q-56 0-111 13.5T260-306q-9 5-14.5 14t-5.5 20v32Zm240-320q33 0 56.5-23.5T560-640q0-33-23.5-56.5T480-720q-33 0-56.5 23.5T400-640q0 33 23.5 56.5T480-560Zm0-80Zm0 400Z",
  /** Material's manage_accounts: a client the admin has set something of its own for. */
  personSettings:
    "M287-527q-47-47-47-113t47-113q47-47 113-47t113 47q47 47 47 113t-47 113q-47 47-113 47t-113-47ZM80-160v-112q0-33 17-62t47-44q51-26 115-44t141-18h14q6 0 12 2-8 18-13.5 37.5T404-360h-4q-71 0-127.5 18T180-306q-9 5-14.5 14t-5.5 20v32h252q6 21 16 41.5t22 38.5H80Zm560 40-12-60q-12-5-22.5-10.5T584-204l-58 18-40-68 46-40q-2-14-2-26t2-26l-46-40 40-68 58 18q11-8 21.5-13.5T628-460l12-60h80l12 60q12 5 22.5 11t21.5 15l58-20 40 70-46 40q2 12 2 25t-2 25l46 40-40 68-58-18q-11 8-21.5 13.5T732-180l-12 60h-80Zm96.5-143.5Q760-287 760-320t-23.5-56.5Q713-400 680-400t-56.5 23.5Q600-353 600-320t23.5 56.5Q647-240 680-240t56.5-23.5Zm-280-320Q480-607 480-640t-23.5-56.5Q433-720 400-720t-56.5 23.5Q320-673 320-640t23.5 56.5Q367-560 400-560t56.5-23.5ZM400-640Zm12 400Z",
  play: "M320-203v-560l440 280-440 280Zm60-280Zm0 171 269-171-269-171v342Z",
  power:
    "M480-80q-82 0-155-31.5t-127.5-86Q143-252 111.5-325T80-480q0-84 32-156.5T198-763l42 42q-47 46-73.5 108T140-480q0 141 99.5 240.5T480-140q142 0 241-99.5T820-480q0-71-26-133t-73-108l42-42q54 54 85.5 126.5T880-480q0 82-31.5 155T763-197.5q-54 54.5-127 86T480-80Zm-30-360v-440h60v440h-60Z",
  refresh:
    "M480-160q-134 0-227-93t-93-227q0-134 93-227t227-93q69 0 132 28.5T720-690v-110h80v280H520v-80h168q-32-56-87.5-88T480-720q-100 0-170 70t-70 170q0 100 70 170t170 70q77 0 139-44t87-116h84q-28 106-114 173t-196 67Z",
  search:
    "M784-120 532-372q-30 24-69 38t-83 14q-109 0-184.5-75.5T120-580q0-109 75.5-184.5T380-840q109 0 184.5 75.5T640-580q0 44-14 83t-38 69l252 252-56 56ZM380-400q75 0 127.5-52.5T560-580q0-75-52.5-127.5T380-760q-75 0-127.5 52.5T200-580q0 75 52.5 127.5T380-400Z",
  server:
    "M170-120q-21 0-35.5-14.5T120-170v-200q0-21 14.5-35.5T170-420h620q21 0 35.5 14.5T840-370v200q0 21-14.5 35.5T790-120H170Zm10-60h600v-180H180v180Zm-10-360q-21 0-35.5-14.5T120-590v-200q0-21 14.5-35.5T170-840h620q21 0 35.5 14.5T840-790v200q0 21-14.5 35.5T790-540H170Zm10-60h600v-180H180v180Zm100 390h60v-120h-60v120Zm0-420h60v-120h-60v120Z",
  settings:
    "m388-80-20-126q-19-7-40-19t-37-25l-118 54-93-164 108-79q-2-9-2.5-20.5T185-480q0-9 .5-20.5T188-521L80-600l93-164 118 54q16-13 37-25t40-18l20-127h184l20 126q19 7 40.5 18.5T669-710l118-54 93 164-108 77q2 10 2.5 21.5t.5 21.5q0 10-.5 21t-2.5 21l108 78-93 164-118-54q-16 13-36.5 25.5T592-206L572-80H388Zm48-60h88l14-112q33-8 62.5-25t53.5-41l106 46 40-72-94-69q4-17 6.5-33.5T715-480q0-17-2-33.5t-7-33.5l94-69-40-72-106 46q-23-26-52-43.5T538-708l-14-112h-88l-14 112q-34 7-63.5 24T306-642l-106-46-40 72 94 69q-4 17-6.5 33.5T245-480q0 17 2.5 33.5T254-413l-94 69 40 72 106-46q24 24 53.5 41t62.5 25l14 112Zm44-210q54 0 92-38t38-92q0-54-38-92t-92-38q-54 0-92 38t-38 92q0 54 38 92t92 38Zm0-130Z",
  shield:
    "m436-347 228-228-42-41-183 183-101-101-44 44 142 143Zm44 266q-140-35-230-162.5T160-523v-238l320-120 320 120v238q0 152-90 279.5T480-81Zm0-62q115-38 187.5-143.5T740-523v-196l-260-98-260 98v196q0 131 72.5 236.5T480-143Zm0-337Z",
  stop: "M300-660v360-360Zm-60 420v-480h480v480H240Zm60-60h360v-360H300v360Z",
  visibility:
    "M480-320q75 0 127.5-52.5T660-500q0-75-52.5-127.5T480-680q-75 0-127.5 52.5T300-500q0 75 52.5 127.5T480-320Zm0-72q-45 0-76.5-31.5T372-500q0-45 31.5-76.5T480-608q45 0 76.5 31.5T588-500q0 45-31.5 76.5T480-392Zm0 192q-146 0-266-81.5T40-500q54-137 174-218.5T480-800q146 0 266 81.5T920-500q-54 137-174 218.5T480-200Zm0-300Zm0 220q113 0 207.5-59.5T832-500q-50-101-144.5-160.5T480-720q-113 0-207.5 59.5T128-500q50 101 144.5 160.5T480-280Z",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg className="icon" width={size} height={size} viewBox="0 -960 960 960" fill="currentColor" aria-hidden="true">
      <path d={PATHS[name]} />
    </svg>
  );
}

/** "Ctrl+Alt+Del" as plain text, in one piece: "Ctrl + Alt + Del". */
export function Keys({ combo }: { combo: string }) {
  return <span className="keys">{combo.split("+").join(" + ")}</span>;
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <label className="toggle">
      <input type="checkbox" role="switch" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span />
    </label>
  );
}

/** A status in a word or two, in an oval; `tone` is its colour (`.pill` in style.css). No dot (as Lumen Client's
 * cards, the owner's wish 2026-11-09); `detail`: a second, smaller line under it, both centred (a host's version on
 * a machine's card). */
export function Pill({ tone, children, detail }: { tone: string; children: ReactNode; detail?: ReactNode }) {
  if (!detail)
    return (
      <span className={`pill ${tone}`}>
        {children}
      </span>
    );
  return (
    <span className={`pill two ${tone}`}>
      <span className="pill-lines">
        <span>{children}</span>
        <span className="pill-detail">{detail}</span>
      </span>
    </span>
  );
}

/** A row of the settings: a title, up to three lines of text, the control. */
export function Setting({ title, children, control }: { title: string; children: ReactNode; control: ReactNode }) {
  return (
    <li className="setting">
      <div className="setting-text">
        <h3>{title}</h3>
        {children}
      </div>
      <div className="setting-control">{control}</div>
    </li>
  );
}

type WindowApi = {
  minimize: () => unknown;
  toggleMaximize: () => unknown;
  close: () => unknown;
  isMaximized: () => Promise<boolean>;
};

function CaptionIcon({ d }: { d: string }) {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
      <path d={d} />
    </svg>
  );
}

/** The last row of both apps' settings: the folder with the log. */
export function LogSetting({ onOpen }: { onOpen: () => void }) {
  const w = W();
  return (
    <Setting
      title={w.log}
      control={
        <button className="btn wide" onClick={onOpen}>
          <Icon name="folder" />
          {w.openFolder}
        </button>
      }
    >
      <p>{w.logText}</p>
    </Setting>
  );
}

/**
 * A globe drawn with lines on a 20 px grid: the filled one of Material Symbols has cells of a pixel or two and
 * looks ragged at 100 %.
 */
function Globe() {
  return (
    <svg className="icon" width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <circle cx="10" cy="10" r="8.25" />
      <ellipse cx="10" cy="10" rx="3.5" ry="8.25" />
      <path d="M1.75 10h16.5" />
    </svg>
  );
}

/**
 * The language: a globe, a click switches to the next one (two of them: English and Russian). Its tooltip names the
 * language it switches to, in that language's own words. The same in Lumen Client, Lumen Host and the admin panel.
 */
export function LangToggle({ onLang, className = "lang-toggle" }: { onLang: (l: Lang) => void; className?: string }) {
  const i = LANGS.findIndex((l) => l.id === lang());
  const next = LANGS[(i + 1) % LANGS.length];
  return (
    <button type="button" className={className} title={next.name} aria-label={next.name} onClick={() => onLang(next.id)}>
      <Globe />
    </button>
  );
}

/** The left of a welcome page (the first start of Lumen Client and Lumen Host, a landing page): the mark, the
 * app's name, what it is for. */
export function WelcomeHero({ name }: { name: string }) {
  return (
    <div className="welcome-hero">
      <img src={logo} alt="" width={96} height={96} />
      <h1>{name}</h1>
      <p>{W().tagline}</p>
    </div>
  );
}

/** A welcome page form's head: its title, the globe at its right (the tabs, and their globe, are hidden meanwhile). */
export function WelcomeHead({ title, onLang }: { title: string; onLang: (l: Lang) => void }) {
  return (
    <div className="welcome-head">
      <h2>{title}</h2>
      <LangToggle className="ghost-icon welcome-lang" onLang={onLang} />
    </div>
  );
}

/** The window's own frame. */
export function TitleBar({ name, closeTitle, win }: { name: string; closeTitle: string; win: WindowApi }) {
  const w = W();
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    const check = () => win.isMaximized().then(setMaximized);
    check();
    window.addEventListener("resize", check);
    return () => window.removeEventListener("resize", check);
  }, [win]);
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar-name" data-tauri-drag-region>
        <img src={logo} alt="" width={16} height={16} data-tauri-drag-region />
        <span data-tauri-drag-region>{name}</span>
      </div>
      <div className="caption">
        <button title={w.minimize} onClick={win.minimize}>
          <CaptionIcon d="M0 5.5h10" />
        </button>
        <button title={maximized ? w.restore : w.maximize} onClick={win.toggleMaximize}>
          <CaptionIcon d={maximized ? "M2.5 2.5v-2h7v7h-2M.5 2.5h7v7h-7z" : "M.5.5h9v9h-9z"} />
        </button>
        <button className="close" title={closeTitle} onClick={win.close}>
          <CaptionIcon d="M.5.5l9 9M9.5.5l-9 9" />
        </button>
      </div>
    </header>
  );
}

/** A modal: Esc closes; the first field, else the default button, takes the focus. Without `onClose`: no cross,
 * no Escape (the update under way: it cannot be skipped); without `footer`: no buttons. */
export function Dialog({ title, onClose, children, footer }: { title: string; onClose?: () => void; children: ReactNode; footer: ReactNode }) {
  const w = W();
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.querySelector<HTMLElement>("input, button.dialog-default")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // Once, when it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="scrim">
      <div className="dialog" role="dialog" aria-modal="true" aria-label={title} ref={box}>
        <header>
          <h2>{title}</h2>
          {onClose && (
            <button className="ghost-icon" title={w.close} onClick={onClose}>
              <Icon name="close" size={18} />
            </button>
          )}
        </header>
        <div className="dialog-body">{children}</div>
        {footer && <footer>{footer}</footer>}
      </div>
    </div>
  );
}

/** "Sure?" before something that cannot be undone: «Cancel» is the default, `action` closes it and does it. */
export function Confirm({ title, action, onClose, onConfirm, children }: { title: string; action: string; onClose: () => void; onConfirm: () => void; children: ReactNode }) {
  const w = W();
  return (
    <Dialog
      title={title}
      onClose={onClose}
      footer={
        <>
          <button className="btn plain dialog-default" onClick={onClose}>
            {w.cancel}
          </button>
          <button
            className="btn solid-danger"
            onClick={() => {
              onClose();
              onConfirm();
            }}
          >
            {action}
          </button>
        </>
      }
    >
      {children}
    </Dialog>
  );
}

/** One line that is always there under a card's fields (Lumen Host's cards, both welcome pages): a message does
 * not change the card's height. No children: the empty line. `lock`: a quiet note with a lock. */
export function CardLine({ tone = "", title, lock, children }: { tone?: string; title?: string; lock?: boolean; children?: ReactNode }) {
  if (children === undefined) return <p className="card-text card-empty" />;
  return (
    <p className={`card-text one-line ${lock ? "card-note" : ""} ${tone}`} title={title}>
      {lock && <Icon name="lock" size={14} />}
      {children}
    </p>
  );
}

/** A name with anything but Latin letters, digits, spaces and printable signs: refused by the server
 * (`protocol::signal::name_allowed`, owner's decision 2026-11-09) — a machine's, a client's, the admin's for a client. */
const nameNotLatin = (name: string) => /[^\x20-\x7E]/.test(name.trim());
/** The server's limit for a name: 64 bytes (Latin only: 64 characters). */
const nameTooLong = (name: string) => new TextEncoder().encode(name.trim()).length > 64;
/** What is wrong with a name a person gives, in words ("": nothing). `long`: other words for a long one. */
export const nameProblem = (name: string, long = W().nameLong) => (nameNotLatin(name) ? W().nameLatin : nameTooLong(name) ? long : "");

/** A button held to see what a field hides (a token, a password): `setShown(true)` while it is held. */
export function holdToSee(setShown: (shown: boolean) => void) {
  const hide = () => setShown(false);
  return {
    // Keeps the focus and the caret in the field.
    onMouseDown: (e: React.MouseEvent) => e.preventDefault(),
    onPointerDown: (e: React.PointerEvent<HTMLButtonElement>) => {
      e.currentTarget.setPointerCapture(e.pointerId);
      setShown(true);
    },
    onPointerUp: hide,
    onPointerCancel: hide,
    onLostPointerCapture: hide,
    onClick: (e: React.MouseEvent) => e.preventDefault(),
  };
}

/** A token field: dots and dashes like everywhere else; the text shows only while the eye is held. */
export function TokenInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const w = W();
  const [shown, setShown] = useState(false);
  const [scroll, setScroll] = useState(0);
  return (
    <span className={`secret in-field${shown ? " shown" : ""}`}>
      <input
        className="mono"
        type="text"
        value={value}
        placeholder="XXXX-XXXX-XXXX-XXXX-XXXX-XXXX"
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        onScroll={(e) => setScroll(e.currentTarget.scrollLeft)}
        // Like a password field: the hidden token does not leave through the clipboard.
        onCopy={(e) => !shown && e.preventDefault()}
        onCut={(e) => !shown && e.preventDefault()}
        onDragStart={(e) => !shown && e.preventDefault()}
      />
      {!shown && (
        <span className="secret-mask" aria-hidden="true">
          <span style={{ transform: `translateX(${-scroll}px)` }}>{value.replace(/[^-]/g, "•")}</span>
        </span>
      )}
      <button type="button" className="in-field-button" tabIndex={-1} title={w.holdToSee} aria-label={w.showToken} {...holdToSee(setShown)}>
        <Icon name="visibility" />
      </button>
    </span>
  );
}

export type Resolution = { width: number; height: number; aspect: string };

/** Resolutions by aspect ("16:9", ...), in the list's order: the groups of a select. */
export function byAspect<R extends Resolution>(list: R[]): [string, R[]][] {
  const groups = new Map<string, R[]>();
  for (const r of list) groups.set(r.aspect, [...(groups.get(r.aspect) ?? []), r]);
  return [...groups];
}
