// What the core really sends: written by `cargo test` (signal::contract), do not edit.
// `tsc` checks it against the type: what differs fails the build.
import type { AppState } from "../api";

export const examples = [
  {
    "auto_update": false,
    "autostart": false,
    "defaults": {
      "machine": "WPC-10"
    },
    "host_off": false,
    "keep_awake": false,
    "lang": "en",
    "monitors": {
      "allowed": 2,
      "catalog": {
        "resolutions": [
          {
            "aspect": "16:9",
            "height": 1080,
            "width": 1920
          },
          {
            "aspect": "16:9",
            "height": 1440,
            "width": 2560
          },
          {
            "aspect": "16:9",
            "height": 1800,
            "width": 3200
          },
          {
            "aspect": "16:9",
            "height": 2160,
            "width": 3840
          },
          {
            "aspect": "16:10",
            "height": 1200,
            "width": 1920
          },
          {
            "aspect": "16:10",
            "height": 1600,
            "width": 2560
          },
          {
            "aspect": "16:10",
            "height": 2400,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 1080,
            "width": 2560
          },
          {
            "aspect": "21:9",
            "height": 1440,
            "width": 3440
          },
          {
            "aspect": "21:9",
            "height": 1600,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 2160,
            "width": 5120
          }
        ],
        "scales": [
          100,
          125,
          150,
          175,
          200
        ]
      },
      "lumen_on": true,
      "real": [
        {
          "height": 2160,
          "name": "DELL",
          "width": 3840,
          "x": 0,
          "y": 0
        }
      ],
      "shown": 1,
      "state": {
        "client": null,
        "driver": "0.0.0",
        "error": "",
        "installed": true,
        "physical": true,
        "settings": {
          "enabled": true,
          "virtuals": [
            {
              "attach": {
                "align": "start",
                "side": "right",
                "to": 1
              },
              "height": 1440,
              "scale": 100,
              "width": 2560
            }
          ]
        },
        "up": [
          {
            "height": 1440,
            "monitor": 2,
            "width": 2560
          }
        ]
      }
    },
    "registered": true,
    "service_error": null,
    "settings": {
      "machine": "WPC-10",
      "manual_addrs": "",
      "server": "a:7443",
      "server_ca": "",
      "server_fingerprint": ""
    },
    "status": {
      "addrs": [
        "10.0.0.1:9000"
      ],
      "client": "",
      "detail": "",
      "fps": 0,
      "mbps": 0.0,
      "server_version": "",
      "since": 0,
      "state": "online",
      "update_error": "",
      "updating": ""
    },
    "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0"
  },
  {
    "auto_update": false,
    "autostart": false,
    "defaults": {
      "machine": "WPC-10"
    },
    "host_off": false,
    "keep_awake": false,
    "lang": "en",
    "monitors": {
      "allowed": 2,
      "catalog": {
        "resolutions": [
          {
            "aspect": "16:9",
            "height": 1080,
            "width": 1920
          },
          {
            "aspect": "16:9",
            "height": 1440,
            "width": 2560
          },
          {
            "aspect": "16:9",
            "height": 1800,
            "width": 3200
          },
          {
            "aspect": "16:9",
            "height": 2160,
            "width": 3840
          },
          {
            "aspect": "16:10",
            "height": 1200,
            "width": 1920
          },
          {
            "aspect": "16:10",
            "height": 1600,
            "width": 2560
          },
          {
            "aspect": "16:10",
            "height": 2400,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 1080,
            "width": 2560
          },
          {
            "aspect": "21:9",
            "height": 1440,
            "width": 3440
          },
          {
            "aspect": "21:9",
            "height": 1600,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 2160,
            "width": 5120
          }
        ],
        "scales": [
          100,
          125,
          150,
          175,
          200
        ]
      },
      "lumen_on": true,
      "real": [
        {
          "height": 2160,
          "name": "DELL",
          "width": 3840,
          "x": 0,
          "y": 0
        }
      ],
      "shown": 1,
      "state": {
        "client": null,
        "driver": "0.0.0",
        "error": "",
        "installed": true,
        "physical": true,
        "settings": {
          "enabled": true,
          "virtuals": [
            {
              "attach": {
                "align": "start",
                "side": "right",
                "to": 1
              },
              "height": 1440,
              "scale": 100,
              "width": 2560
            }
          ]
        },
        "up": [
          {
            "height": 1440,
            "monitor": 2,
            "width": 2560
          }
        ]
      }
    },
    "registered": true,
    "service_error": null,
    "settings": {
      "machine": "WPC-10",
      "manual_addrs": "",
      "server": "a:7443",
      "server_ca": "",
      "server_fingerprint": ""
    },
    "status": {
      "addrs": [
        "10.0.0.1:9000"
      ],
      "client": "anna",
      "detail": "",
      "fps": 60,
      "mbps": 40.0,
      "server_version": "",
      "since": 0,
      "state": "session",
      "update_error": "",
      "updating": ""
    },
    "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0"
  },
  {
    "auto_update": false,
    "autostart": false,
    "defaults": {
      "machine": "WPC-10"
    },
    "host_off": false,
    "keep_awake": false,
    "lang": "en",
    "monitors": {
      "allowed": 2,
      "catalog": {
        "resolutions": [
          {
            "aspect": "16:9",
            "height": 1080,
            "width": 1920
          },
          {
            "aspect": "16:9",
            "height": 1440,
            "width": 2560
          },
          {
            "aspect": "16:9",
            "height": 1800,
            "width": 3200
          },
          {
            "aspect": "16:9",
            "height": 2160,
            "width": 3840
          },
          {
            "aspect": "16:10",
            "height": 1200,
            "width": 1920
          },
          {
            "aspect": "16:10",
            "height": 1600,
            "width": 2560
          },
          {
            "aspect": "16:10",
            "height": 2400,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 1080,
            "width": 2560
          },
          {
            "aspect": "21:9",
            "height": 1440,
            "width": 3440
          },
          {
            "aspect": "21:9",
            "height": 1600,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 2160,
            "width": 5120
          }
        ],
        "scales": [
          100,
          125,
          150,
          175,
          200
        ]
      },
      "lumen_on": true,
      "real": [
        {
          "height": 2160,
          "name": "DELL",
          "width": 3840,
          "x": 0,
          "y": 0
        }
      ],
      "shown": 1,
      "state": {
        "client": null,
        "driver": "0.0.0",
        "error": "",
        "installed": true,
        "physical": true,
        "settings": {
          "enabled": true,
          "virtuals": [
            {
              "attach": {
                "align": "start",
                "side": "right",
                "to": 1
              },
              "height": 1440,
              "scale": 100,
              "width": 2560
            }
          ]
        },
        "up": [
          {
            "height": 1440,
            "monitor": 2,
            "width": 2560
          }
        ]
      }
    },
    "registered": false,
    "service_error": null,
    "settings": {
      "machine": "WPC-10",
      "manual_addrs": "",
      "server": "",
      "server_ca": "",
      "server_fingerprint": ""
    },
    "status": {
      "addrs": [],
      "client": "",
      "detail": "",
      "fps": 0,
      "mbps": 0.0,
      "server_version": "",
      "since": 0,
      "state": "stopped",
      "update_error": "",
      "updating": ""
    },
    "token": null,
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0"
  },
  {
    "auto_update": false,
    "autostart": false,
    "defaults": {
      "machine": "WPC-10"
    },
    "host_off": true,
    "keep_awake": false,
    "lang": "en",
    "monitors": {
      "allowed": 2,
      "catalog": {
        "resolutions": [
          {
            "aspect": "16:9",
            "height": 1080,
            "width": 1920
          },
          {
            "aspect": "16:9",
            "height": 1440,
            "width": 2560
          },
          {
            "aspect": "16:9",
            "height": 1800,
            "width": 3200
          },
          {
            "aspect": "16:9",
            "height": 2160,
            "width": 3840
          },
          {
            "aspect": "16:10",
            "height": 1200,
            "width": 1920
          },
          {
            "aspect": "16:10",
            "height": 1600,
            "width": 2560
          },
          {
            "aspect": "16:10",
            "height": 2400,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 1080,
            "width": 2560
          },
          {
            "aspect": "21:9",
            "height": 1440,
            "width": 3440
          },
          {
            "aspect": "21:9",
            "height": 1600,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 2160,
            "width": 5120
          }
        ],
        "scales": [
          100,
          125,
          150,
          175,
          200
        ]
      },
      "lumen_on": true,
      "real": [
        {
          "height": 2160,
          "name": "DELL",
          "width": 3840,
          "x": 0,
          "y": 0
        }
      ],
      "shown": 1,
      "state": {
        "client": null,
        "driver": "0.0.0",
        "error": "",
        "installed": true,
        "physical": true,
        "settings": {
          "enabled": true,
          "virtuals": [
            {
              "attach": {
                "align": "start",
                "side": "right",
                "to": 1
              },
              "height": 1440,
              "scale": 100,
              "width": 2560
            }
          ]
        },
        "up": [
          {
            "height": 1440,
            "monitor": 2,
            "width": 2560
          }
        ]
      }
    },
    "registered": false,
    "service_error": "err_service_down|x",
    "settings": {
      "machine": "WPC-10",
      "manual_addrs": "",
      "server": "",
      "server_ca": "",
      "server_fingerprint": ""
    },
    "status": {
      "addrs": [],
      "client": "",
      "detail": "",
      "fps": 0,
      "mbps": 0.0,
      "server_version": "",
      "since": 0,
      "state": "stopped",
      "update_error": "",
      "updating": ""
    },
    "token": null,
    "update": {
      "error": "",
      "newer": false,
      "phase": "",
      "server_version": ""
    },
    "version": "0"
  },
  {
    "auto_update": false,
    "autostart": false,
    "defaults": {
      "machine": "WPC-10"
    },
    "host_off": false,
    "keep_awake": false,
    "lang": "en",
    "monitors": {
      "allowed": 2,
      "catalog": {
        "resolutions": [
          {
            "aspect": "16:9",
            "height": 1080,
            "width": 1920
          },
          {
            "aspect": "16:9",
            "height": 1440,
            "width": 2560
          },
          {
            "aspect": "16:9",
            "height": 1800,
            "width": 3200
          },
          {
            "aspect": "16:9",
            "height": 2160,
            "width": 3840
          },
          {
            "aspect": "16:10",
            "height": 1200,
            "width": 1920
          },
          {
            "aspect": "16:10",
            "height": 1600,
            "width": 2560
          },
          {
            "aspect": "16:10",
            "height": 2400,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 1080,
            "width": 2560
          },
          {
            "aspect": "21:9",
            "height": 1440,
            "width": 3440
          },
          {
            "aspect": "21:9",
            "height": 1600,
            "width": 3840
          },
          {
            "aspect": "21:9",
            "height": 2160,
            "width": 5120
          }
        ],
        "scales": [
          100,
          125,
          150,
          175,
          200
        ]
      },
      "lumen_on": true,
      "real": [
        {
          "height": 2160,
          "name": "DELL",
          "width": 3840,
          "x": 0,
          "y": 0
        }
      ],
      "shown": 1,
      "state": {
        "client": null,
        "driver": "0.0.0",
        "error": "",
        "installed": true,
        "physical": true,
        "settings": {
          "enabled": true,
          "virtuals": [
            {
              "attach": {
                "align": "start",
                "side": "right",
                "to": 1
              },
              "height": 1440,
              "scale": 100,
              "width": 2560
            }
          ]
        },
        "up": [
          {
            "height": 1440,
            "monitor": 2,
            "width": 2560
          }
        ]
      }
    },
    "registered": true,
    "service_error": null,
    "settings": {
      "machine": "WPC-10",
      "manual_addrs": "",
      "server": "a:7443",
      "server_ca": "",
      "server_fingerprint": ""
    },
    "status": {
      "addrs": [
        "10.0.0.1:9000"
      ],
      "client": "",
      "detail": "",
      "fps": 0,
      "mbps": 0.0,
      "server_version": "",
      "since": 0,
      "state": "starting",
      "update_error": "",
      "updating": "99.0.0"
    },
    "token": "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF",
    "update": {
      "error": "",
      "newer": true,
      "phase": "downloading",
      "server_version": "99.0.0"
    },
    "version": "0"
  }
] satisfies AppState[];
