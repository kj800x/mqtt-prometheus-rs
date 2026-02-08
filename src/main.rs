use metrics::{describe_gauge, gauge};
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

fn main() {
    tracing_subscriber::fmt::init();

    let builder = PrometheusBuilder::new();
    builder
        .idle_timeout(MetricKindMask::ALL, Some(Duration::from_secs(60 * 60 * 3)))
        .with_http_listener(([0, 0, 0, 0], 9090))
        .install()
        .expect("Failed to install Prometheus recorder");

    describe_gauge!(
        "waterheater_state",
        "Water heater metric value from MQTT; 'sensor' is the topic path suffix (e.g. sensor/lower_tank_temperature)."
    );
    describe_gauge!(
        "waterheater_state_info",
        "Water heater categorical state (e.g. mode, preset); value 1 when state is current, 'state' label is the string value (e.g. heat, eco)."
    );

    #[cfg(feature = "mqtt")]
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");
        rt.block_on(run_mqtt_loop()).expect("MQTT loop failed");
    }
}

fn mqtt_config_from_env() -> Result<(MqttOptions, String), Box<dyn std::error::Error + Send + Sync>>
{
    let host = std::env::var("MQTT_HOST").or_else(|_| std::env::var("MQTT_BROKER"))?;
    let port: u16 = std::env::var("MQTT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(1883);
    let client_id = std::env::var("MQTT_CLIENT_ID").unwrap_or_else(|_| "mqtt-prometheus-rs".into());
    let topic = std::env::var("MQTT_TOPIC").unwrap_or_else(|_| "waterheater/#".into());

    let mut opts = MqttOptions::new(client_id, host, port);
    opts.set_keep_alive(Duration::from_secs(30));

    Ok((opts, topic))
}

async fn run_mqtt_loop() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (opts, topic) = mqtt_config_from_env()?;
    let (client, mut eventloop) = AsyncClient::new(opts, 100);
    client.subscribe(&topic, QoS::AtMostOnce).await?;

    let state_tracker: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    while let Ok(event) = eventloop.poll().await {
        if let Event::Incoming(Packet::Publish(publish)) = event {
            handle_waterheater_message(&publish.topic, &publish.payload, &state_tracker);
        }
    }
    Ok(())
}

fn handle_waterheater_message(
    topic: &str,
    payload: &[u8],
    state_tracker: &Mutex<HashMap<String, String>>,
) {
    let payload_str = match std::str::from_utf8(payload) {
        Ok(s) => s.trim(),
        Err(_) => return,
    };
    let suffix = topic.strip_prefix("waterheater/").unwrap_or(topic);
    let metric_key = suffix.replace('/', "_").replace('-', "_");
    if metric_key.is_empty() {
        return;
    }

    let labels = [("sensor".to_string(), metric_key.clone())];
    if let Ok(n) = payload_str.parse::<f64>() {
        gauge!("waterheater_state", n, &labels);
        return;
    }

    let on_off = match payload_str.to_uppercase().as_str() {
        "ON" | "TRUE" | "1" => Some(1.0),
        "OFF" | "FALSE" | "0" => Some(0.0),
        _ => None,
    };
    if let Some(v) = on_off {
        gauge!("waterheater_state", v, &labels);
        return;
    }

    if metric_key.contains("alarm_history") || metric_key.contains("debug") {
        return;
    }

    let state_label = payload_str
        .to_lowercase()
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    if state_label.is_empty() {
        return;
    }
    {
        let mut map = state_tracker.lock().expect("state_tracker lock");
        if let Some(previous) = map.get(&metric_key) {
            if previous != &state_label {
                let prev_labels = [
                    ("sensor".to_string(), metric_key.clone()),
                    ("state".to_string(), previous.clone()),
                ];
                gauge!("waterheater_state_info", 0.0, &prev_labels);
            }
        }
        map.insert(metric_key.clone(), state_label.clone());
    }
    let state_labels = [
        ("sensor".to_string(), metric_key),
        ("state".to_string(), state_label),
    ];
    gauge!("waterheater_state_info", 1.0, &state_labels);
}
