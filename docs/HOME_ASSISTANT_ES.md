🇬🇧 [English](HOME_ASSISTANT.md) | 🇫🇷 [Français](HOME_ASSISTANT_FR.md) | 🇪🇸 Español

# Integración con Home Assistant

Home Assistant puede alimentar las pantallas **MQTT Data** del panel con los blueprints listos de [`tools/home_assistant/blueprints/`](../tools/home_assistant/blueprints/). Publican mensajes retained conformes al [contrato MQTT Data](MQTT_DATA_CONTRACT_ES.md).

## 1. Requisitos

- Un broker MQTT accesible desde Home Assistant y desde el panel (por ejemplo el add-on **Mosquitto broker**) y la integración **MQTT** configurada en Home Assistant.
- En el panel, **Sistema → MQTT Data**: la dirección del broker, el puerto y un usuario. Un usuario de Home Assistant dedicado funciona bien, ya que el add-on Mosquitto acepta usuarios de Home Assistant.

## 2. Los blueprints

| Archivo | Tipo | Publica |
| :--- | :--- | :--- |
| `arcadematrix_value.yaml` | automatización | una entidad como página `value` |
| `arcadematrix_table.yaml` | automatización | hasta 4 entidades como página `table` |
| `arcadematrix_weather.yaml` | automatización | una entidad del tiempo (actual + pronóstico diario) como página `weather` |
| `arcadematrix_graph_history.yaml` | **template** | un sensor que guarda 24 h de muestras (96 x 15 min) de uno o dos sensores |
| `arcadematrix_graph.yaml` | automatización | una página `graph` a partir de ese sensor de historial |

### Importación

- **Desde la interfaz**: Ajustes → Automatizaciones y escenas → Blueprints → Importar blueprint, y pegar la URL de GitHub del archivo.
- **Copiando archivos**: copiar los blueprints de automatización en `/config/blueprints/automation/arcadematrix/` y el blueprint template en `/config/blueprints/template/arcadematrix/`, y recargar (Herramientas para desarrolladores → YAML → Recargar todo, o reiniciar).

### Valor, tabla y tiempo

Crear una automatización desde el blueprint (Ajustes → Automatizaciones → Crear → desde blueprint), elegir la entidad o entidades y el **topic MQTT** (por ejemplo `arcadematrix/value/outside`). La automatización publica en cada cambio y al arrancar Home Assistant, así que el mensaje retained siempre está al día.

El blueprint de tabla también tiene una entrada opcional **Short labels** (etiquetas cortas): abreviaturas separadas por comas, en el mismo orden que las entidades (por ejemplo `DOWN, UP`). El panel solo usa una etiqueta corta cuando la completa no cabe en su casilla.

### Gráfico (dos blueprints)

Un gráfico necesita historial, que el panel nunca recoge. Use los dos blueprints:

1. Añadir el sensor template **de historial** en `configuration.yaml` y recargar las entidades template:

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

   Si importó el blueprint desde la interfaz, use la ruta que Home Assistant muestra en Blueprints.

2. Crear una automatización desde **ArcadeMatrix - publish a graph**, elegir ese sensor de historial y poner el topic, el título, las leyendas y los colores.

El historial añade una muestra cada 15 minutos, así que el gráfico **se completa durante las primeras 24 horas**.

## 3. En el panel

Añadir una pantalla con el motor **MQTT Data** y escribir los topics en su campo **Topics MQTT**, separados por comas, por ejemplo:

```
arcadematrix/weather/local, arcadematrix/graph/outside, arcadematrix/value/outside
```

Cada topic es una página (un topic del tiempo muestra su página AHORA y cada día del pronóstico). La pantalla se mantiene un ciclo completo de sus páginas. Las páginas muestran `SIN DATOS` hasta que la automatización publique una vez; ejecútela a mano (⋮ → Ejecutar acciones) para publicar enseguida.
