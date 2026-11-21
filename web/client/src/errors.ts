// The words of what the core reports. The core sends a code: `code` or `code|detail|detail`;
// it has no words of its own. Details (a system's message, a path) come as they are. Used by the hub and Lumen Host.
import { lang } from "./i18n.ts";

type Say = (...d: string[]) => string;

const FIELDS_EN: Record<string, string> = { server: "Server", machine: "Machine", addresses: "Addresses" };
const FIELDS_RU: Record<string, string> = { server: "Сервер", machine: "Машина", addresses: "Адреса" };
const CLEAN_UP_EN = "the administrator will remove the server's record in the admin panel";
const CLEAN_UP_RU = "запись на сервере удалит администратор в админке";

const EN = {
  // Lumen Client
  err_video_start: (d) => `Could not start the video window: ${d}`,
  err_video_exit: (d) => `The video window closed with an error (${d})`,
  err_name_locked: () => "To change the name, remove all machines from the list",
  err_name_invalid: () => "Name: Latin letters, digits and signs only, up to 64 characters",
  err_name_required: () => "Enter your name",
  err_server_has_machines: () => "To change the server, remove all machines from the list",
  err_server_required: () => "Enter the server's address",
  err_quality: () => "Frame rate 30 or 60, cap — auto or from the list",
  err_lang: () => "Unknown language",
  err_no_log_folder: () => "The log has no folder",
  err_unknown_device: () => "The server does not know this computer",
  err_no_card: () => "No such card",
  err_host_not_found: () => "No host with this token on the server (a typo, or the host changed its token)",
  err_token: () => "Token: wrong format (like XXXX-XXXX-XXXX-XXXX-XXXX-XXXX)",
  err_card_exists: () => "This card is already there",
  err_reboot_offline: () => "The machine is not online: there is nothing to restart",
  err_reboot_not_approved: () => "The administrator has not approved access to this machine",
  err_reboot_failed: (d) => `Could not restart the machine: ${d}`,
  note_client_unreachable: (d) => `The server is unreachable (${d}): ${CLEAN_UP_EN}`,
  note_client_unconfirmed: (d) => `The server did not confirm removing this computer (${d}): ${CLEAN_UP_EN}`,

  // Both
  err_server_address: () => "Server: an address like lumen.example.com or lumen.example.com:7443",
  err_open_folder: (d) => `Could not open the folder: ${d}`,
  err_cert_untrusted: (fp) => `The server has its own certificate, not trusted yet (fingerprint ${fp})`,
  err_cert_changed: (fp) =>
    `The server's certificate has changed (fingerprint ${fp}): the server was reinstalled, or it is not your server`,
  err_cert_fingerprint: () => "The server's fingerprint: 64 hex digits",
  // Updates: the door, the download, the check.
  err_update_download: () => "Could not download the update from the server",
  err_update_busy: () => "The server is busy with other updates; try again later",
  err_update_sha: () => "The downloaded update is damaged (checksum mismatch)",
  err_update_disk: () => "Could not save the update on this computer",
  err_update_running: () => "The update is already under way",
  err_update_none: () => "The server has no newer version",
  err_update_run: () => "Could not start the installer",
  err_update_install: () => "The installer did not install the new version (its log: install.log next to it)",

  // Lumen Host
  err_machine_name: () => "Machine name: Latin letters, digits and signs only, 1 to 64 characters",
  err_ca_file: (d) => `The server's CA certificate: ${d}`,
  err_ca_big: () => "The server's certificate is too large",
  err_probe_timeout: () => "does not answer",
  err_probe_closed: () => "closed the connection (a server of another Lumen version?)",
  err_no_log: () => "No log yet: the host has not run",
  err_host_exited: () => "The host process has exited",
  err_field_long: (what) => `${FIELDS_EN[what] ?? what}: up to 128 characters, no control characters`,
  err_server_empty: () => "No server given",
  err_leading_dash: () => 'The value cannot start with "-"',
  err_not_registered: () => "This machine is not registered",
  err_server_locked: () => `A registered machine's server does not change: first "Remove registration", then "Register" on the new one`,
  err_caller: (d) => `Could not tell who the user is: ${d}`,
  err_bad_request: (d) => `The service did not understand the request: ${d}`,
  err_sas_only: () => "Ctrl+Alt+Del is only for hosts run by the service",
  err_system_only: () => "This request is only for hosts run by the service",
  err_user_only: () => "Only a Windows user can register the host",
  note_removed: () => "The registration is removed — both here and on the server",
  note_removed_local: () => "The registration is removed (the machine was not registered on the server)",
  note_removed_unknown: () => "The registration is removed (the server no longer knew this machine)",
  note_removed_here: (d) => `The registration is removed here; the server is unreachable (${d}) — ${CLEAN_UP_EN}`,
  note_last_session: (client, reason) => `The last session with ${client}: ${reason}`,

  // The service
  err_service_down: (d) => `The Lumen Host service does not answer (${d}): is it installed and running?`,
  err_service: (d) => `The Lumen Host service: ${d}`,
  err_service_answer: (d) => `The Lumen Host service: an answer that makes no sense (${d})`,
  err_service_missing: () => "The Lumen Host service is not installed (or there is no right to start it)",
  err_service_start: (d) => `The Lumen Host service does not start: ${d}`,
  err_service_silent: () => "The Lumen Host service is running but does not answer",

  // Monitors
  err_vdd_missing: () => "Lumen VDD is not installed",
  err_client_monitors_gone: () => "The client has already disconnected: its monitors are no longer shown",
  err_monitors_no_physical: () => "Without a physical monitor the virtual ones are not made: Lumen Client gets its own when it connects",
  err_monitor_place: (d) => `Monitor arrangement: ${d}`,
  err_monitor_scale: (d) => `Monitor scale: ${d}`,
  err_monitors_limit: (most, allowed) => `A client gets at most ${most} monitors: there can be at most ${allowed} virtual ones here`,
  err_layout_too_many: (most) => `At most ${most} virtual monitors`,
  err_layout_resolution: (m) => `Monitor ${m}: the resolution is not in the list`,
  err_layout_scale: (m) => `Monitor ${m}: the scale is not in the list`,
  err_layout_attach: (m) => `Monitor ${m}: can only adjoin the monitors before it`,
  err_layout_overlap: (a, b) => `Monitors ${a} and ${b} overlap`,
} satisfies Record<string, Say>;

const RU: Record<keyof typeof EN, Say> = {
  // Lumen Client
  err_video_start: (d) => `Не удалось запустить окно видео: ${d}`,
  err_video_exit: (d) => `Окно видео закрылось с ошибкой (${d})`,
  err_name_locked: () => "Чтобы изменить имя, удалите все машины из списка",
  err_name_invalid: () => "Имя: только латинские буквы, цифры и знаки, до 64 символов",
  err_name_required: () => "Введите имя",
  err_server_has_machines: () => "Чтобы сменить сервер, удалите все машины из списка",
  err_server_required: () => "Введите адрес сервера",
  err_quality: () => "Частота 30 или 60, потолок — авто или из списка",
  err_lang: () => "Неизвестный язык",
  err_no_log_folder: () => "У журнала нет папки",
  err_unknown_device: () => "Сервер не знает этот компьютер",
  err_no_card: () => "Нет такой карточки",
  err_host_not_found: () => "Хост с таким токеном на сервере не найден (опечатка, или хост сменил токен)",
  err_token: () => "Токен: неверный формат (вида XXXX-XXXX-XXXX-XXXX-XXXX-XXXX)",
  err_card_exists: () => "Такая карточка уже есть",
  err_reboot_offline: () => "Машина не в сети — перезагружать нечего",
  err_reboot_not_approved: () => "Администратор не одобрил доступ к этой машине",
  err_reboot_failed: (d) => `Не удалось перезагрузить машину: ${d}`,
  note_client_unreachable: (d) => `Сервер недоступен (${d}): ${CLEAN_UP_RU}`,
  note_client_unconfirmed: (d) => `Сервер не подтвердил удаление этого компьютера (${d}): ${CLEAN_UP_RU}`,

  // Both
  err_server_address: () => "Сервер: адрес вида lumen.example.com или lumen.example.com:7443",
  err_open_folder: (d) => `Не удалось открыть папку: ${d}`,
  err_cert_untrusted: (fp) => `У сервера собственный сертификат, ему ещё нет доверия (отпечаток ${fp})`,
  err_cert_changed: (fp) =>
    `Сертификат сервера изменился (отпечаток ${fp}): сервер переустановили — или это не ваш сервер`,
  err_cert_fingerprint: () => "Отпечаток сервера: 64 шестнадцатеричные цифры",
  err_update_download: () => "Не удалось скачать обновление с сервера",
  err_update_busy: () => "Сервер занят другими обновлениями, попробуйте позже",
  err_update_sha: () => "Скачанное обновление повреждено (не сходится контрольная сумма)",
  err_update_disk: () => "Не удалось сохранить обновление на этом компьютере",
  err_update_running: () => "Обновление уже идёт",
  err_update_none: () => "На сервере нет более новой версии",
  err_update_run: () => "Не удалось запустить установщик",
  err_update_install: () => "Установщик не поставил новую версию (его журнал — install.log рядом с ним)",

  // Lumen Host
  err_machine_name: () => "Имя машины: только латинские буквы, цифры и знаки, от 1 до 64 символов",
  err_ca_file: (d) => `Сертификат CA сервера: ${d}`,
  err_ca_big: () => "Сертификат сервера слишком большой",
  err_probe_timeout: () => "не отвечает",
  err_probe_closed: () => "закрыл соединение (сервер другой версии Lumen?)",
  err_no_log: () => "Журнала ещё нет: хост не запускался",
  err_host_exited: () => "Процесс хоста завершился",
  err_field_long: (what) => `${FIELDS_RU[what] ?? what}: до 128 символов, без управляющих`,
  err_server_empty: () => "Сервер не указан",
  err_leading_dash: () => "Значение не может начинаться с «-»",
  err_not_registered: () => "Регистрации на этой машине нет",
  err_server_locked: () => "Сервер зарегистрированной машины не меняется: сначала «Удалить», затем «Зарегистрировать» на новом",
  err_caller: (d) => `Не удалось определить пользователя: ${d}`,
  err_bad_request: (d) => `Служба не поняла запрос: ${d}`,
  err_sas_only: () => "Ctrl+Alt+Del — только для хостов службы",
  err_system_only: () => "Этот запрос — только для хостов службы",
  err_user_only: () => "Регистрация хоста — только для пользователя Windows",
  note_removed: () => "Регистрация удалена — и здесь, и на сервере",
  note_removed_local: () => "Регистрация удалена (на сервере машина зарегистрирована не была)",
  note_removed_unknown: () => "Регистрация удалена (сервер эту машину уже не знал)",
  note_removed_here: (d) => `Регистрация удалена здесь; сервер недоступен (${d}) — ${CLEAN_UP_RU}`,
  note_last_session: (client, reason) => `Последняя сессия с ${client}: ${reason}`,

  // The service
  err_service_down: (d) => `Служба Lumen Host не отвечает (${d}): она установлена и запущена?`,
  err_service: (d) => `Служба Lumen Host: ${d}`,
  err_service_answer: (d) => `Служба Lumen Host: непонятный ответ (${d})`,
  err_service_missing: () => "Служба Lumen Host не установлена (или нет прав её запустить)",
  err_service_start: (d) => `Служба Lumen Host не запускается: ${d}`,
  err_service_silent: () => "Служба Lumen Host запущена, но не отвечает",

  // Monitors
  err_vdd_missing: () => "Lumen VDD не установлен",
  err_client_monitors_gone: () => "Клиент уже отключился: его мониторы больше не показываются",
  err_monitors_no_physical: () => "Без физического монитора виртуальные не создаются: Lumen Client получит свои при подключении",
  err_monitor_place: (d) => `Расположение мониторов: ${d}`,
  err_monitor_scale: (d) => `Масштаб мониторов: ${d}`,
  err_monitors_limit: (most, allowed) => `Клиенту передаётся не больше ${most} мониторов: виртуальных здесь можно не больше ${allowed}`,
  err_layout_too_many: (most) => `Виртуальных мониторов — не больше ${most}`,
  err_layout_resolution: (m) => `Монитор ${m}: разрешения нет в списке`,
  err_layout_scale: (m) => `Монитор ${m}: масштаба нет в списке`,
  err_layout_attach: (m) => `Монитор ${m}: может примыкать только к мониторам до него`,
  err_layout_overlap: (a, b) => `Мониторы ${a} и ${b} налезают друг на друга`,
};

/** Every code the core sends, in each language. */
export const MESSAGES: Record<string, Say> = EN;
export const MESSAGES_RU: Record<string, Say> = RU;

/** What the core reported, in words. Anything but a known code is shown as it came: a system's message, a fault here. */
export function errorText(e: unknown): string {
  const raw = String(e);
  const [code, ...details] = raw.split("|");
  const messages = lang() === "ru" ? MESSAGES_RU : MESSAGES;
  const say = Object.hasOwn(messages, code) ? messages[code] : undefined;
  if (!say) return raw;
  // Fewer places than details: a detail with a | in it stays whole.
  const room = Math.max(say.length, 1);
  const parts = details.length > room ? [...details.slice(0, room - 1), details.slice(room - 1).join("|")] : details;
  return say(...parts);
}
