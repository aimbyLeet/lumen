// What the core really sends: written by `cargo test` (signal::contract), do not edit.
// `tsc` checks it against the type: what differs fails the build.
import type { AppState } from "../api";

export const examples = [
  {
    "cards": [
      {
        "access": "approved",
        "account": "",
        "button": {
          "enabled": true,
          "op": "update"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 1,
        "machine": "WPC-10",
        "presence": "online",
        "server_error": "",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "",
        "account": "",
        "button": {
          "enabled": false,
          "op": "connect"
        },
        "found": null,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 2,
        "machine": "WPC-10",
        "presence": "",
        "server_error": "",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "pending",
        "account": "",
        "button": {
          "enabled": false,
          "op": "pending"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 3,
        "machine": "WPC-10",
        "presence": "offline",
        "server_error": "",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      }
    ],
    "forced": {
      "fps": 30,
      "mbps": null
    },
    "fps_choices": [
      30,
      60
    ],
    "last_video": null,
    "log_path": "client.log",
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "settings": {
      "auto_update": true,
      "borderless": false,
      "fps": 60,
      "keep_unlocked": false,
      "lang": "en",
      "mbps": 0.0,
      "name": "gate",
      "server": "a:7443",
      "shortcuts_to_host": true
    },
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0",
    "video": null
  },
  {
    "cards": [
      {
        "access": "approved",
        "account": "",
        "button": {
          "enabled": false,
          "op": "update"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 1,
        "machine": "WPC-10",
        "presence": "online",
        "server_error": "no link",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "",
        "account": "",
        "button": {
          "enabled": false,
          "op": "connect"
        },
        "found": null,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 2,
        "machine": "WPC-10",
        "presence": "",
        "server_error": "no link",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "pending",
        "account": "",
        "button": {
          "enabled": false,
          "op": "pending"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 3,
        "machine": "WPC-10",
        "presence": "offline",
        "server_error": "no link",
        "server_unknown": false,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      }
    ],
    "forced": {
      "fps": null,
      "mbps": null
    },
    "fps_choices": [
      30,
      60
    ],
    "last_video": null,
    "log_path": "client.log",
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "settings": {
      "auto_update": true,
      "borderless": false,
      "fps": 60,
      "keep_unlocked": false,
      "lang": "en",
      "mbps": 0.0,
      "name": "gate",
      "server": "a:7443",
      "shortcuts_to_host": true
    },
    "update": {
      "error": "",
      "newer": true,
      "phase": "downloading",
      "server_version": "9.9.9"
    },
    "version": "0",
    "video": null
  },
  {
    "cards": [
      {
        "access": "approved",
        "account": "",
        "button": {
          "enabled": true,
          "op": "request"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 1,
        "machine": "WPC-10",
        "presence": "online",
        "server_error": "err_unknown_device",
        "server_unknown": true,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "",
        "account": "",
        "button": {
          "enabled": true,
          "op": "request"
        },
        "found": null,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 2,
        "machine": "WPC-10",
        "presence": "",
        "server_error": "err_unknown_device",
        "server_unknown": true,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      },
      {
        "access": "pending",
        "account": "",
        "button": {
          "enabled": true,
          "op": "request"
        },
        "found": true,
        "host_update": "on_connect",
        "host_version": "0.0.0",
        "id": 3,
        "machine": "WPC-10",
        "presence": "offline",
        "server_error": "err_unknown_device",
        "server_unknown": true,
        "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
        "update_failed": ""
      }
    ],
    "forced": {
      "fps": null,
      "mbps": null
    },
    "fps_choices": [
      30,
      60
    ],
    "last_video": null,
    "log_path": "client.log",
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "settings": {
      "auto_update": true,
      "borderless": false,
      "fps": 60,
      "keep_unlocked": false,
      "lang": "en",
      "mbps": 0.0,
      "name": "gate",
      "server": "a:7443",
      "shortcuts_to_host": true
    },
    "update": {
      "error": "err_update_sha",
      "newer": true,
      "phase": "",
      "server_version": "9.9.9"
    },
    "version": "0",
    "video": null
  },
  {
    "cards": [],
    "forced": {
      "fps": null,
      "mbps": null
    },
    "fps_choices": [
      30,
      60
    ],
    "last_video": {
      "card": 2,
      "error": "err_video_exit|1"
    },
    "log_path": "client.log",
    "mbps_presets": [
      10.0,
      20.0,
      30.0,
      40.0,
      60.0,
      80.0,
      100.0
    ],
    "settings": {
      "auto_update": true,
      "borderless": false,
      "fps": 60,
      "keep_unlocked": false,
      "lang": "en",
      "mbps": 0.0,
      "name": "",
      "server": "",
      "shortcuts_to_host": true
    },
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0",
    "video": {
      "card": 1,
      "since": 5,
      "status": {
        "event": "connecting"
      }
    }
  }
] satisfies AppState[];
