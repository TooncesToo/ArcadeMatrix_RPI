🇬🇧 English | 🇫🇷 [Français](HOME_ASSISTANT_FR.md) | 🇪🇸 [Español](HOME_ASSISTANT_ES.md)

# Home Assistant integration

Home Assistant can feed the sign's **MQTT Data** screens with ready-made blueprints in [`tools/home_assistant/blueprints/`](../tools/home_assistant/blueprints/). They publish retained messages that follow the [MQTT Data contract](MQTT_DATA_CONTRACT.md).

## 1. Prerequisites

- An MQTT broker reachable by Home Assistant and the sign (for example the **Mosquitto broker** add-on) and the **MQTT** integration set up in Home Assistant.
- On the sign, **System → MQTT Data**: the broker address, port and a login. A dedicated Home Assistant user works well, since the Mosquitto add-on accepts Home Assistant users.

## 2. The blueprints

| File | Kind | Publishes |
| :--- | :--- | :--- |
| `arcadematrix_value.yaml` | automation | one entity as a `value` page |
| `arcadematrix_table.yaml` | automation | up to 4 entities as a `table` page |
| `arcadematrix_weather.yaml` | automation | a weather entity (current + daily forecast) as a `weather` page |
| `arcadematrix_graph_history.yaml` | **template** | a sensor holding 24 h of samples (96 x 15 min) for one or two sensors |
| `arcadematrix_graph.yaml` | automation | a `graph` page from that history sensor |

### Importing

- **From the UI**: Settings → Automations & scenes → Blueprints → Import blueprint, and paste the file's URL on GitHub.
- **By copying files**: copy the automation blueprints into `/config/blueprints/automation/arcadematrix/` and the template blueprint into `/config/blueprints/template/arcadematrix/`, then reload (Developer tools → YAML → Reload all, or restart).

### Value, table and weather

Create an automation from the blueprint (Settings → Automations → Create → from blueprint), pick the entity or entities, and set the **MQTT topic** (for example `arcadematrix/value/outside`). The automation publishes on every change and when Home Assistant starts, so the retained message is always current.

The table blueprint also has an optional **Short labels** input: comma-separated abbreviations in the same order as the entities (for example `DOWN, UP`). The sign uses a short label only when the full one doesn't fit its tile.

### Graph (two blueprints)

A graph needs history, which the sign never collects. Use both blueprints:

1. Add the **history** template sensor to `configuration.yaml`, then reload template entities:

   ```yaml
   template:
     - use_blueprint:
         path: arcadematrix/arcadematrix_graph_history.yaml
         input:
           sensor_1: sensor.outdoor_temperature
           sensor_2: sensor.downstairs_temperature
       name: ArcadeMatrix outside vs down
       unique_id: arcadematrix_outside_vs_down
   ```

   If you imported the blueprint from the UI, use the path Home Assistant shows for it under Blueprints.

2. Create an automation from **ArcadeMatrix - publish a graph**, select that history sensor, and set the topic, title, legends and colours.

The history adds one sample every 15 minutes, so the graph **fills in over the first 24 hours**.

## 3. On the sign

Add a screen with the **MQTT Data** engine and put the topics in its **MQTT Topics** field, comma separated, for example:

```
arcadematrix/weather/local, arcadematrix/graph/outside, arcadematrix/value/outside
```

Each topic is one page (a weather topic shows its NOW page and each forecast day). The screen stays up for one full cycle of its pages. Pages show `NO DATA` until the automation has published once; run it manually (⋮ → Run actions) to publish right away.
