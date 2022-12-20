use std::collections::HashMap;

use config::Config;
use log::{error, Level};
use modem_scraper::{construct_loki_streams, FixedSizeSortedHashSet};
use modem_scraper_lib::payloads::s33::{
    GetMultipleHNAPsLogsResponse, GetMultipleHNAPsMetricsResponse,
};
use modem_scraper_lib::payloads::{Channel, LogEntry};
use modem_scraper_lib::SOAPClient;
use opentelemetry::sdk::{trace, Resource};
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use telegraf::{Metric, Point};
use tracing::instrument;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{prelude::*, EnvFilter};
use tracing_unwrap::ResultExt;

/// sends channel metrics to telegraf
#[instrument(skip(telegraf_client))]
fn metrics_to_telegraf(
    metrics: GetMultipleHNAPsMetricsResponse,
    telegraf_client: &mut telegraf::Client,
) -> Result<(), telegraf::TelegrafError> {
    // downstream points
    let mut points: Vec<telegraf::Point> = metrics
        .get_customer_status_downstream_channel_info_response
        .customer_conn_downstream_channel
        .iter()
        .map(|p| match p {
            Channel::Downstream(c) => c.to_point(),
            Channel::Upstream(c) => c.to_point(),
        })
        .collect();
    // upstream points
    points.extend(
        metrics
            .get_customer_status_upstream_channel_info_response
            .customer_conn_upstream_channel
            .iter()
            .map(|p| match p {
                Channel::Downstream(c) => c.to_point(),
                Channel::Upstream(c) => c.to_point(),
            })
            .collect::<Vec<Point>>(),
    );
    // timestamps?
    telegraf_client.write_points(&points)
}

#[instrument]
async fn logs_to_loki(
    logs: GetMultipleHNAPsLogsResponse,
    http_client: &reqwest::Client,
    loki_url: String,
    seen_logs: &mut FixedSizeSortedHashSet<LogEntry>,
) -> Result<reqwest::Response, reqwest::Error> {
    let labels = HashMap::from([("app".to_owned(), "modem_scraper".to_owned())]);
    let streams = construct_loki_streams(
        labels,
        logs.get_customer_status_log_response
            .customer_status_log_list
            .iter()
            .map(|log_entry| {
                // if we haven't seen this log entry yet, add it so that we're not repeating logs.
                // since seen_logs is a hash set, this should be an O(1) lookup.
                if !seen_logs.contains(log_entry) {
                    seen_logs.insert(log_entry.clone());
                    // return the destructured tuple for sending to Loki
                    Some((
                        log_entry.level,
                        u128::try_from(log_entry.timestamp.timestamp_nanos()).unwrap_or_log(),
                        log_entry.message.to_owned(),
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<Option<(Level, u128, String)>>>(),
    );

    http_client.post(loki_url).json(&streams).send().await
}

#[tokio::main]
async fn main() {
    let settings = Config::builder()
        .add_source(config::File::with_name("config.yml"))
        .add_source(config::Environment::with_prefix(
            &env!("CARGO_PKG_NAME").replace('-', "_").to_uppercase(),
        ))
        .build()
        .unwrap();

    if settings.get_bool("trace").unwrap_or(false) {
        let otlp_tracer =
            opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_exporter(opentelemetry_otlp::new_exporter().http().with_env())
                .with_trace_config(trace::config().with_resource(Resource::new(vec![
                    KeyValue::new("service.name", env!("CARGO_PKG_NAME").replace('-', "_")),
                ])))
                .install_batch(opentelemetry::runtime::Tokio)
                .unwrap();
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(otlp_tracer)
            .with_filter(EnvFilter::from_default_env());
        tracing_subscriber::registry()
            .with(otel_layer)
            .try_init()
            .unwrap_or_log();
    }

    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(settings.get_bool("accept_invalid_certs").unwrap())
        .build()
        .unwrap();

    let mut modem_client = SOAPClient::new(
        settings.get_string("device_address").unwrap(),
        settings.get_bool("accept_invalid_certs").unwrap_or(false),
    );

    let scrape_duration = std::time::Duration::from_secs(
        u64::try_from(settings.get_int("scrape_interval_seconds").unwrap()).unwrap(),
    );

    let mut telegraf_client =
        telegraf::Client::new(&settings.get_string("telegraf_address").unwrap()).unwrap();

    // tick this every 5s
    let forever = tokio::task::spawn(async move {
        let mut interval = tokio::time::interval(scrape_duration);

        modem_client
            .login(
                &settings.get_string("device_username").unwrap(),
                &settings.get_string("device_password").unwrap(),
            )
            .await;

        let mut last_n_logs: FixedSizeSortedHashSet<LogEntry> =
            FixedSizeSortedHashSet::with_capacity(30);
        loop {
            let metrics: GetMultipleHNAPsMetricsResponse = modem_client.metrics().await;
            match metrics_to_telegraf(metrics, &mut telegraf_client) {
                Ok(_) => (),
                Err(e) => error!("{}", e),
            }
            let logs_response: GetMultipleHNAPsLogsResponse = modem_client.logs().await;
            logs_to_loki(
                logs_response,
                &http_client,
                settings.get_string("logs_address").unwrap(),
                &mut last_n_logs,
            )
            .await
            .unwrap_or_log();
            interval.tick().await;
        }
    });
    forever.await.unwrap_or_log();
}
