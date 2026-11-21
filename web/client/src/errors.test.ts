// `npm test`: the words of the core's codes, and that the core has no code without them.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { errorText, MESSAGES, MESSAGES_RU } from "./errors.ts";
import { setLang } from "./i18n.ts";
import { videoTitle } from "./status.ts";

const ROOT = join(import.meta.dirname, "..", "..", "..");

function rustFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) rustFiles(path, out);
    else if (name.endsWith(".rs")) out.push(path);
  }
  return out;
}

const sources = () => ["apps", "crates"].flatMap((d) => rustFiles(join(ROOT, d))).filter((f) => !/bindings\.rs$|guids\.rs$/.test(f));

test("every code in the core has its words, and all words have a code", () => {
  const used = new Set<string>();
  for (const f of sources()) {
    for (const m of readFileSync(f, "utf8").matchAll(/"((?:err|note)_[a-z0-9_]+)/g)) used.add(m[1]);
  }
  assert.ok(used.size > 40, `found ${used.size}`);
  assert.deepEqual([...used].filter((c) => !(c in MESSAGES)).sort(), [], "codes without words");
  assert.deepEqual(Object.keys(MESSAGES).filter((c) => !used.has(c)).sort(), [], "words nothing sends");
});

test("no words left in what the apps and the service report", () => {
  const cyrillic = /"[^"\n]*[А-Яа-яЁё][^"\n]*"/;
  const reporting = /\b(?:Err|bail!|format!|Error|ok_or|write!|error)\b/;
  const left: string[] = [];
  for (const dir of ["apps/client-app/src", "apps/host-app/src", "apps/host/src", "crates/vdd/src", "crates/signal/src"]) {
    for (const f of rustFiles(join(ROOT, dir))) {
      const lines = readFileSync(f, "utf8").split("\n");
      // Tests may use any text.
      const end = lines.findIndex((l) => l.includes("#[cfg(test)]"));
      for (const [i, l] of lines.slice(0, end < 0 ? undefined : end).entries()) {
        if (cyrillic.test(l) && reporting.test(l)) left.push(`${f}:${i + 1}: ${l.trim()}`);
      }
    }
  }
  assert.deepEqual(left, []);
});

test("the video window's title has no trailing separator", () => {
  assert.equal(videoTitle("", "x"), "Lumen");
  assert.equal(videoTitle("PC", ""), "Lumen — PC");
  assert.equal(videoTitle("PC", "  "), "Lumen — PC");
  assert.equal(videoTitle("PC", "bob"), "Lumen — PC / bob");
});

test("both languages have the words of every code", () => {
  assert.deepEqual(Object.keys(MESSAGES_RU).sort(), Object.keys(MESSAGES).sort());
});

test("codes become words in English by default", () => {
  assert.equal(errorText("err_card_exists"), "This card is already there");
  assert.equal(errorText("err_monitors_limit|3|1"), "A client gets at most 3 monitors: there can be at most 1 virtual ones here");
  assert.equal(errorText("err_field_long|server"), "Server: up to 128 characters, no control characters");
});

test("codes become words", () => {
  setLang("ru");
  assert.equal(errorText("err_card_exists"), "Такая карточка уже есть");
  assert.equal(errorText("err_token"), "Токен: неверный формат (вида XXXX-XXXX-XXXX-XXXX-XXXX-XXXX)");
  assert.equal(errorText("err_service_down|x"), "Служба Lumen Host не отвечает (x): она установлена и запущена?");
  assert.equal(errorText("err_monitors_limit|3|1"), "Клиенту передаётся не больше 3 мониторов: виртуальных здесь можно не больше 1");
  assert.equal(errorText("note_last_session|anna|Bye"), "Последняя сессия с anna: Bye");
  assert.equal(errorText("err_field_long|server"), "Сервер: до 128 символов, без управляющих");
  assert.equal(errorText("err_field_long|machine"), "Машина: до 128 символов, без управляющих");
});

test("a detail keeps its | and anything else comes as it is", () => {
  setLang("ru");
  assert.equal(errorText("err_service|a|b"), "Служба Lumen Host: a|b");
  assert.equal(errorText("connection refused (os error 10061)"), "connection refused (os error 10061)");
  assert.equal(errorText("toString"), "toString", "not a code, though objects have it");
  assert.equal(errorText(new Error("x")), "Error: x");
  setLang("en");
});
