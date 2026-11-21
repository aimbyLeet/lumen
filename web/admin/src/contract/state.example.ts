// What the core really sends: written by `cargo test` (signal::contract), do not edit.
// `tsc` checks it against the type: what differs fails the build.
import type { State } from "../api";

export const examples = [
  {
    "access": [
      {
        "account": "",
        "client": 2,
        "client_name": "R3tr0",
        "decided": null,
        "host": 1,
        "machine": "WPC-10",
        "requested": 0,
        "status": "pending"
      }
    ],
    "clients": [
      {
        "admin_name": null,
        "files": null,
        "id": 2,
        "last_seen": 0,
        "name": "R3tr0",
        "online": true,
        "override": {
          "fps": null,
          "mbps": null
        },
        "version": "0"
      }
    ],
    "files": {
      "allowed": true,
      "mbps": 100
    },
    "fps_choices": [
      30,
      60
    ],
    "global_override": {
      "fps": null,
      "mbps": null
    },
    "hosts": [
      {
        "account": "",
        "active": true,
        "id": 1,
        "last_seen": 0,
        "machine": "WPC-10",
        "online": true,
        "update_failed": "",
        "updating": false,
        "version": "0"
      }
    ],
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "now": 0,
    "pending": 1,
    "sessions": [],
    "user": "admin"
  },
  {
    "access": [
      {
        "account": "",
        "client": 2,
        "client_name": "R3tr0",
        "decided": 0,
        "host": 1,
        "machine": "WPC-10",
        "requested": 0,
        "status": "approved"
      }
    ],
    "clients": [
      {
        "admin_name": null,
        "files": null,
        "id": 2,
        "last_seen": 0,
        "name": "R3tr0",
        "online": true,
        "override": {
          "fps": 30,
          "mbps": null
        },
        "version": "0"
      }
    ],
    "files": {
      "allowed": true,
      "mbps": 100
    },
    "fps_choices": [
      30,
      60
    ],
    "global_override": {
      "fps": null,
      "mbps": null
    },
    "hosts": [
      {
        "account": "",
        "active": true,
        "id": 1,
        "last_seen": 0,
        "machine": "WPC-10",
        "online": true,
        "update_failed": "",
        "updating": false,
        "version": "0"
      }
    ],
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "now": 0,
    "pending": 0,
    "sessions": [
      {
        "client": 2,
        "client_name": "R3tr0",
        "fps": 60,
        "host": 1,
        "id": "0",
        "mbps": 0.0,
        "seconds": 0
      }
    ],
    "user": "admin"
  }
] satisfies State[];
