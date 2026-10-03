//! Data feed MQTT client (`data_mqtt`).
//!
//! A second, independent MQTT connection used to receive data published by
//! Home Assistant, Node-RED, etc. Unlike the Frontend Sync listener (`mqtt`),
//! it never takes over the display: it only stores the last payload of every
//! topic under `<topic_prefix>/#` in the [`data_feed`] store, where engines
//! such as `graph` pick it up.

use crate::core::config::{Config, DataMqttConfig};
use crate::core::data_feed::{data_feed, PublishResult, MAX_PAYLOAD_BYTES};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

const BACKOFF_MIN_SECS: u64 = 5;
const BACKOFF_MAX_SECS: u64 = 60;

/// MQTT client id for the data connection: the device's own name plus
/// `-data`, so several signs can share one broker without kicking each other.
pub fn data_client_id(hostname: &str, device_name: &str) -> String {
    let base = if !hostname.trim().is_empty() {
        hostname.trim()
    } else if !device_name.trim().is_empty() {
        device_name.trim()
    } else {
        "arcadematrix"
    };
    format!("{}-data", base)
}

/// Subscription filter for the configured prefix (`arcadematrix` -> `arcadematrix/#`).
pub fn subscription_filter(prefix: &str) -> String {
    let p = prefix.trim().trim_end_matches('/');
    if p.is_empty() {
        "arcadematrix/#".to_string()
    } else {
        format!("{}/#", p)
    }
}

/// Whether the configuration asks for a running client.
pub fn is_active(cfg: &DataMqttConfig) -> bool {
    cfg.enabled && !cfg.broker.trim().is_empty()
}

/// Starts the data MQTT client in a background thread if enabled. Settings
/// changes restart the whole app (see `post_system`), so the thread reads its
/// configuration once.
pub fn start_data_mqtt_client(config: Arc<Config>) {
    let (cfg, client_id) = {
        let s = config.settings.read();
        (
            s.data_mqtt.clone(),
            data_client_id(&s.wifi.hostname, &s.mqtt.device_name),
        )
    };
    if !is_active(&cfg) {
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("data-mqtt".to_string())
        .spawn(move || run_client(cfg, client_id));
    if let Err(e) = spawned {
        warn!("Data MQTT: failed to start client thread: {}", e);
    }
}

fn run_client(cfg: DataMqttConfig, client_id: String) {
    let filter = subscription_filter(&cfg.topic_prefix);
    let mut options = MqttOptions::new(client_id.clone(), cfg.broker.trim(), cfg.port);
    options.set_keep_alive(Duration::from_secs(60));
    // Leave room for the 4 KB payload limit plus topic and header.
    options.set_max_packet_size(MAX_PAYLOAD_BYTES + 1024, 1024);
    if !cfg.user.is_empty() {
        options.set_credentials(cfg.user.clone(), cfg.pass.clone());
    }

    info!(
        "Data MQTT: connecting to {}:{} as '{}', subscribing to '{}'",
        cfg.broker, cfg.port, client_id, filter
    );
    let (client, mut connection) = Client::new(options, 10);
    let mut backoff = BACKOFF_MIN_SECS;
    let mut warned_too_large = false;
    let mut warned_full = false;

    for notification in connection.iter() {
        match notification {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                info!("Data MQTT: connected");
                data_feed().set_connected(true);
                backoff = BACKOFF_MIN_SECS;
                // Clean session: subscribe again on every (re)connect so the
                // broker re-sends retained values.
                if let Err(e) = client.try_subscribe(filter.as_str(), QoS::AtMostOnce) {
                    warn!("Data MQTT: subscribe to '{}' failed: {}", filter, e);
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let Ok(payload) = std::str::from_utf8(&publish.payload) else {
                    continue;
                };
                match data_feed().publish(&publish.topic, payload) {
                    PublishResult::Stored => {}
                    PublishResult::TooLarge if !warned_too_large => {
                        warned_too_large = true;
                        warn!(
                            "Data MQTT: dropped payload on '{}' ({} bytes > {} max)",
                            publish.topic,
                            payload.len(),
                            MAX_PAYLOAD_BYTES
                        );
                    }
                    PublishResult::Full if !warned_full => {
                        warned_full = true;
                        warn!(
                            "Data MQTT: topic limit reached, ignoring '{}'",
                            publish.topic
                        );
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(e) => {
                data_feed().set_connected(false);
                warn!("Data MQTT: connection error: {} (retry in {}s)", e, backoff);
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
            }
        }
    }
}
