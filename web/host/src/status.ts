// What the host's state means in words: the status panel of the Machine tab.
import { errorText } from "../../client/src/errors";
import { words } from "../../client/src/i18n";
import type { AppState } from "./api";

export type Tone = "ok" | "off" | "wait" | "bad" | "active";

const W = words(
  {
    off: ["Host is off", "Nobody can connect to this machine."],
    noService: "The service does not answer",
    notSet: ["Machine not set up", "Enter the server and register the machine."],
    starting: ["Starting", "The host is connecting to the server."],
    online: ["Online", "Nobody is connected. The machine waits for a client."],
    session: "Connected",
    offline: "No connection to the server",
    retrying: "The host tries to connect again.",
    retryingAfter: (detail: string) => `${detail}. The host tries to connect again.`,
    error: "Host error",
    badCredentials: ["The server does not know the machine", "Register it again. The token stays the same."],
    tokenTaken: ["Token taken", "The server has a machine with this token. Remove it in the admin panel or change the token."],
    badName: ["Name not accepted", "The server did not accept the machine's name. Enter another one."],
    version: ["Another server version", "Update the server or Lumen Host."],
    refused: "Refused by the server",
    stopped: "Stopped",
  },
  {
    off: ["Хост выключен", "К этой машине нельзя подключиться."],
    noService: "Служба не отвечает",
    notSet: ["Машина не настроена", "Укажите сервер и зарегистрируйте машину."],
    starting: ["Запуск", "Хост подключается к серверу."],
    online: ["В сети", "Никто не подключён. Машина ждёт клиента."],
    session: "Подключён",
    offline: "Нет связи с сервером",
    retrying: "Хост пробует подключиться снова.",
    retryingAfter: (detail: string) => `${detail}. Хост пробует подключиться снова.`,
    error: "Ошибка хоста",
    badCredentials: ["Сервер не знает машину", "Зарегистрируйте её заново. Токен останется прежним."],
    tokenTaken: ["Токен занят", "На сервере есть машина с этим токеном. Удалите её в админке или смените токен."],
    badName: ["Имя не принято", "Сервер не принял имя машины. Укажите другое."],
    version: ["Другая версия сервера", "Обновите сервер или Lumen Host."],
    refused: "Отказ сервера",
    stopped: "Остановлен",
  },
);

export function hostStatus(s: AppState): { tone: Tone; title: string; text: string } {
  const w = W();
  const st = s.status;
  const detail = errorText(st.detail);
  const say = (tone: Tone, [title, text]: string[]) => ({ tone, title, text });
  if (s.host_off) return say("off", w.off);
  if (s.service_error) return { tone: "bad", title: w.noService, text: errorText(s.service_error) };
  if (!s.registered) return say("off", w.notSet);
  switch (st.state) {
    case "starting":
      return say("wait", w.starting);
    case "online":
      return { tone: "ok", title: w.online[0], text: detail || w.online[1] };
    case "session":
      return { tone: "active", title: w.session, text: "" };
    case "offline":
      return { tone: "bad", title: w.offline, text: detail ? w.retryingAfter(detail) : w.retrying };
    case "error":
      return { tone: "bad", title: w.error, text: detail };
    case "refused":
      switch (st.detail) {
        case "BadCredentials":
          return say("bad", w.badCredentials);
        case "TokenTaken":
          return say("bad", w.tokenTaken);
        case "BadName":
          return say("bad", w.badName);
        case "Version":
          return say("bad", w.version);
        default:
          return { tone: "bad", title: w.refused, text: detail };
      }
    default:
      return { tone: "off", title: w.stopped, text: "" };
  }
}
