// What a machine's state means in words: the short pill of its row.
import type { Card, VideoStatus } from "./api";
import { words } from "./i18n.ts";

export type Tone = "ok" | "off" | "wait" | "bad" | "active";
export type Status = { tone: Tone; pill: string };

const W = words(
  {
    videoFailed: "Video error",
    connecting: "Connecting",
    waiting: "Waiting",
    reconnecting: "Reconnecting",
    unreachable: "No answer",
    streaming: "Connected",
    monitorWanted: "Choosing a monitor",
    noMonitors: "No monitors",
    serverUnknown: "Not on the server",
    serverError: "No connection",
    notFound: "Token not found",
    checking: "Checking",
    pending: "Awaiting approval",
    rejected: "Rejected",
    noAccess: "No access",
    otherVersion: "Other version",
    updateOff: "the host does not update itself",
    hostNewer: "newer than this client",
    online: "Online",
    busy: "Busy",
    inactive: "Not on screen",
    updating: "Host is updating…",
    restarting: "Restarting…",
    updateFailed: "Host update failed",
    offline: "Offline",
    connect: "Connect",
    request: "Request access",
    update: "Update and connect",
    updatesOff: "Updates are off",
    updateClient: "Update the client",
  },
  {
    videoFailed: "Ошибка видео",
    connecting: "Подключение",
    waiting: "Ожидание",
    reconnecting: "Переподключение",
    unreachable: "Нет ответа",
    streaming: "Подключено",
    monitorWanted: "Выбор монитора",
    noMonitors: "Нет мониторов",
    serverUnknown: "Нет на сервере",
    serverError: "Нет связи",
    notFound: "Токен не найден",
    checking: "Проверка",
    pending: "Ждёт одобрения",
    rejected: "Отклонено",
    noAccess: "Нет доступа",
    otherVersion: "Другая версия",
    updateOff: "хост не обновляется сам",
    hostNewer: "новее этого клиента",
    online: "В сети",
    busy: "Занят",
    inactive: "Не на экране",
    updating: "Хост обновляется…",
    restarting: "Перезагружается…",
    updateFailed: "Обновление хоста не удалось",
    offline: "Не в сети",
    connect: "Подключиться",
    request: "Запросить подключение",
    update: "Обновить и подключиться",
    updatesOff: "Обновление отключено",
    updateClient: "Обновите клиент",
  },
);

export const videoFailed = (): Status => ({ tone: "bad", pill: W().videoFailed });

export function videoStatus(v: VideoStatus): Status {
  const w = W();
  switch (v.event) {
    case "connecting":
      return { tone: "active", pill: w.connecting };
    case "waiting":
      return { tone: "wait", pill: v.reason === "host_updating" ? w.updating : w.waiting };
    case "reconnecting":
      return { tone: "wait", pill: w.reconnecting };
    case "unreachable":
      return { tone: "bad", pill: w.unreachable };
    case "streaming":
      return { tone: "ok", pill: w.streaming };
    case "monitor_wanted":
      return { tone: "wait", pill: w.monitorWanted };
    case "no_monitors":
      return { tone: "bad", pill: w.noMonitors };
  }
}

/** Another version on the host, for the pill's second line: which, and the one it updates to. */
export function versionDetail(c: Card, version: string): string | undefined {
  const w = W();
  if (!c.host_version || c.host_version === version || c.found !== true || c.access !== "approved") return undefined;
  switch (c.host_update) {
    case "on_connect":
      return `${c.host_version} → ${version}`;
    case "off":
      return `${c.host_version} · ${w.updateOff}`;
    case "newer":
      return `${c.host_version} · ${w.hostNewer}`;
    default:
      return c.host_version;
  }
}

export function cardStatus(c: Card, version: string): Status {
  const w = W();
  if (c.server_unknown) return { tone: "bad", pill: w.serverUnknown };
  if (c.server_error) return { tone: "bad", pill: w.serverError };
  if (c.found === false) return { tone: "bad", pill: w.notFound };
  if (c.found === null) return { tone: "off", pill: w.checking };
  switch (c.access) {
    case "pending":
      return { tone: "wait", pill: w.pending };
    case "rejected":
      return { tone: "bad", pill: w.rejected };
    case "none":
      return { tone: "off", pill: w.noAccess };
  }
  // Before the version: an updating host is still the old one.
  if (c.presence === "updating") return { tone: "wait", pill: w.updating };
  if (c.presence === "restarting") return { tone: "wait", pill: w.restarting };
  // Another version: an older host that updates itself shows its presence (the button says «Update and connect»);
  // any other — a newer host, or one that does not update itself — cannot be connected to.
  if (c.host_version && c.host_version !== version) {
    if (c.host_update !== "on_connect") return { tone: "bad", pill: w.otherVersion };
    if (c.update_failed) return { tone: "bad", pill: w.updateFailed };
  }
  switch (c.presence) {
    case "online":
      return { tone: "ok", pill: w.online };
    case "busy":
      return { tone: "wait", pill: w.busy };
    case "inactive":
      return { tone: "wait", pill: w.inactive };
    default:
      return { tone: "off", pill: w.offline };
  }
}

/** The video window's title: the machine, with the account if there is one. */
export function videoTitle(machine: string, account: string): string {
  const [m, a] = [machine.trim(), account.trim()];
  if (!m) return "Lumen";
  return a ? `Lumen — ${m} / ${a}` : `Lumen — ${m}`;
}

/** What the machine's one button says. */
export function buttonLabel(op: Card["button"]["op"]): string {
  const w = W();
  return { connect: w.connect, update: w.update, update_off: w.updatesOff, update_client: w.updateClient, request: w.request, pending: w.pending, rejected: w.rejected }[op];
}
