// What the core really sends: written by `cargo test` (signal::contract), do not edit.
// `tsc` checks it against the type: what differs fails the build.
import type { VideoStatus } from "../api";

export const examples = [
  {
    "event": "connecting"
  },
  {
    "event": "waiting",
    "reason": "host offline"
  },
  {
    "event": "reconnecting",
    "reason": "lost"
  },
  {
    "adapter": "WireGuard",
    "client_net": "10.0.0.0/24",
    "event": "streaming",
    "host": "10.0.0.1:9000",
    "host_net": "10.0.0.0/24",
    "via": "10.0.0.2"
  },
  {
    "event": "streaming",
    "host": "172.16.0.1:9000",
    "host_net": "172.16.0.0/24"
  },
  {
    "client_nets": [
      "10.0.0.0/24"
    ],
    "event": "unreachable",
    "host_nets": [
      "10.0.0.0/24",
      "192.168.100.0/24",
      "172.16.0.0/24"
    ],
    "routes": [
      {
        "adapter": "WireGuard",
        "default": false,
        "host": "10.0.0.1:9000",
        "via": "10.0.0.2"
      },
      {
        "adapter": "Wi-Fi",
        "default": true,
        "host": "192.168.100.41:9000",
        "via": "192.168.0.5"
      },
      {
        "host": "172.16.0.1:9000"
      }
    ]
  },
  {
    "ask": 1,
    "event": "monitor_wanted",
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
    ],
    "suggest": {
      "height": 1600,
      "scale": 150,
      "width": 2560
    }
  },
  {
    "event": "no_monitors"
  }
] satisfies VideoStatus[];
