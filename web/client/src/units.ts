// Units and the numbers written with them: the same in Lumen Client, Lumen Host, the admin panel and the overlays.
// No React here: `npm test` checks the overlay's texts made of them.
import { lang, words } from "./i18n.ts";

/** The units' words: what is written after a number. */
export const U = words(
  { gb: "GB", mb: "MB", kb: "KB", b: "B", s: "s", min: "min", h: "h", mbps: "Mbit/s", fps: "fps", auto: "Auto" },
  { gb: "ГБ", mb: "МБ", kb: "КБ", b: "Б", s: "с", min: "мин", h: "ч", mbps: "Мбит/с", fps: "к/с", auto: "Авто" },
);

/** "60 fps". */
export const fps = (n: number) => `${n} ${U().fps}`;

/** "40 Mbit/s". */
export const mbps = (n: number) => `${n} ${U().mbps}`;

/** A bitrate cap: 0 is "Auto" (the server's table). */
export const cap = (n: number) => (n > 0 ? mbps(n) : U().auto);

/** "2 h 15 min", "15 min": how long something has been going on. */
export function hoursMinutes(seconds: number): string {
  const u = U();
  const minutes = Math.max(0, Math.floor(seconds / 60));
  const h = Math.floor(minutes / 60);
  return h ? `${h} ${u.h} ${minutes % 60} ${u.min}` : `${minutes} ${u.min}`;
}

/** Dates and times written the current language's way. */
export const locale = () => (lang() === "ru" ? "ru-RU" : "en-US");

/** The port written after a server's host: `lumen.example.com:7443`, `[::1]:7443`. A bare IPv6 address has none. */
const PORT = /^(\[[^\]]*\]|[^:]*):\d+$/;

/** A server as people read it: the address without the port (`lumen.example.com:7443` → `lumen.example.com`). */
export const serverName = (server: string) => server.trim().replace(PORT, "$1");

/** The server's default port; an address with it means the same without it. */
const DEFAULT_PORT = 7443;

/** An address to put in a field: the default port is left out, another one stays (it is needed to connect). */
export const serverField = (server: string) => {
  const s = server.trim();
  return s.endsWith(`:${DEFAULT_PORT}`) ? serverName(s) : s;
};
