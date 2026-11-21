// `vite --mode mock` only: stands in for lumen-server by replacing fetch, so src/api.ts runs as it is.
// Scenes: `?scene=...`, picked in Panel.tsx. Actions change the state, as on the real server.
import type { Access, Client, Host, Limits, Session, State } from "../api";

const VERSION = "0.5.12";
const OLD = "0.5.11";
const now = () => Math.floor(Date.now() / 1000);
const MIN = 60;
const HOUR = 3600;
const DAY = 86400;

type Link = { client: number; host: number; status: Access["status"]; requested: number; decided: number | null };
type Live = { client: number; host: number; fps: number; mbps: number; started: number };
type Db = {
  clients: Client[];
  hosts: Host[];
  links: Link[];
  live: Live[];
  global: Limits;
  files: { allowed: boolean; mbps: number };
};

function client(id: number, name: string, more: Partial<Client> = {}): Client {
  return { id, name, admin_name: null, override: { fps: null, mbps: null }, files: null, online: false, last_seen: now() - 2 * HOUR, version: VERSION, ...more };
}
function host(id: number, machine: string, more: Partial<Host> = {}): Host {
  return { id, machine, account: "", online: true, active: true, last_seen: now(), version: VERSION, updating: false, update_failed: "", ...more };
}
const approved = (client: number, host: number, ago = 3 * DAY): Link => ({ client, host, status: "approved", requested: now() - ago - HOUR, decided: now() - ago });
const pending = (client: number, host: number, ago = 2 * MIN): Link => ({ client, host, status: "pending", requested: now() - ago, decided: null });
const rejected = (client: number, host: number, ago = 5 * DAY): Link => ({ client, host, status: "rejected", requested: now() - ago - HOUR, decided: now() - ago });
const live = (client: number, host: number, fps: number, mbps: number, ago: number): Live => ({ client, host, fps, mbps, started: now() - ago });
const defaults = { global: { fps: null, mbps: null }, files: { allowed: true, mbps: 100 } };

function usual(): Db {
  return {
    ...defaults,
    clients: [
      client(1, "Анна Иванова", { online: true, override: { fps: 60, mbps: 40 } }),
      client(2, "Пётр Петров", { online: true, override: { mbps: 20, fps: null } }),
      client(3, "M.Sokolova", { admin_name: "Мария, монтаж", online: true, files: false, override: { fps: 30, mbps: null } }),
      client(4, "Дмитрий Козлов", { online: true, version: OLD }),
      client(5, "Ноутбук Елены", { last_seen: now() - 3 * DAY }),
      client(6, "new-laptop", { online: true }),
    ],
    hosts: [
      host(11, "WPC-10"),
      host(12, "WPC-11"),
      host(13, "WPC-12"),
      host(14, "GRADE-01"),
      host(15, "GRADE-02", { online: false, last_seen: now() - 5 * HOUR, updating: true, version: OLD }),
      host(16, "RETOUCH-03", { online: false, last_seen: now() - 2 * DAY, version: OLD, update_failed: "err_update_download" }),
    ],
    links: [
      approved(1, 11),
      approved(1, 14, 9 * DAY),
      approved(1, 15, 9 * DAY),
      approved(2, 12),
      approved(2, 14, 20 * HOUR),
      approved(3, 13),
      rejected(3, 16),
      approved(4, 13, 30 * DAY),
      approved(4, 12, 30 * DAY),
      approved(5, 15),
      pending(6, 12),
      pending(2, 16, 25 * MIN),
    ],
    live: [live(1, 11, 60, 40, HOUR + 12 * MIN), live(3, 13, 30, 0, 8 * MIN + 12)],
  };
}

const SURNAMES = ["Иванов", "Петрова", "Соколов", "Козлова", "Морозов", "Волкова", "Лебедев", "Новикова", "Фёдоров", "Егорова", "Павлов", "Орлова", "Семёнов", "Зайцева", "Голубев", "Виноградова", "Богданов", "Воробьёва", "Смирнов", "Кузнецова"];

function office(): Db {
  const hosts = Array.from({ length: 40 }, (_, i) =>
    host(100 + i, `WS-${String(i + 1).padStart(2, "0")}`, i % 7 === 6 ? { online: false, last_seen: now() - (i + 1) * HOUR } : {}),
  );
  const clients = Array.from({ length: 40 }, (_, i) =>
    client(i + 1, `${SURNAMES[i % 20]}${i >= 20 ? " 2" : ""}`, {
      online: i % 4 !== 3,
      override: i % 11 === 0 ? { fps: 60, mbps: 60 } : { fps: null, mbps: null },
      last_seen: now() - (i + 1) * 40 * MIN,
    }),
  );
  const links: Link[] = hosts.map((h, i) => approved(i + 1, h.id, (i + 1) * DAY));
  for (let i = 0; i < 8; i++) links.push(approved(i + 1, 110 + i, 2 * DAY));
  links.push(pending(39, 101), pending(40, 102, 40 * MIN));
  const live_ = Array.from({ length: 20 }, (_, i) => live(i + 1, 100 + i, i % 2 ? 30 : 60, i % 3 ? 40 : 0, (i + 1) * 9 * MIN)).filter((l) => hosts.find((h) => h.id === l.host)?.online);
  return { ...defaults, clients, hosts, links, live: live_ };
}

export const SCENES: Record<string, { title: string; db?: () => Db; login?: "any" | "wrong" | "locked"; fail?: "state" | "act" }> = {
  usual: { title: "Обычный день: заявки, сессии, отказ", db: usual },
  empty: { title: "Пусто: никто не зарегистрирован", db: () => ({ ...defaults, clients: [], hosts: [], links: [], live: [] }) },
  first: {
    title: "Первая заявка, ничего ещё не одобрено",
    db: () => ({ ...defaults, clients: [client(1, "A.Ivanov", { online: true })], hosts: [host(11, "WPC-10")], links: [pending(1, 11, 20)], live: [] }),
  },
  office: { title: "Офис: 40 клиентов, 40 машин, 18 сессий", db: office },
  global: {
    title: "Для всех задана частота, файлы выключены",
    db: () => ({ ...usual(), global: { fps: 30, mbps: null }, files: { allowed: false, mbps: 50 } }),
  },
  long: {
    title: "Длинные имена",
    db: () => {
      const d = usual();
      d.clients[0].name = "Анастасия Константиновна Преображенская-Вознесенская";
      d.clients[1].name = "очень-длинное-имя-компьютера-без-пробелов-которое-не-влезает-в-карточку";
      d.hosts[0].machine = "DESKTOP-COLOR-GRADING-STUDIO-NUMBER-ONE-MOSCOW";
      return d;
    },
  },
  lost: { title: "Связь с сервером потеряна", db: usual, fail: "state" },
  actfail: { title: "Сервер отказывает в действиях", db: usual, fail: "act" },
  login: { title: "Вход", db: usual, login: "any" },
  wrong: { title: "Вход: неверный пароль", db: usual, login: "wrong" },
  locked: { title: "Вход: блокировка подбора", db: usual, login: "locked" },
};

const params = new URLSearchParams(location.search);
export const sceneName = params.get("scene") && SCENES[params.get("scene")!] ? params.get("scene")! : "usual";
const scene = SCENES[sceneName];
const db: Db = (scene.db ?? usual)();
let loggedIn = !scene.login;
let stateCalls = 0;

function state(): State {
  const access: Access[] = db.links.map((l) => {
    const c = db.clients.find((x) => x.id === l.client)!;
    const h = db.hosts.find((x) => x.id === l.host)!;
    return { ...l, client_name: c.name, machine: h.machine, account: h.account };
  });
  const sessions: Session[] = db.live.map((l, i) => ({
    id: String(i).padStart(16, "0"),
    client: l.client,
    client_name: db.clients.find((c) => c.id === l.client)!.name,
    host: l.host,
    fps: l.fps,
    mbps: l.mbps,
    seconds: now() - l.started,
  }));
  return {
    user: "admin",
    now: now(),
    fps_choices: [30, 60],
    mbps_presets: [10, 20, 30, 40, 60, 80, 100],
    global_override: db.global,
    files: db.files,
    pending: db.links.filter((l) => l.status === "pending").length,
    hosts: db.hosts,
    clients: db.clients,
    access,
    sessions,
  };
}

function setLink(c: number, h: number, status: Link["status"]) {
  const l = db.links.find((x) => x.client === c && x.host === h);
  if (l) {
    l.status = status;
    l.decided = now();
  }
}
function drop(pred: (l: { client: number; host: number }) => boolean) {
  db.links = db.links.filter((l) => !pred(l));
  db.live = db.live.filter((l) => !pred(l));
}

type Body = {
  user?: string;
  password?: string;
  client?: number | null;
  host?: number;
  device?: number;
  limits?: Limits;
  allowed?: boolean | null;
  mbps?: number;
  name?: string | null;
};

function post(path: string, b: Body): void {
  const c = db.clients.find((x) => x.id === b.client);
  switch (path) {
    case "/api/approve":
      setLink(b.client!, b.host!, "approved");
      break;
    case "/api/reject":
      setLink(b.client!, b.host!, "rejected");
      break;
    case "/api/unlink":
      drop((l) => l.client === b.client && l.host === b.host);
      break;
    case "/api/unlink-all":
      db.live = db.live.filter((l) => l.client !== b.client);
      db.links = db.links.filter((l) => l.client !== b.client || l.status !== "approved");
      break;
    case "/api/remove":
      drop((l) => l.client === b.device || l.host === b.device);
      db.clients = db.clients.filter((x) => x.id !== b.device);
      db.hosts = db.hosts.filter((x) => x.id !== b.device);
      break;
    case "/api/override":
      if (c) c.override = b.limits!;
      else db.global = b.limits!;
      break;
    case "/api/files":
      if (c) c.files = b.allowed ?? null;
      else db.files = { ...db.files, allowed: !!b.allowed };
      break;
    case "/api/name":
      if (c) c.admin_name = b.name?.trim() || null;
      break;
    case "/api/files-speed":
      db.files = { ...db.files, mbps: b.mbps! };
      break;
  }
}

const reply = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

const realFetch = window.fetch.bind(window);
window.fetch = async (input, init) => {
  const url = String(input);
  if (!url.startsWith("/admin/api/")) return realFetch(input, init);
  await new Promise((r) => setTimeout(r, 60));
  const path = url.slice("/admin".length);
  const body: Body = init?.body ? JSON.parse(String(init.body)) : {};

  if (path === "/api/login") {
    if (scene.login === "locked") return reply(429, { error: "too many wrong passwords: wait a minute" });
    if (scene.login === "wrong") return reply(401, { error: "wrong login or password" });
    loggedIn = true;
    return reply(200, { ok: true });
  }
  if (!loggedIn) return reply(401, { error: "log in" });
  if (path === "/api/logout") {
    loggedIn = false;
    return reply(200, { ok: true });
  }
  if (path === "/api/state") {
    stateCalls++;
    if (scene.fail === "state" && stateCalls > 1) throw new TypeError("Failed to fetch");
    return reply(200, state());
  }
  if (scene.fail === "act") return reply(400, { error: "database is locked" });
  post(path, body);
  return reply(200, { ok: true });
};
