🇬🇧 [English](HOME_ASSISTANT.md) | 🇫🇷 Français | 🇪🇸 [Español](HOME_ASSISTANT_ES.md)

# Intégration Home Assistant

Home Assistant peut alimenter les écrans **MQTT Data** du panneau grâce aux blueprints prêts à l'emploi de [`tools/home_assistant/blueprints/`](../tools/home_assistant/blueprints/). Ils publient des messages retained conformes au [contrat MQTT Data](MQTT_DATA_CONTRACT_FR.md).

## 1. Prérequis

- Un broker MQTT joignable par Home Assistant et par le panneau (par exemple l'add-on **Mosquitto broker**) et l'intégration **MQTT** configurée dans Home Assistant.
- Sur le panneau, **Système → MQTT Data** : l'adresse du broker, le port et un identifiant. Un utilisateur Home Assistant dédié convient bien : l'add-on Mosquitto accepte les utilisateurs Home Assistant.

## 2. Les blueprints

| Fichier | Type | Publie |
| :--- | :--- | :--- |
| `arcadematrix_value.yaml` | automatisation | une entité en page `value` |
| `arcadematrix_table.yaml` | automatisation | jusqu'à 4 entités en page `table` |
| `arcadematrix_weather.yaml` | automatisation | une entité météo (actuel + prévisions quotidiennes) en page `weather` |
| `arcadematrix_graph_history.yaml` | **template** | un capteur qui garde 24 h d'échantillons (96 x 15 min) d'un ou deux capteurs |
| `arcadematrix_graph.yaml` | automatisation | une page `graph` à partir de ce capteur d'historique |

### Importation

- **Depuis l'interface** : Paramètres → Automatisations et scènes → Blueprints → Importer un blueprint, puis coller l'URL GitHub du fichier.
- **En copiant les fichiers** : copier les blueprints d'automatisation dans `/config/blueprints/automation/arcadematrix/` et le blueprint template dans `/config/blueprints/template/arcadematrix/`, puis recharger (Outils de développement → YAML → Tout recharger, ou redémarrer).

### Valeur, tableau et météo

Créer une automatisation à partir du blueprint (Paramètres → Automatisations → Créer → à partir d'un blueprint), choisir la ou les entités et le **topic MQTT** (par exemple `arcadematrix/value/outside`). L'automatisation publie à chaque changement et au démarrage de Home Assistant : le message retained est donc toujours à jour.

Le blueprint de tableau a aussi une entrée optionnelle **Short labels** (libellés courts) : des abréviations séparées par des virgules, dans le même ordre que les entités (par exemple `DOWN, UP`). Le panneau n'utilise un libellé court que si le libellé complet ne tient pas dans sa tuile.

### Graphique (deux blueprints)

Un graphique a besoin d'un historique, que le panneau ne collecte jamais. Utiliser les deux blueprints :

1. Ajouter le capteur template **d'historique** dans `configuration.yaml`, puis recharger les entités template :

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

   Si le blueprint a été importé depuis l'interface, utiliser le chemin indiqué par Home Assistant dans Blueprints.

2. Créer une automatisation à partir de **ArcadeMatrix - publish a graph**, choisir ce capteur d'historique, puis le topic, le titre, les légendes et les couleurs.

L'historique ajoute un échantillon toutes les 15 minutes : le graphique **se remplit pendant les premières 24 heures**.

## 3. Sur le panneau

Ajouter un écran avec le moteur **MQTT Data** et saisir les topics dans son champ **Topics MQTT**, séparés par des virgules, par exemple :

```
arcadematrix/weather/local, arcadematrix/graph/outside, arcadematrix/value/outside
```

Chaque topic est une page (un topic météo affiche sa page ACTU. et chaque jour de prévision). L'écran reste affiché le temps d'un cycle complet de ses pages. Les pages affichent `PAS DE DONNEES` tant que l'automatisation n'a pas publié ; l'exécuter à la main (⋮ → Exécuter les actions) pour publier tout de suite.
