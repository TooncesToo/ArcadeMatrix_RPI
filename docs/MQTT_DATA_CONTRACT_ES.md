🇬🇧 [English](MQTT_DATA_CONTRACT.md) | 🇫🇷 [Français](MQTT_DATA_CONTRACT_FR.md) | 🇪🇸 Español

# Contrato MQTT Data

El motor **MQTT Data** (`mqttdata`) muestra páginas que cualquier emisor envía por MQTT: un sistema domótico, un script, un servidor. Este documento es el contrato entre el emisor y el panel. Es idéntico para los firmwares de Raspberry Pi y ESP32.

Para Home Assistant hay blueprints listos: ver [HOME_ASSISTANT_ES.md](HOME_ASSISTANT_ES.md).

---

## 1. Topics e instantánea retained

- Una pantalla MQTT Data lista de 1 a 6 **topics** (su único ajuste). Cada topic es una página, mostrada en el orden de los topics.
- El emisor **debe publicar con `retain: true`**. El panel no se queda conectado en segundo plano: cuando la pantalla se activa se conecta al broker definido en **Sistema → MQTT Data**, se suscribe y dibuja el mensaje retained de cada topic (la instantánea). Mientras se muestra sigue suscrito y redibuja con cada mensaje nuevo. Cuando la pantalla sale de la rotación se desconecta y libera todo.
- Un **mensaje retained vacío** borra un topic: su página muestra `SIN DATOS`.
- El emisor hace toda la agregación y el formato (historial, redondeo, unidades). El panel no recoge puntos ni calcula nada.

## 2. Campos comunes

Cada mensaje es un objeto JSON.

| Campo | Obligatorio | Por defecto | Descripción |
| :--- | :--- | :--- | :--- |
| `type` | **sí** | — | `value`, `table`, `graph` o `weather`. Elige el dibujo. |
| `v` | no | `1` | Versión del contrato. |
| `seconds` | no | `10` | Tiempo de la página, mínimo `3` (máximo `3600`). Para `weather`, por página (AHORA y cada día). |

El tiempo de la pantalla en la rotación es la suma de los `seconds` de sus páginas; la duración propia de la casilla de rotación se ignora. Un topic aún sin mensaje cuenta 10 s.

## 3. Tipos de página

### `value`: un valor grande

| Campo | Descripción |
| :--- | :--- |
| `value` | El valor, ya formateado (texto; se acepta un número). Nunca se trunca. |
| `unit` | Se dibuja tras el valor, más pequeña. Se quita si valor y unidad no caben. |
| `label` | Se dibuja sobre el valor. `label_short` solo se usa si `label` no cabe. |
| `color` | Color del valor `#RRGGBB` (blanco por defecto). `label_color` por defecto `#808080`. |

```json
{"v": 1, "type": "value", "value": "89", "unit": "°F", "label": "OUTSIDE", "color": "#FF6040"}
```

### `table`: de 1 a 4 valores

| Campo | Descripción |
| :--- | :--- |
| `title` | Fila de título (se muestra cuando hay espacio: 1 a 3 casillas). |
| `show_title` | `false` oculta el título (por defecto `true`). |
| `tiles` | De 1 a 4 objetos con los campos de `value`: `label`, `label_short`, `value`, `unit`, `color`, `label_color`. Las casillas de más se ignoran. |

De 1 a 3 casillas comparten el ancho; 4 casillas forman una cuadrícula 2x2. Cada casilla va centrada: etiqueta arriba, valor + unidad debajo.

```json
{"v": 1, "type": "table", "title": "HOME", "show_title": true,
 "tiles": [{"label": "DOWNSTAIRS", "label_short": "DOWN", "value": "77.9", "unit": "°F"},
           {"label": "UPSTAIRS", "label_short": "UP", "value": "76.1", "unit": "°F"}]}
```

### `graph`: barras y líneas

| Campo | Descripción |
| :--- | :--- |
| `title`, `title_short` | Título del encabezado (la forma corta solo si la completa no cabe). |
| `summary`, `summary_color` | Parte derecha del encabezado (color por defecto: el de la primera serie). |
| `show_header` | `false` oculta el encabezado (por defecto `true`). |
| `series` | Hasta 4: `label`, `color`, `style` (`bar` por defecto, o `line`), `data` (números, `null` = falta). |
| `slots` | Número de posiciones en x (por defecto: la serie más larga). Datos alineados a la derecha: el último punto es el más reciente. |
| `min`, `max` | Escala y. Ausentes: automática (líneas: rango de los datos más un 5 %). |
| `stack` | `true` (por defecto) apila las barras; `false` las superpone. Las barras parten de cero (negativas hacia abajo). |
| `yaxis` | `true`: etiquetas mín / máx a la izquierda (paneles de 64 px de alto). |
| `bands` | Hasta 96 `{from, to, color}`: columnas de fondo (índices); si hay más, se conservan las más recientes. |
| `marks` | `{at, color}`: línea vertical punteada en el índice `at`. |
| `legend` | Hasta 4 palabras `{text, color}` abajo (solo paneles de 64 px de alto). |

```json
{"v": 1, "type": "graph", "title": "OUTSIDE VS DOWN", "summary": "89°", "summary_color": "#FF4020",
 "slots": 96, "min": 76, "max": 90, "yaxis": true,
 "series": [{"label": "OUTSIDE", "style": "line", "color": "#FF4020", "data": [88.5, 89.0, null, 89.0]},
            {"label": "DOWNSTAIRS", "style": "line", "color": "#40A0FF", "data": [77.6, 77.9, 77.9, 77.9]}],
 "marks": [{"at": 48, "color": "#404040"}],
 "legend": [{"text": "OUTSIDE", "color": "#FF4020"}, {"text": "DOWNSTAIRS", "color": "#40A0FF"}]}
```

### `weather`: condiciones actuales y pronóstico

| Campo | Descripción |
| :--- | :--- |
| `units` | `imperial` (°F; la máxima encima de la mínima) o `metric` (°C). El panel no convierte. |
| `current` | `temp`, `condition`, `humidity`, `wind`, `wind_unit`, `wind_dir` (`N`, `NE`, ... `NW`). |
| `days` | Hasta 5: `temp_max`, `temp_min`, `condition`. |

La página se despliega en una página **AHORA** (la lectura en vivo `current.temp`, luego humedad y viento, p. ej. `41%  NE 9mph`) seguida de una página por día. Condiciones: `sunny`, `clear-night`, `partlycloudy`, `cloudy`, `fog`, `exceptional`, `rainy`, `pouring`, `lightning`, `lightning-rainy`, `snowy`, `snowy-rainy`, `hail`, `windy`, `windy-variant`; cualquier otra se muestra tal cual.

```json
{"v": 1, "type": "weather", "units": "imperial",
 "current": {"temp": 90, "condition": "windy", "humidity": 30, "wind": 6, "wind_unit": "mph", "wind_dir": "SE"},
 "days": [{"temp_max": 95, "temp_min": 72, "condition": "sunny"},
          {"temp_max": 93, "temp_min": 71, "condition": "partlycloudy"}]}
```

## 4. Límites

| | Raspberry Pi | ESP32 con PSRAM | ESP32 sin PSRAM |
| :--- | :--- | :--- | :--- |
| Tamaño del mensaje | 8 KB | 8 KB | 4 KB |
| Puntos por serie | 288 | 288 | 96 |
| Topics por pantalla | 6 | 6 | 4 |
| Series / casillas / leyendas / bandas | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 |

Un mensaje demasiado grande se ignora (la página conserva su contenido anterior). Las series, casillas, puntos o bandas de más se descartan.

## 5. Versiones y compatibilidad

- Los campos desconocidos se ignoran: el emisor puede añadir campos libremente.
- Un mensaje con un `v` mayor que el que conoce el firmware muestra igualmente todos los campos conocidos.
- Los nuevos tipos de página tendrán nuevos nombres de `type`; un firmware más antiguo los muestra como `NO COMPATIBLE`.

## 6. Avisos

| Aviso | Significado |
| :--- | :--- |
| `CONECTANDO` | La pantalla acaba de activarse y se está conectando. |
| `SIN CONEXION` | El broker es inalcanzable (revisar Sistema → MQTT Data). |
| `SIN DATOS` | El topic no tiene mensaje retained (3 s tras conectar), o uno vacío. |
| `NO COMPATIBLE` | `type` falta o es desconocido, faltan campos obligatorios o el mensaje no es JSON. Se muestra durante los `seconds` de la página; las demás páginas no se ven afectadas. |
