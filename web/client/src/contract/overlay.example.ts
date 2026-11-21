// What the core really sends: written by `cargo test` (signal::contract), do not edit.
// `tsc` checks it against the type: what differs fails the build.
import type { OverlayState } from "../overlay/api";

export const examples = [
  {
    "notice": {
      "code": "two_windows_on_one_monitor",
      "serial": 1
    },
    "picture_scale": 0.5,
    "stats": {
      "audio": {
        "gaps_recent": 0,
        "gaps_span": 0,
        "lag_ms": 48.0,
        "playing": true
      },
      "budget_ms": 16.66666603088379,
      "cap_mbps": 40.0,
      "columns": [
        null,
        [
          20.0,
          5.0,
          2.0,
          false,
          2.5
        ],
        [
          20.0,
          5.0,
          2.0,
          true,
          2.5
        ]
      ],
      "decode_ms": 5.0,
      "decode_p95_ms": 5.0,
      "encode_ms": 2.5,
      "format": {
        "bit_depth": 10,
        "chroma": "4:2:0",
        "codec": "HEVC",
        "full_range": true,
        "height": 2160,
        "matrix": 1,
        "primaries": 1,
        "transfer": 1,
        "width": 3840
      },
      "fps": 60.0,
      "frames_lost_recent": 2,
      "frames_recent": 120,
      "host_fps": 60.0,
      "latency_ms": 27.0,
      "loss_percent": 0.29910269192422734,
      "net_ms": 20.0,
      "net_p95_ms": 20.0,
      "rtt_ms": null,
      "show_ms": 2.0,
      "shown_fps": 60.0,
      "video_mbps": 21.0
    },
    "stats_shown": true,
    "transfers": [
      {
        "batch": 3,
        "done": 50,
        "download": true,
        "ended_ago_ms": null,
        "image": false,
        "len": 100,
        "name": "a.mov",
        "outgoing": false,
        "reason": null,
        "set": null,
        "speed": 1.5,
        "stage": "running",
        "started_ago_ms": 1000,
        "transfer": 1
      },
      {
        "batch": null,
        "done": 50,
        "download": false,
        "ended_ago_ms": 10,
        "image": false,
        "len": 100,
        "name": "a.mov",
        "outgoing": true,
        "reason": "locked",
        "set": 4,
        "speed": 1.5,
        "stage": "cancelled",
        "started_ago_ms": 1000,
        "transfer": 1
      }
    ],
    "transfers_key": 1,
    "view": {
      "active": 1,
      "count": 3,
      "first": [
        0.0,
        0.0,
        0.5,
        0.75
      ],
      "mode": "one"
    }
  },
  {
    "notice": null,
    "picture_scale": 1.0,
    "stats": null,
    "stats_shown": false,
    "transfers": [],
    "transfers_key": 0,
    "view": {
      "active": 1,
      "count": 3,
      "first": [
        0.0,
        0.0,
        0.5,
        0.75
      ],
      "mode": "one"
    }
  }
] satisfies OverlayState[];
