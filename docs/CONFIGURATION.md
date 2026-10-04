🇬🇧 English | 🇫🇷 [Français](CONFIGURATION_FR.md) | 🇪🇸 [Español](CONFIGURATION_ES.md)

# Detailed Configuration (config.json) - Raspberry Pi

The configuration system relies exclusively on a single `config.json` file located at the root of the project (or on the **DATA** partition of the pre-built image). It handles the hardware driver, network, MQTT integration, system behaviour, API security, and the decoupled logic of each engine ("instances").

> The legacy `conf.ini` format has been fully removed. `config.json` is now the **single source of truth**. On boot the file is validated and self-healed (see §8), so a partial or hand-edited file is safe: missing keys are re-created with their defaults.

---

## 1. Global Structure

```json
{
  "matrix": { ... },
  "wifi": { ... },
  "mqtt": { ... },
  "data_mqtt": { ... },
  "system": { ... },
  "instances": [ ... ],
  "rotation": [ ... ],
  "api_auth_enabled": false,
  "api_token": ""
}
```

---

## 2. The `"matrix"` Block (Hardware Driver)

This block configures the DMA parameters for the hzeller `rpi-rgb-led-matrix` library. Changing any hardware value triggers an automatic restart so the new driver settings take effect.

| Key | Type | Description |
| :--- | :--- | :--- |
| `width` | `int` | Width of a single panel (e.g., `64`). |
| `height` | `int` | Height of a single panel (e.g., `32`). |
| `chain_length` | `int` | Number of panels chained horizontally. |
| `mapping` | `String` | GPIO wiring/mapping (`regular`, `adafruit-hat`, `adafruit-hat-pwm`, ...). |
| `driver_chip` | `String` | Controller chip (`SHIFTREG`, `FM6126A`). |
| `rgb_sequence` | `String` | Color order (`RGB`, `RBG`, `BGR`, ...). Fix swapped colors here. |
| `slowdown` | `int` | GPIO slowdown (`1`–`4`). Increase on Pi 3/4 if you see artifacts. |
| `pwm_bits` | `int` | Color depth. Default `11`; lower to `8` to save CPU. |
| `pwm_lsb_nanoseconds` | `int` | LSB pulse width tuning (advanced). |
| `disable_hardware_pulsing` | `bool` | Set `true` to stop DMA starving the internal Wi-Fi (slight flicker). |
| `limit_refresh_rate_hz` | `int` | Cap the refresh rate (`0` = uncapped). |
| `row_address_mode` | `int` | Row addressing type for exotic panels (`0` default). |
| `multiplexing` | `int` | Panel multiplexing type (`0` default). |
| `panel_type` | `String` | Optional panel init string (e.g. `FM6126A`), usually empty. |
| `slot_transition` | `String` | Effect played when the rotation moves to the next slot (`wipe`, `curtain`, `shutter`, `dissolve`, `checker`, `matrix`, `slide`, `zoom`, `glitch`, `random`, `none`). Default `none`. |
| `slot_transition_duration_ms` | `int` | Slot transition duration in milliseconds (`100`-`3000`, default `500`). |

> Live daytime brightness is **not** stored in this block; it is controlled at runtime from the Web UI (Dashboard slider → `POST /api/system { "brightness_limit": 0-100 }`). Night brightness lives in the `system` block (§4).

---

## 3. The `"wifi"` Block

| Key | Type | Description |
| :--- | :--- | :--- |
| `ssid` | `String` | The name of your Wi-Fi network. |
| `password` | `String` | The WPA2 key. |
| `hostname` | `String` | Device hostname advertised on the network. |
| `configured` | `bool` | Set to `false` to force a (re)connection attempt on next boot. Set back to `true` automatically on success. |
| `disable_internal` | `bool` | If using an external USB dongle, disable the Pi's internal Wi-Fi (changing this triggers a restart). |

You can also push credentials at runtime with `POST /api/wifi { "ssid": "...", "password": "..." }`, which sets `configured=false` and restarts the network provisioning.

---

## 4. The `"system"` Block (Environment & Standby)

| Key | Type | Description |
| :--- | :--- | :--- |
| `timezone` | `String` | POSIX string (e.g., `CET-1CEST,M3.5.0,M10.5.0/3`). |
| `format_24h` | `bool` | Time format. `true` = 23:00, `false` = 11:00 PM. |
| `lang` | `String` | System language (e.g., `en`, `fr`, `es`). |
| `night_mode_enabled` | `bool` | Enables automatic turn-off / brightness reduction at night. |
| `turn_off_at` | `String` | Standby start time (e.g., `"23:00"`). |
| `wake_up_at` | `String` | Wake-up time (e.g., `"07:00"`). |
| `night_brightness` | `int` | Standby brightness (`0` = matrix completely off). |
| `day_brightness` | `int` | Live daytime brightness (`0`–`100`). Set from the dashboard slider and persisted across restarts. |
| `idle_fighter_enabled` | `bool` | Master switch for the decorative Fighter overlay composited on top of idle rotation screens (per-screen opt-in via each rotation entry). |
| `idle_fighter_interval` | `int` | Seconds between two fight animations (minimum `1`). |

---

## 5. The `"mqtt"` Block (Recalbox / Batocera Marquees)

| Key | Type | Description |
| :--- | :--- | :--- |
| `enabled` | `bool` | Enable the MQTT listener for Pixelcade-style marquees. |
| `broker` | `String` | Broker IP/host (usually the Pi itself). |
| `port` | `int` | Broker port (default `1883`). |
| `user` | `String` | Broker username (optional). |
| `pass` | `String` | Broker password (optional). |
| `device_name` | `String` | Identifier published by this device. |
| `allow_overlay` | `bool` | Allow decorative overlays (e.g. Street Fighter) on top of MQTT/marquee screens (default `false`). |
| *(auto-subscription)* | `system/playing/#` | Subscribes automatically to all supported retro gaming systems: `system/playing/recalbox`, `system/playing/batocera`, `system/playing/retropie`. |

The sync daemon can be installed on the console (Recalbox, Batocera, RetroPie) over SSH from the Web UI (`POST /api/mqtt/install`) with target OS selection or auto-detection, and its logs fetched with `POST /api/mqtt/logs`.

> [!NOTE]
> For Batocera, version **v33 or newer** is required for dynamic marquee browsing (`game-selected` and `system-selected` hooks). Batocera v32 and earlier only trigger game launch/stop events. Recalbox is supported on all versions.

---

## 5b. The `"data_mqtt"` Block (Data Feed: Home Assistant, Node-RED...)

A second MQTT connection, independent of the `"mqtt"` block above. It **never takes over the display**: it only keeps the last payload of every topic under `<topic_prefix>/#` so that engines such as `graph` can draw it. Use it to show Home Assistant data (energy, cooling history, temperatures) on the matrix without touching the marquee setup.

| Key | Type | Description |
| :--- | :--- | :--- |
| `enabled` | `bool` | Enable the data feed client (default `false`). |
| `broker` | `String` | Broker host or IP, e.g. the Home Assistant Mosquitto add-on. Empty = off. |
| `port` | `int` | Broker port (default `1883`). |
| `user` | `String` | Broker username (optional). |
| `pass` | `String` | Broker password (optional). |
| `topic_prefix` | `String` | The client subscribes to `<topic_prefix>/#` (default `arcadematrix`). |

- The MQTT client id is `<hostname>-data`, so several signs can share one broker.
- Up to 8 topics are kept, 8192 bytes per payload (larger payloads are dropped). Publish **retained** so the value is there right after a restart.
- `GET /api/stats` reports `data_mqtt_connected` and `data_mqtt_topics`.
- Changing this block restarts the application, like the `"mqtt"` block.

## 6. API Security (`api_auth_enabled` / `api_token`)

These two top-level keys secure the write/administrative endpoints.

| Key | Type | Description |
| :--- | :--- | :--- |
| `api_auth_enabled` | `bool` | If `true`, sensitive endpoints require the `X-API-Token` header to match `api_token`. |
| `api_token` | `String` | Secret token (auto-generated on first boot). Sent by the Web UI as `X-API-Token`. |

Disabled by default so the bundled Web UI works out of the box. Enable it if the device is reachable beyond a trusted LAN.

---

### Clock instance options

Beyond `theme`, `font`, `size` and the colours, a clock instance takes:

| Key | Type | Description |
| :--- | :--- | :--- |
| `clock_glow` | `enum` | Outline around the digits: `0` off, `1` neon (the chosen colour at full strength with a near-white centre, as the Matrix face draws it), `2` a colour of its own. Default `0`. |
| `clock_glow_color` | `color` | Outline colour when `clock_glow` is `2` (default `#00FF41`). |

Themes `30`-`34` and `37` are faces adapted from the Clockwise clockfaces. Those named `(256x64)` are
laid out for a wide panel and show a notice on smaller ones.

---

## 7. Engines: `"instances"` & `"rotation"`

The decoupled architecture lets you create multiple independent, differently-configured copies of the same Engine.

### `"instances"`
An array holding the configuration of each logical block.

```json
{
  "instance_id": "crypto_main",
  "engine_id": "crypto",
  "config": {
    "symbols": "BTC,ETH,SOL"
  }
}
```
* `instance_id`: Unique name of this block.
* `engine_id`: The internal identifier of the Rust Engine (must be a registered engine — see §9).
* `config`: A flat map of `String` values specific to the engine, validated against its `ConfigSchema`.

Editing an instance through the Web UI (`POST /api/instances`) is applied **live, without a restart**: the runtime calls the engine's `on_config_changed()` on the next frame (Lazy-Once hot-reload). Adding or removing an instance resets the rotation cleanly.

### `"rotation"`
Defines the display order, per-slot duration, and transverse overlay toggles.

```json
{
  "instance_id": "crypto_main",
  "duration_sec": 30,
  "overlays": {
    "fighter": true
  }
}
```
* `instance_id`: Target engine instance.
* `duration_sec`: Dwell duration in seconds (or playback budget for self-paced engines like GIF).
* `overlays.fighter`: (`bool`) Granular toggle for the M.U.G.E.N Fighter decorative overlay on this specific screen.

Only instances listed here are ever initialized, saving memory for unused features. The rotation is editable from the Web UI **Rotation** panel (`GET`/`POST /api/rotation`).

> **Human-Readable Persistence**: When saved to disk (`config.json`), the file is always written formatted and indented (`to_string_pretty`) so it can be cleanly inspected and edited by hand without breaking.

---

## 8. Self-Healing Validation

On every boot **and** on every write via `POST /api/instances`, the `ConfigSanitizer` reconciles each instance against its engine `ConfigSchema`:

* **Missing key** → the schema `default_value` is injected.
* **Integer / Float** → parsed and, if out of `min`/`max`, clamped or reset to default (per the field's `validation_policy`).
* **Boolean** → normalized (`true/1/yes/on` → `true`, `false/0/no/off` → `false`); an unparseable value falls back to the default.
* **Options** → the value must be one of the declared options (comma-separated list for multi-select); otherwise it falls back to the default.
* **Obsolete keys** → keys no longer present in the schema (e.g. after an OTA that renamed a field) are pruned.

The result is saved atomically, so an OTA that adds a new field self-populates it without any user intervention.

---

## 9. Engine Configurations

Each engine advertises its own fields through its `ConfigSchema` (discoverable at `GET /api/engines`, which is what powers the dynamic Web UI). The most common engines:

### Engine: `clock`
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `theme` | `int` | `0` | Animated clock theme index: `0` = Digital Standard, `1` = Flip Clock, `2` = Cyberpunk, `3` = Word Clock, `4` = Binary Clock, `5` = Pac-Man, `6` = Tetris, `7` = Slot Machine, `8` = Versus (M.U.G.E.N), `9` = Pong, `10` = Matrix Rain (Katakana). |
| `format` | `String` | `%H:%M:%S` | strftime time format. |
| `font` | `String` | `PressStart2P.ttf` | Font file from `/fonts/`. |
| `size` | `int` | `2` | Font scaling factor. |
| `color_1` | `String` | `#FFFFFF` | Primary hex color (gradient start on Custom theme). |
| `color_2` | `String` | `#FFFFFF` | Secondary hex color (gradient end on Custom theme). |
| `offset_x` | `int` | `0` | Horizontal pixel offset. |
| `offset_y` | `int` | `0` | Vertical pixel offset. |

### Engine: `date`
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `theme` | `int` | `0` | Date theme index. |
| `format` | `String` | `%d/%m` | strftime date format. |
| `font` | `String` | `PressStart2P.ttf` | Font file from `/fonts/`. |
| `size` | `int` | `2` | Font scaling factor. |
| `color_1` | `String` | `#FFFFFF` | Primary hex color. |
| `color_2` | `String` | `#FFFFFF` | Secondary hex color. |
| `offset_x` | `int` | `0` | Horizontal pixel offset. |
| `offset_y` | `int` | `0` | Vertical pixel offset. |

### Engine: `dashboard` (Smart Dashboard Hub)
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `clock_mode` | `Options` | `0` | `0` (Digital), `1` (Watch Dial), `2` (Minimal) | Clock rendering face and dial style. |
| `theme` | `Options` | `0` | `0` (Cyberpunk Neon), `1` (Amber HUD), `2` (Minimalist Luxury), `3` (Matrix Phosphor) | Color palette for dashboard widgets and borders. |
| `show_clock` | `bool` | `true` | `true`, `false` | Display primary time widget. |
| `show_world_clock` | `bool` | `true` | `true`, `false` | Display secondary world timezones (NYC, TYO, LON...). |
| `world_clocks` | `String` | `NYC,TYO,LON` | Airport/City codes | Comma-separated list of secondary timezone codes. |
| `show_weather` | `bool` | `true` | `true`, `false` | Display outdoor temperature and weather forecast. |
| `weather_city` | `String` | `Paris, FR` | Text | City location for weather forecast. |
| `show_markets` | `bool` | `true` | `true`, `false` | Display real-time crypto and stock ticker badges. |
| `tracked_markets` | `String` | `BTC,ETH,SOL,NVDA` | Comma-separated | Crypto and stock ticker symbols to cycle through. |
| `show_sysinfo` | `bool` | `true` | `true`, `false` | Display CPU & RAM usage vitals. |
| `show_date` | `bool` | `true` | `true`, `false` | Display calendar date badge. |
| `show_seconds` | `bool` | `true` | `true`, `false` | Display second digits or sweeping tick. |
| `offset_x` | `int` | `0` | `-64` to `64` | Horizontal pixel offset. |
| `offset_y` | `int` | `0` | `-32` to `32` | Vertical pixel offset. |

### Engine: `crypto`
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `symbols` | `String` | `BTC,ETH` | Comma-separated | Crypto symbols to monitor (CoinGecko / Binance). |
| `currency` | `Options` | `USD` | `USD`, `EUR`, `GBP`, `JPY` | Currency comparison and display symbol (`$`, `€`, `£`, `¥`). |
| `show_chart` | `bool` | `true` | `true`, `false` | Display historical price sparkline chart. |
| `chart_timeframe` | `Options` | `daily` | `hourly`, `daily`, `weekly`, `monthly` | Timeframe for historical price series. |
| `page_seconds` | `int` | `5` | `3` to `30` | Seconds to dwell on each page. |
| `cache_ttl_min` | `int` | `1` | `1` to `60` | Minutes to cache quote price. |

### Engine: `stock`
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `symbols` | `String` | `AAPL,NVDA,TSLA` | Comma-separated | Stock ticker symbols to monitor (Yahoo Finance). |
| `currency` | `Options` | `USD` | `USD`, `EUR`, `GBP`, `JPY` | Currency comparison and display symbol (`$`, `€`, `£`, `¥`). |
| `show_chart` | `bool` | `true` | `true`, `false` | Display historical price sparkline chart. |
| `chart_timeframe` | `Options` | `daily` | `hourly`, `daily`, `weekly`, `monthly` | Timeframe for historical price series. |
| `page_seconds` | `int` | `5` | `3` to `30` | Seconds to dwell on each page. |
### Engine: `gnews` (GNews Live Feed & Breaking News Ticker)
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `api_key` | `String` | `""` | Any valid key | GNews.io API key (optional; uses demo articles if empty). |
| `category` | `Options` | `technology` | `general`, `world`, `nation`, `business`, `technology`, `entertainment`, `sports`, `science`, `health` | Primary news category. |
| `keywords` | `String` | `""` | Text / Query | Custom search query or filter tags (e.g. `ai OR arcade`). |
| `lang` | `Options` | `auto` | `auto`, `en`, `fr`, `es`, `de`, `it`, `pt`, `nl`, `ru`, `zh`, `ja` | Article language (`auto` syncs with system language). |
| `country` | `Options` | `auto` | `auto`, `us`, `fr`, `gb`, `es`, `de`, `ca`, `it`, `jp`, `au`, `br`, `in` | Country edition (`auto` uses local region). |
| `max_articles` | `int` | `5` | `3` to `15` | Maximum number of headlines cached and rotated per cycle. |
| `cache_ttl_min` | `int` | `30` | `5` to `120` | Cache refresh interval in minutes between background API queries. |
| `display_mode` | `Options` | `smooth_scroll` | `smooth_scroll`, `serpentine`, `vertical_crawl`, `static_paged` | Animation mode (smooth right-to-left ticker, arcade serpentine snake flow, vertical crawl, or static paged word wrapping). |
| `scroll_speed` | `int` | `3` | `1` to `5` | Ticker scrolling speed multiplier (1: Slow ~18 px/s to 5: Turbo ~60 px/s). |
| `scroll_pause_start_ms` | `int` | `1200` | `0` to `4000` | Initial pause dwell time (ms) at headline start before scrolling. |
| `scroll_pause_end_ms` | `int` | `1000` | `0` to `4000` | Pause dwell time (ms) at end of headline before transitioning. |
| `article_duration_sec` | `int` | `12` | `5` to `60` | Display duration per article in seconds. |
| `theme` | `Options` | `category_dynamic` | `category_dynamic`, `breaking_crimson`, `cyberpunk`, `monochrome_paper` | Color palette scheme. |
| `show_category_badge` | `bool` | `true` | `true`, `false` | Display color-coded category pill (`[TECH]`, `[WORLD]`, etc.). |
| `show_source` | `bool` | `true` | `true`, `false` | Display news source name badge (`BBC News`, `Reuters`, etc.). |
| `show_time_ago` | `bool` | `true` | `true`, `false` | Display relative time badge (`5m ago`, `2h ago`). |
| `show_beacon` | `bool` | `true` | `true`, `false` | Display pulsing live broadcast beacon dot. |
| `show_progress_dots` | `bool` | `true` | `true`, `false` | Display headline index dots (`● ○ ○ ○ ○`). |

### Engine: `weather`
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `source` | `Options` | `owm` | `owm`, `ha` | `owm` = OpenWeatherMap over HTTP. `ha` = "Home Assistant (MQTT)": a retained payload on the `data_mqtt` broker; `api_key`, `city` and `units` are then hidden and unused. |
| `topic` | `String` | `arcadematrix/weather/local` | Text | Only with `source = ha`: topic of the weather payload. |
| `api_key` | `String` | `""` | Free API key | Your OpenWeatherMap API Key (free tier at [openweathermap.org](https://home.openweathermap.org/users/sign_up)). |
| `city` | `String` | `""` | Text | City location (see formatting guide below). |
| `units` | `Options` | `metric` | `metric`, `imperial` | Temperature unit: `metric` for Celsius (°C) or `imperial` for Fahrenheit (°F). |
| `lang` | `Options` | `en` | `en`, `fr`, `es` | Language code for day labels (TODAY / AUJ. / HOY). |
| `offset_x` | `int` | `0` | `-64` to `64` | Horizontal pixel shift. |
| `offset_y` | `int` | `0` | `-32` to `32` | Vertical pixel shift. |

With `source = ha` the sign does no HTTP fetch at all. Payload (temperatures already in the unit HA uses; the sign never converts):

```json
{"v":1,"units":"imperial",
 "current":{"temp":75,"condition":"clear-night","humidity":47,"wind":7,"wind_unit":"mph","wind_dir":"NE"},
 "days":[{"temp_max":89,"temp_min":70,"condition":"sunny"},
         {"temp_max":90,"temp_min":71,"condition":"partlycloudy"}]}
```

The cycle starts with a NOW page: the live `current.temp`, then humidity and wind (`current.humidity`, `current.wind_dir`, `current.wind`, `current.wind_unit`, e.g. "41%  NE 9mph"; the direction is dropped first if the line doesn't fit). `units`: `imperial` = °F labels, `metric` = °C. Day 0 uses the live `current.temp` as its temperature; up to 5 days are used. HA conditions (sunny, clear-night, partlycloudy, cloudy, fog, exceptional, rainy, pouring, lightning, lightning-rainy, snowy, snowy-rainy, hail, windy, windy-variant) map to the existing icons and get translated labels.

In °F the high is shown above the low (US convention).

#### How to Format the `city` Field on OpenWeatherMap
OpenWeatherMap uses the ISO 3166 country code (and 2-letter state code for the US) to disambiguate locations:
* **International Locations:** Use `City,CountryCode` (e.g. `Paris,FR`, `London,GB`, `Tokyo,JP`, `Montreal,CA`).
* **United States Locations:** Use `City,StateCode,CountryCode` (e.g. `Tucson,AZ,US`, `Miami,FL,US`, `Dallas,TX,US`). Specifying only the city or omitting the country may return an incorrect city with the same name.
* **Where to Look:** Go to [openweathermap.org](https://openweathermap.org), search for your city. The top search result header and URL show the exact `City,State,Country` string recognized by the API.

### Engine: `sysinfo` (System Monitor)
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `theme` | `int` | `0` | `0` to `2` | Visual theme: `0` = Color Gauge Bars, `1` = Compact 4-Block Grid, `2` = Retro Terminal. |
| `show_cpu` | `bool` | `true` | `true`, `false` | Display real-time CPU usage percentage (CPU %). |
| `show_ram` | `bool` | `true` | `true`, `false` | Display real-time RAM usage percentage (RAM %). |
| `show_temp` | `bool` | `true` | `true`, `false` | Display hardware SoC temperature (dynamic green/amber/red color scale). |
| `show_uptime` | `bool` | `true` | `true`, `false` | Display system Uptime in hours/days. |
| `temp_unit` | `Options` | `C` | `C`, `F` | Temperature unit: Celsius (`C`) or Fahrenheit (`F`). |
| `offset_x` | `int` | `0` | `-64` to `64` | Horizontal pixel offset. |
| `offset_y` | `int` | `0` | `-32` to `32` | Vertical pixel offset. |

### Engine: `fighter` (M.U.G.E.N Combat)
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `mode` | `Options` | `match` | `match`, `showcase` | Battle mode: `match` (full battle with K.O. & victory) or `showcase` (continuous demonstration). |
| `fighter_1` | `String` | `""` | Character folder name | P1 Fighter (leave blank for random selection). |
| `fighter_2` | `String` | `""` | Character folder name | P2 Fighter (leave blank for random selection). |
| `show_hud` | `bool` | `true` | `true`, `false` | Display retro HP health bars, Super gauges, and fighter names. |
| `match_duration` | `int` | `30` | `10` to `120` | Maximum round duration in seconds before timeout. |

### Engine: `dashboard` (Smart Dashboard Hub)
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `clock_mode` | `Options` | `0` | `0` (Digital), `1` (Watch Dial), `2` (Minimal) | Display clock style |
| `theme` | `Options` | `0` | `0` (Cyberpunk), `1` (Amber HUD), `2` (Minimalist), `3` (Matrix) | Color palette for widgets |
| `show_clock` | `bool` | `true` | `true`, `false` | Display main clock |
| `show_world_clock` | `bool` | `true` | `true`, `false` | Display secondary world timezones |
| `world_clocks` | `String` | `NYC,TYO,LON` | Airport codes | Comma-separated world timezones (e.g. `NYC,TYO,LON,PAR,SFO`) |
| `show_weather` | `bool` | `true` | `true`, `false` | Display live weather widget |
| `weather_city` | `String` | `Paris,FR` | City string | City query for OpenWeatherMap |
| `show_markets` | `bool` | `true` | `true`, `false` | Display rolling crypto/stock market ticker |
| `tracked_markets` | `String` | `BTC,ETH,NVDA,AAPL` | Symbols | Comma-separated ticker symbols |
| `show_sysinfo` | `bool` | `true` | `true`, `false` | Display CPU % / RAM % vitals |
| `show_date` | `bool` | `true` | `true`, `false` | Display current date |
| `show_seconds` | `bool` | `true` | `true`, `false` | Display seconds counter |

### Engine: `google_cast` (Google Home / Nest Audio)
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `device_ip` | `String` | `""` | Static IP of your Google Home / Nest Audio speaker. Leave blank for automatic mDNS local discovery. |
| `device_name` | `String` | `""` | Device name filter (e.g. `Living Room`) when automatically scanning your LAN. |
| `show_album_art` | `bool` | `true` | Downloads and displays album artwork on the left of the LED matrix. |
| `show_progress` | `bool` | `true` | Displays real-time playback progress bar at the bottom. |
| `show_visualizer` | `bool` | `true` | Displays animated audio frequency equalizer during playback. |
| `show_volume` | `bool` | `true` | Displays current Google Nest speaker volume level. |

### Engine: `spotify` (Spotify Official Player)
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `client_id` | `String` | `""` | Your Spotify Developer API Client ID. |
| `client_secret` | `String` | `""` | Your Spotify Developer API Client Secret (optional for PKCE). |
| `refresh_token` | `String` | `""` | Your Spotify OAuth2 Refresh Token for seamless ongoing playback sync. |
| `show_album_art` | `bool` | `true` | Downloads and displays full-color Spotify album artwork. |
| `show_progress` | `bool` | `true` | Displays track playback time progress bar at the bottom. |
| `show_visualizer` | `bool` | `true` | Displays animated audio equalizer during playback. |
| `show_volume` | `bool` | `true` | Displays active Spotify volume percentage. |

### Engine: `gifs`
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `playlists` | `String` (Multi) | `""` | Options from `/api/playlists` | Active GIF playlists or folders to cycle through (comma-separated). |

### Engine: `message`
| Field | Type | Default | Options | Description |
| :--- | :--- | :--- | :--- | :--- |
| `text` | `String` | `Hello` | Text | The text banner or message to display. |
| `color` | `String` | `#ffffff` | Hex Color | Text color in `#RRGGBB` format. |
| `size` | `int` | `1` | `1` to `4` | Font scaling multiplier. |
| `direction` | `Options` | `left` | `left`, `none` | Scroll direction (`left` for leftward scrolling, `none` for centered static text). |
| `speed` | `int` | `50` | `10` to `200` | Milliseconds per scroll step (lower is faster; ignored when static). |
| `font` | `String` | `Default` | Dynamic | Font file from `/fonts/`. |

### Engine: `marquee`
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| *(auto)* | `None` | — | Internal Pixelcade/Recalbox/Batocera marquee sync engine. Displays scraped game box-art and marquees received via MQTT / Webhook. |

### Engine: `graph` (Data Feed Graph)
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `topic` | `String` | `arcadematrix/graph/ac` | Full topic of the graph payload on the `data_mqtt` broker. |
| `show_header` | `Boolean` | `true` | Title on the left and summary on the right above the plot. |

Draws bars (stacked or overlaid) from a JSON payload. 128x32: 5x7 header on rows 0-6, plot on rows 8-31. 256x64: double-size header, plot on rows 15-63 with a dim 50 % gridline. Shows `NO DATA` until the first payload arrives.

Payload (v1):

```json
{
  "v": 1,
  "title": "AC 24H",
  "summary": "12.3kWh",
  "unit": "kW",
  "slots": 96,
  "max": 6,
  "stack": true,
  "series": [
    {"label": "UP", "color": "#2080FF", "data": [0.0, 1.25, 2.4]},
    {"label": "DN", "color": "#00D0A0", "data": [0.0, 0.8, 1.1]}
  ],
  "bands": [{"from": 60, "to": 72, "color": "#301800"}],
  "marks": [{"at": 28, "color": "#404040"}]
}
```

| Key | Description |
| :--- | :--- |
| `slots` | Number of x positions (default: longest series). Data is **right-aligned**: the last value is the newest, drawn at the right edge. |
| `series` | Up to 4 series of up to 288 numbers. `null` or non-numbers count as `0`. |
| `max` | Top of the y scale. Absent or `<= 0` = automatic (highest stacked total). |
| `stack` | `true`: series stacked bottom-up in order. `false`: overlaid, later series on top. |
| `bands` | Background column ranges in slot indices `[from, to)`, e.g. on-peak hours. Up to 96 bands, each with its own colour; with more, the newest (highest `from`) are kept. |
| `marks` | Dotted vertical line at the left edge of slot `at`, e.g. midnight. |
| `min` | Bottom of the y scale (default `0`). With `max` absent or `<= min` the top is automatic. With any line series the automatic range is the data range padded by 5 % (at least 0.5) each side. |
| `style` (per series) | `"bar"` (default) or `"line"`. Bars grow from the zero line (negatives go down). Lines are 1 px steps drawn on top; `null` leaves a gap. |
| `yaxis` | `true`: min and max printed at the left edge in a 3x5 font (64 px tall panels only). |
| `summary_color` | Optional `"#RRGGBB"` for the header summary (default: the first series' colour). |
| `legend` | Optional, up to 4 `{"text","color"}` items drawn in a 3x5 font along the bottom edge, only on panels at least 64 px tall. Items that don't fit are dropped from the end. |
| colors | `"#RRGGBB"`. |

Home Assistant example (an automation action publishing a retained payload every 15 minutes):

```yaml
- action: mqtt.publish
  data:
    topic: arcadematrix/graph/ac
    retain: true
    payload: >-
      {{ {"v": 1, "title": "AC 24H", "unit": "kW", "slots": 96, "max": 6,
          "series": [{"label": "AC", "color": "#2080FF",
                      "data": state_attr('sensor.ac_history', 'values')}]} | to_json }}
```

### Engine: `values` (Data Feed Values)
| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `topic` | `String` | `arcadematrix/values/energy` | Full topic of the values payload on the `data_mqtt` broker. |
| `show_title` | `Boolean` | `true` | Title row on top when the layout has room (1 to 3 tiles). |

1 to 4 tiles of preformatted text: the sign never does math.

```json
{"v":1,"title":"HOME",
 "tiles":[{"label":"DOWNSTAIRS","label_short":"DOWN","value":"76","unit":"°F","color":"#2060FF"},
          {"label":"AC 24H","value":"17.0","unit":"h","color":"#FFB000","label_color":"#808080"}]}
```

Each tile is stacked and centred in its cell: label on top, value + unit below. 1-3 tiles share the width (value as large as fits); 4 tiles form a 2x2 grid with no title. An optional `label_short` is used only when the full `label` doesn't fit (likewise `title_short` for the `graph` engine). A value too wide for its cell drops its unit; values are never truncated. Shows `NO DATA` until the first payload.

---

*Note: All schemas can also be queried dynamically in JSON format from the running system at `GET /api/engines`.*
