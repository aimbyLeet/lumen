import assert from "node:assert/strict";
import { test } from "node:test";
import { serverField, serverName } from "./units.ts";

test("a server is shown without its port", () => {
  assert.equal(serverName("lumen.example.com:7443"), "lumen.example.com");
  assert.equal(serverName("lumen.example.com:8443"), "lumen.example.com");
  assert.equal(serverName("lumen.example.com"), "lumen.example.com");
  assert.equal(serverName("192.168.1.10:7443"), "192.168.1.10");
  assert.equal(serverName("[fe80::1]:7443"), "[fe80::1]");
  assert.equal(serverName("[fe80::1]"), "[fe80::1]");
  assert.equal(serverName("fe80::1"), "fe80::1");
  assert.equal(serverName(" a:7443 "), "a");
});

test("a field keeps a port other than the default", () => {
  assert.equal(serverField("lumen.example.com:7443"), "lumen.example.com");
  assert.equal(serverField("lumen.example.com:8443"), "lumen.example.com:8443");
  assert.equal(serverField("[::1]:7443"), "[::1]");
  assert.equal(serverField(""), "");
});
