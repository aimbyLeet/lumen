// The lumen-server admin API (apps/server/src/admin.rs).

/** The frame rate and the cap the admin sets, each on its own. null: the client chooses; mbps 0: auto. */
export type Limits = { fps: number | null; mbps: number | null };

export type Host = {
  id: number;
  machine: string;
  /** Empty for the machine's host (the service's). */
  account: string;
  online: boolean;
  /** Someone is signed in on its screen. */
  active: boolean;
  last_seen: number;
  /** The Lumen version it last logged in with ("": never). */
  version: string;
  /** It said it is installing a newer Lumen. */
  updating: boolean;
  /** Its last update failed: an `err_update_…` code ("": no failure). */
  update_failed: string;
};

export type Client = {
  id: number;
  /** As the client calls itself in Lumen Client. */
  name: string;
  /** What the admin calls it: only here, in the admin panel. null: not set. */
  admin_name: string | null;
  override: Limits;
  /** Its own file transfer switch; null: as everyone. */
  files: boolean | null;
  online: boolean;
  last_seen: number;
  /** The Lumen version it last logged in with ("": never). */
  version: string;
};

/** How a host is shown: the machine, with the account for a stage 4 host. */
export function hostLabel(h: { machine: string; account: string }): string {
  return h.account ? `${h.machine} / ${h.account}` : h.machine;
}

/** How a client is shown in the admin panel: the admin's name first, with its own after it ("Anna [R3tr0]"). */
export function clientLabel(c: { name: string; admin_name: string | null }): string {
  return c.admin_name ? `${c.admin_name} [${c.name}]` : c.name;
}

/** A client by its ID, as [`clientLabel`]; `own`: its name where the client is not (yet) in the list. */
export function clientLabelOf(s: { clients: Client[] }, id: number, own: string): string {
  const c = s.clients.find((x) => x.id === id);
  return c ? clientLabel(c) : own;
}

/** An access (approval) is one client → host pair: its ID. */
export function accessId(a: { client: number; host: number }): string {
  return `${a.client}-${a.host}`;
}

type AccessStatus = "pending" | "approved" | "rejected";

export type Access = {
  client: number;
  client_name: string;
  host: number;
  machine: string;
  account: string;
  status: AccessStatus;
  requested: number;
  decided: number | null;
};

export type Session = {
  id: string;
  client: number;
  client_name: string;
  host: number;
  fps: number;
  mbps: number;
  seconds: number;
};

export type State = {
  user: string;
  now: number;
  fps_choices: number[];
  mbps_presets: number[];
  global_override: Limits;
  /** File transfers for everyone, and their speed cap. */
  files: { allowed: boolean; mbps: number };
  pending: number;
  hosts: Host[];
  clients: Client[];
  access: Access[];
  sessions: Session[];
};

export class Unauthorized extends Error {}

async function call(method: string, path: string, body?: unknown): Promise<unknown> {
  const r = await fetch(`/admin${path}`, {
    method,
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  const data = await r.json().catch(() => ({}));
  if (r.status === 401) throw new Unauthorized((data as { error?: string }).error ?? "log in");
  if (!r.ok) throw new Error((data as { error?: string }).error ?? `HTTP ${r.status}`);
  return data;
}

export const api = {
  state: () => call("GET", "/api/state") as Promise<State>,
  login: (user: string, password: string) => call("POST", "/api/login", { user, password }),
  logout: () => call("POST", "/api/logout", {}),
  approve: (client: number, host: number) => call("POST", "/api/approve", { client, host }),
  reject: (client: number, host: number) => call("POST", "/api/reject", { client, host }),
  unlink: (client: number, host: number) => call("POST", "/api/unlink", { client, host }),
  unlinkAll: (client: number) => call("POST", "/api/unlink-all", { client }),
  remove: (device: number) => call("POST", "/api/remove", { device }),
  /** A client's limits, or everyone's (client null). */
  override: (client: number | null, limits: Limits) => call("POST", "/api/override", client === null ? { limits } : { client, limits }),
  /** A client's switch (null: as everyone), or everyone's (client null). */
  files: (client: number | null, allowed: boolean | null) =>
    call("POST", "/api/files", client === null ? { allowed } : { client, allowed }),
  filesSpeed: (mbps: number) => call("POST", "/api/files-speed", { mbps }),
  /** The admin's name for a client; null clears it. */
  name: (client: number, name: string | null) => call("POST", "/api/name", { client, name }),
};
