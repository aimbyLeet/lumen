// A program updated from the server: the footer turns yellow when the server has a newer
// version; its click asks first, then the core downloads, checks and installs it, and the program starts again.
// Lumen Client and Lumen Host (web/host) alike.
import { useState } from "react";
import { api, type AppState, type Run } from "./api";
import { errorText } from "./errors";
import { words } from "./i18n";
import { Confirm, Dialog, Icon } from "./ui";

/** The server's version and this program's update, as the core says. */
export type UpdateInfo = {
  server_version: string;
  newer: boolean;
  phase: "" | "downloading" | "installing";
  error: string;
};

const W = words(
  {
    outdated: "Your version is out of date. Click to update",
    failed: (why: string) => `The update failed: ${why}. Click to try again`,
    askTitle: (program: string, v: string) => `Update ${program} to ${v}?`,
    askText: (program: string) => `${program} closes, installs the new version and starts again.`,
    askSession: "The session in progress will be cut off.",
    update: "Update",
    progressTitle: (v: string) => `Updating to ${v}`,
    downloading: "Downloading the new version from the server…",
    installing: (program: string) => `Installing. ${program} starts again in a moment.`,
  },
  {
    outdated: "Ваша версия устарела. Нажмите, чтобы обновиться",
    failed: (why: string) => `Обновление не удалось: ${why}. Нажмите, чтобы попробовать снова`,
    askTitle: (program: string, v: string) => `Обновить ${program} до ${v}?`,
    askText: (program: string) => `${program} закроется, установит новую версию и запустится снова.`,
    askSession: "Идущая сессия прервётся.",
    update: "Обновить",
    progressTitle: (v: string) => `Обновление до ${v}`,
    downloading: "Скачиваю новую версию с сервера…",
    installing: (program: string) => `Устанавливаю. ${program} сейчас запустится снова.`,
  },
);

/** The footer: the version in the corner; yellow and clickable when the server has a newer one. */
export function UpdateFooter({
  program,
  version,
  update: u,
  session,
  run,
  onUpdate,
}: {
  program: string;
  version: string;
  update: UpdateInfo;
  /** A session is under way: the question says it will be cut off. */
  session: boolean;
  run: (f: () => Promise<unknown>) => unknown;
  onUpdate: () => Promise<unknown>;
}) {
  const w = W();
  const [asking, setAsking] = useState(false);
  const shown = <span>v{version}</span>;
  if (!u.newer && !u.phase) return <footer className="statusbar">{shown}</footer>;
  // The first letter of an error's words is lowercase inside the sentence.
  const why = u.error ? errorText(u.error).replace(/^./, (c) => c.toLowerCase()) : "";
  return (
    <>
      <button type="button" className="statusbar outdated" disabled={!!u.phase} onClick={() => setAsking(true)}>
        <span className="outdated-text">
          <Icon name="download" size={14} />
          {u.error ? w.failed(why) : w.outdated}
        </span>
        {shown}
      </button>
      {asking && (
        <Confirm
          title={w.askTitle(program, u.server_version)}
          action={w.update}
          onClose={() => setAsking(false)}
          onConfirm={() => {
            setAsking(false);
            run(onUpdate);
          }}
        >
          <p>{w.askText(program)}</p>
          {session && <p>{w.askSession}</p>}
        </Confirm>
      )}
      {u.phase && (
        <Dialog title={w.progressTitle(u.server_version)} footer={null}>
          <p>{u.phase === "installing" ? w.installing(program) : w.downloading}</p>
        </Dialog>
      )}
    </>
  );
}

/** Lumen Client's footer. */
export function Footer({ s, run }: { s: AppState; run: Run }) {
  return <UpdateFooter program="Lumen Client" version={s.version} update={s.update} session={!!s.video} run={run} onUpdate={api.updateNow} />;
}
