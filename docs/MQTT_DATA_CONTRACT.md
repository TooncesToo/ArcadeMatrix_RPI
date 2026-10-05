🇬🇧 English | 🇫🇷 [Français](MQTT_DATA_CONTRACT_FR.md) | 🇪🇸 [Español](MQTT_DATA_CONTRACT_ES.md)

# MQTT Data contract

The **MQTT Data** engine (`mqttdata`) shows pages that any publisher sends over MQTT: a home automation system, a script, a server. This document is the contract between the publisher and the sign. It is identical for the Raspberry Pi and ESP32 firmwares.

For Home Assistant there are ready-made blueprints: see [HOME_ASSISTANT.md](HOME_ASSISTANT.md).

---

## 1. Topics and the retained snapshot

- An MQTT Data screen lists 1 to 6 **topics** (its only setting). Each topic is one page, shown in topic order.
- The publisher **must publish with `retain: true`**. The sign does not stay connected in the background: when the screen becomes active it connects to the broker set in **System → MQTT Data**, subscribes, and draws the retained message of each topic (the snapshot). While the screen is shown it stays subscribed and redraws on every new message. When the screen leaves the rotation it disconnects and frees everything.
- An **empty retained message** clears a topic: its page shows `NO DATA`.
- The publisher does all aggregation and formatting (history, rounding, units). The sign never collects points or does math.

## 2. Common fields

Every payload is one JSON object.

| Field | Required | Default | Description |
| :--- | :--- | :--- | :--- |
| `type` | **yes** | — | `value`, `table`, `graph` or `weather`. Selects the renderer. |
| `v` | no | `1` | Contract version. |
| `seconds` | no | `10` | Time on this page, minimum `3` (maximum `3600`). For `weather`, per page (NOW and each day). |

The screen's time in the rotation is the sum of its pages' `seconds`; the rotation slot's own duration is ignored. A topic with no payload yet counts as 10 s.

## 3. Page types

### `value`: one big value

| Field | Description |
| :--- | :--- |
| `value` | The value, preformatted (string; a number is accepted). Never truncated. |
| `unit` | Drawn after the value, smaller. Dropped if the value and unit don't fit. |
| `label` | Drawn above the value. `label_short` is used only when `label` doesn't fit. |
| `color` | Value colour `#RRGGBB` (default white). `label_color` default `#808080`. |

```json
{"v": 1, "type": "value", "value": "89", "unit": "°F", "label": "OUTSIDE", "color": "#FF6040"}
```

### `table`: 1 to 4 values

| Field | Description |
| :--- | :--- |
| `title` | Title row (shown when the layout has room: 1 to 3 tiles). |
| `show_title` | `false` hides the title (default `true`). |
| `tiles` | 1 to 4 objects with the `value` fields above: `label`, `label_short`, `value`, `unit`, `color`, `label_color`. Extra tiles are ignored. |

1 to 3 tiles share the width; 4 tiles form a 2x2 grid. Each tile is centred: label on top, value + unit below.

```json
{"v": 1, "type": "table", "title": "HOME", "show_title": true,
 "tiles": [{"label": "DOWNSTAIRS", "label_short": "DOWN", "value": "77.9", "unit": "°F"},
           {"label": "UPSTAIRS", "label_short": "UP", "value": "76.1", "unit": "°F"}]}
```

### `graph`: bars and lines

| Field | Description |
| :--- | :--- |
| `title`, `title_short` | Header title (the short form only when the full one doesn't fit). |
| `summary`, `summary_color` | Right side of the header (default colour: the first series'). |
| `show_header` | `false` hides the header (default `true`). |
| `series` | Up to 4: `label`, `color`, `style` (`bar` default, or `line`), `data` (numbers, `null` = missing). |
| `slots` | Number of x positions (default: the longest series). Data is right-aligned: the last point is the newest. |
| `min`, `max` | y range. Absent: automatic (lines: data range plus 5 %). |
| `stack` | `true` (default) stacks bar series; `false` overlays them. Bars grow from zero (negatives go down). |
| `yaxis` | `true`: min / max labels at the left (64 px tall panels). |
| `bands` | Up to 96 `{from, to, color}` background column ranges (slot indices); with more, the newest are kept. |
| `marks` | `{at, color}`: dotted vertical line at slot `at`. |
| `legend` | Up to 4 `{text, color}` words along the bottom (64 px tall panels only). |

```json
{"v": 1, "type": "graph", "title": "OUTSIDE VS DOWN", "summary": "89°", "summary_color": "#FF4020",
 "slots": 96, "min": 76, "max": 90, "yaxis": true,
 "series": [{"label": "OUTSIDE", "style": "line", "color": "#FF4020", "data": [88.5, 89.0, null, 89.0]},
            {"label": "DOWNSTAIRS", "style": "line", "color": "#40A0FF", "data": [77.6, 77.9, 77.9, 77.9]}],
 "marks": [{"at": 48, "color": "#404040"}],
 "legend": [{"text": "OUTSIDE", "color": "#FF4020"}, {"text": "DOWNSTAIRS", "color": "#40A0FF"}]}
```

### `weather`: live conditions and forecast

| Field | Description |
| :--- | :--- |
| `units` | `imperial` (°F; the high is drawn above the low) or `metric` (°C). The sign does not convert. |
| `current` | `temp`, `condition`, `humidity`, `wind`, `wind_unit`, `wind_dir` (`N`, `NE`, ... `NW`). |
| `days` | Up to 5: `temp_max`, `temp_min`, `condition`. |

The page expands to a **NOW** page (the live `current.temp`, then humidity and wind, e.g. `41%  NE 9mph`) followed by one page per day. Conditions: `sunny`, `clear-night`, `partlycloudy`, `cloudy`, `fog`, `exceptional`, `rainy`, `pouring`, `lightning`, `lightning-rainy`, `snowy`, `snowy-rainy`, `hail`, `windy`, `windy-variant`; anything else is shown as sent.

```json
{"v": 1, "type": "weather", "units": "imperial",
 "current": {"temp": 90, "condition": "windy", "humidity": 30, "wind": 6, "wind_unit": "mph", "wind_dir": "SE"},
 "days": [{"temp_max": 95, "temp_min": 72, "condition": "sunny"},
          {"temp_max": 93, "temp_min": 71, "condition": "partlycloudy"}]}
```

## 4. Limits

| | Raspberry Pi | ESP32 with PSRAM | ESP32 without PSRAM |
| :--- | :--- | :--- | :--- |
| Payload size | 8 KB | 8 KB | 4 KB |
| Points per series | 288 | 288 | 96 |
| Topics per screen | 6 | 6 | 4 |
| Series / tiles / legend items / bands | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 |

Payloads over the limit are ignored (the page keeps its previous content). Extra series, tiles, points or bands are dropped.

## 5. Versioning and compatibility

- Unknown fields are ignored, so publishers can add fields freely.
- A payload with a higher `v` than the firmware knows still renders every field the firmware understands.
- New page types will get new `type` names; older firmware shows them as `UNSUPPORTED`.

## 6. Notices

| Notice | Meaning |
| :--- | :--- |
| `CONNECTING` | The screen just became active and is connecting. |
| `NO CONNECTION` | The broker can't be reached (check System → MQTT Data). |
| `NO DATA` | The topic has no retained message (3 s after connecting), or an empty one. |
| `UNSUPPORTED` | `type` is missing or unknown, required fields are missing, or the payload is not JSON. Shown for the page's `seconds`; other pages are unaffected. |
