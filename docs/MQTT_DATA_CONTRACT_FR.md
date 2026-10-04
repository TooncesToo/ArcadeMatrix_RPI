🇬🇧 [English](MQTT_DATA_CONTRACT.md) | 🇫🇷 Français | 🇪🇸 [Español](MQTT_DATA_CONTRACT_ES.md)

# Contrat MQTT Data

Le moteur **MQTT Data** (`mqttdata`) affiche des pages envoyées en MQTT par n'importe quel éditeur : une domotique, un script, un serveur. Ce document est le contrat entre l'éditeur et le panneau. Il est identique pour les firmwares Raspberry Pi et ESP32.

Pour Home Assistant, des blueprints prêts à l'emploi existent : voir [HOME_ASSISTANT_FR.md](HOME_ASSISTANT_FR.md).

---

## 1. Topics et instantané retained

- Un écran MQTT Data liste 1 à 6 **topics** (son seul réglage). Chaque topic est une page, affichée dans l'ordre des topics.
- L'éditeur **doit publier avec `retain: true`**. Le panneau ne reste pas connecté en arrière-plan : quand l'écran devient actif, il se connecte au broker défini dans **Système → MQTT Data**, s'abonne et dessine le message retained de chaque topic (l'instantané). Pendant l'affichage, il reste abonné et redessine à chaque nouveau message. Quand l'écran quitte la rotation, il se déconnecte et libère tout.
- Un **message retained vide** efface un topic : sa page affiche `PAS DE DONNEES`.
- L'éditeur fait toute l'agrégation et la mise en forme (historique, arrondis, unités). Le panneau ne collecte rien et ne calcule rien.

## 2. Champs communs

Chaque message est un objet JSON.

| Champ | Obligatoire | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `type` | **oui** | — | `value`, `table`, `graph` ou `weather`. Choisit le rendu. |
| `v` | non | `1` | Version du contrat. |
| `seconds` | non | `10` | Durée de la page, minimum `3` (maximum `3600`). Pour `weather`, par page (ACTU. et chaque jour). |

Le temps de l'écran dans la rotation est la somme des `seconds` de ses pages ; la durée propre du créneau de rotation est ignorée. Un topic encore sans message compte pour 10 s.

## 3. Types de page

### `value` : une grande valeur

| Champ | Description |
| :--- | :--- |
| `value` | La valeur, déjà formatée (texte ; un nombre est accepté). Jamais tronquée. |
| `unit` | Dessinée après la valeur, plus petite. Retirée si valeur et unité ne tiennent pas. |
| `label` | Dessiné au-dessus de la valeur. `label_short` n'est utilisé que si `label` ne tient pas. |
| `color` | Couleur de la valeur `#RRGGBB` (blanc par défaut). `label_color` par défaut `#808080`. |

```json
{"v": 1, "type": "value", "value": "89", "unit": "°F", "label": "OUTSIDE", "color": "#FF6040"}
```

### `table` : 1 à 4 valeurs

| Champ | Description |
| :--- | :--- |
| `title` | Ligne de titre (affichée quand la mise en page le permet : 1 à 3 tuiles). |
| `show_title` | `false` masque le titre (défaut `true`). |
| `tiles` | 1 à 4 objets avec les champs de `value` : `label`, `label_short`, `value`, `unit`, `color`, `label_color`. Les tuiles en trop sont ignorées. |

1 à 3 tuiles se partagent la largeur ; 4 tuiles forment une grille 2x2. Chaque tuile est centrée : libellé en haut, valeur + unité dessous.

```json
{"v": 1, "type": "table", "title": "HOME", "show_title": true,
 "tiles": [{"label": "DOWNSTAIRS", "label_short": "DOWN", "value": "77.9", "unit": "°F"},
           {"label": "UPSTAIRS", "label_short": "UP", "value": "76.1", "unit": "°F"}]}
```

### `graph` : barres et lignes

| Champ | Description |
| :--- | :--- |
| `title`, `title_short` | Titre de l'en-tête (la forme courte seulement si la complète ne tient pas). |
| `summary`, `summary_color` | Partie droite de l'en-tête (couleur par défaut : celle de la première série). |
| `show_header` | `false` masque l'en-tête (défaut `true`). |
| `series` | Jusqu'à 4 : `label`, `color`, `style` (`bar` par défaut, ou `line`), `data` (nombres, `null` = manquant). |
| `slots` | Nombre de positions en x (défaut : la plus longue série). Données alignées à droite : le dernier point est le plus récent. |
| `min`, `max` | Échelle y. Absents : automatique (lignes : plage des données plus 5 %). |
| `stack` | `true` (défaut) empile les barres ; `false` les superpose. Les barres partent de zéro (négatives vers le bas). |
| `yaxis` | `true` : étiquettes min / max à gauche (panneaux de 64 px de haut). |
| `bands` | Jusqu'à 96 `{from, to, color}` : colonnes de fond (indices) ; au-delà, les plus récentes sont gardées. |
| `marks` | `{at, color}` : ligne verticale pointillée à l'indice `at`. |
| `legend` | Jusqu'à 4 mots `{text, color}` en bas (panneaux de 64 px de haut uniquement). |

```json
{"v": 1, "type": "graph", "title": "OUTSIDE VS DOWN", "summary": "89°", "summary_color": "#FF4020",
 "slots": 96, "min": 76, "max": 90, "yaxis": true,
 "series": [{"label": "OUTSIDE", "style": "line", "color": "#FF4020", "data": [88.5, 89.0, null, 89.0]},
            {"label": "DOWNSTAIRS", "style": "line", "color": "#40A0FF", "data": [77.6, 77.9, 77.9, 77.9]}],
 "marks": [{"at": 48, "color": "#404040"}],
 "legend": [{"text": "OUTSIDE", "color": "#FF4020"}, {"text": "DOWNSTAIRS", "color": "#40A0FF"}]}
```

### `weather` : conditions actuelles et prévisions

| Champ | Description |
| :--- | :--- |
| `units` | `imperial` (°F ; maximale au-dessus de la minimale) ou `metric` (°C). Le panneau ne convertit pas. |
| `current` | `temp`, `condition`, `humidity`, `wind`, `wind_unit`, `wind_dir` (`N`, `NE`, ... `NW`). |
| `days` | Jusqu'à 5 : `temp_max`, `temp_min`, `condition`. |

La page se déploie en une page **ACTU.** (la mesure en direct `current.temp`, puis humidité et vent, par ex. `41%  NE 9mph`) suivie d'une page par jour. Conditions : `sunny`, `clear-night`, `partlycloudy`, `cloudy`, `fog`, `exceptional`, `rainy`, `pouring`, `lightning`, `lightning-rainy`, `snowy`, `snowy-rainy`, `hail`, `windy`, `windy-variant` ; toute autre valeur est affichée telle quelle.

```json
{"v": 1, "type": "weather", "units": "imperial",
 "current": {"temp": 90, "condition": "windy", "humidity": 30, "wind": 6, "wind_unit": "mph", "wind_dir": "SE"},
 "days": [{"temp_max": 95, "temp_min": 72, "condition": "sunny"},
          {"temp_max": 93, "temp_min": 71, "condition": "partlycloudy"}]}
```

## 4. Limites

| | Raspberry Pi | ESP32 avec PSRAM | ESP32 sans PSRAM |
| :--- | :--- | :--- | :--- |
| Taille d'un message | 8 Ko | 8 Ko | 4 Ko |
| Points par série | 288 | 288 | 96 |
| Topics par écran | 6 | 6 | 4 |
| Séries / tuiles / légendes / bandes | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 | 4 / 4 / 4 / 96 |

Un message trop gros est ignoré (la page garde son contenu précédent). Séries, tuiles, points ou bandes en trop sont retirés.

## 5. Versions et compatibilité

- Les champs inconnus sont ignorés : l'éditeur peut en ajouter librement.
- Un message avec un `v` plus élevé que celui du firmware affiche quand même tous les champs connus.
- Les nouveaux types de page auront de nouveaux noms de `type` ; un firmware plus ancien les affiche comme `NON PRIS EN CHARGE`.

## 6. Messages

| Message | Signification |
| :--- | :--- |
| `CONNEXION` | L'écran vient de s'activer et se connecte. |
| `PAS DE CONNEXION` | Le broker est injoignable (vérifier Système → MQTT Data). |
| `PAS DE DONNEES` | Le topic n'a pas de message retained (3 s après la connexion), ou un message vide. |
| `NON PRIS EN CHARGE` | `type` manquant ou inconnu, champs requis absents, ou message non JSON. Affiché pendant les `seconds` de la page ; les autres pages ne sont pas touchées. |
