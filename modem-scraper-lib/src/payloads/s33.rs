use chrono::offset::Utc;
use chrono::{DateTime, NaiveDateTime};
use log::Level;
use regex::{Captures, Regex};
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::fmt::Display;
use std::time::Duration;
use telegraf::*;
use tracing::debug;

/// Parses `0 days 13h:14m:15s` to a Duration
fn duration_deserializer<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let re = Regex::new(r"(?P<days>\d+) days (?P<hours>\d+)h:(?P<minutes>\d+)m:(?P<seconds>\d+)s")
        .unwrap();
    let captures = re.captures(s.as_str()).unwrap();

    // using a u64 for all these is a little inefficient, but that makes using it in Duration::new()
    // a lot easier, so
    let days = captures
        .name("days")
        .unwrap()
        .as_str()
        .parse::<u64>()
        .expect("Unable to convert captured days to a u16");
    let hours = captures
        .name("hours")
        .unwrap()
        .as_str()
        .parse::<u64>()
        .unwrap();
    let minutes = captures
        .name("minutes")
        .unwrap()
        .as_str()
        .parse::<u64>()
        .unwrap();
    let seconds = captures
        .name("seconds")
        .unwrap()
        .as_str()
        .parse::<u64>()
        .unwrap();

    let duration = Duration::new(
        days * 24 * 60 * 60 + hours * 60 * 60 + minutes * 60 + seconds,
        0,
    );

    debug!("Deserialized {} to {:?}", s, duration);

    Ok(duration)
}

/// Parses the locale-based timestamp to a UTC timestamp. Assumes modem is already reporting UTC.
fn timestamp_deserializer<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let datetime = NaiveDateTime::parse_from_str(&s, "%c").unwrap();
    Ok(DateTime::<Utc>::from_local(datetime, Utc))
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: Level,
    pub message: String,
}

fn log_parser<'de, D>(deserializer: D) -> Result<Vec<LogEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let re = Regex::new(r"0\^(?P<time>[:\d]+)\^(?P<date>[/\d]+)\^(?P<level>\d)\^(?P<message>.*)")
        .unwrap();

    let mut log_entries: Vec<LogEntry> = Vec::new();
    for line in s.split("}-{") {
        let captures = re
            .captures(line)
            .unwrap_or_else(|| panic!("Unable to parse: {}", line));

        // parse the date first
        let capture_time = captures.name("time").unwrap().as_str();
        let capture_date = captures.name("date").unwrap().as_str();
        let capture_datetime = capture_date.to_owned() + " " + capture_time;
        let naive_date = NaiveDateTime::parse_from_str(&capture_datetime, "%d/%m/%Y %T").unwrap();
        let timestamp = DateTime::<Utc>::from_local(naive_date, Utc);

        let level: Level = match captures
            .name("level")
            .unwrap()
            .as_str()
            .parse::<u8>()
            .unwrap()
        {
            3 => Level::Error,
            4 => Level::Warn,
            5 => Level::Info,
            6 => Level::Debug,
            _ => Level::Error,
        };
        let message: String = captures.name("message").unwrap().as_str().to_string();

        log_entries.push(LogEntry {
            timestamp,
            level,
            message,
        })
    }

    Ok(log_entries)
}

#[derive(Debug, Clone)]
pub enum Modulation {
    QAM256,
    OFDMPLC,
    SCQAM, // unclear if modulation method, but maybe
    Unknown,
}

impl Display for Modulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Modulation::QAM256 => write!(f, "QAM-256"),
            Modulation::OFDMPLC => write!(f, "OFDM-PLC"),
            Modulation::SCQAM => write!(f, "SC-QAM"),
            Modulation::Unknown => write!(f, "Unknown"),
        }
    }
}

impl IntoFieldData for Modulation {
    fn field_data(&self) -> FieldData {
        FieldData::Str(self.to_string())
    }
}

#[derive(Debug, Clone, Metric)]
#[measurement = "modem_downstream_channel"]
pub struct DownstreamChannel {
    #[telegraf(tag)]
    pub channel_id: u8,
    #[telegraf(tag)]
    pub modulation: Modulation,
    pub lock_status: bool,
    pub frequency: u32,
    pub power: u8,
    pub snr: u8,
    pub corrected: u32,      // TODO: does this need to be bigger?
    pub uncorrectables: u32, // TODO: does this need to be bigger?
}

#[derive(Debug, Clone, Metric)]
#[measurement = "modem_upstream_channel"]
pub struct UpstreamChannel {
    #[telegraf(tag)]
    pub channel_id: u8,
    #[telegraf(tag)]
    pub modulation: Modulation,
    pub lock_status: bool,
    pub frequency: u32,
    pub width: u32,
    pub power: f64,
}

#[derive(Debug, Clone)]
pub enum Channel {
    Downstream(DownstreamChannel),
    Upstream(UpstreamChannel),
}

const DOWNSTREAM_CHANNEL_PATTERN: &str = r"(?:\d+)\^(?P<lock_status>\w+)\^(?P<modulation>[\w\d ]+)\^(?P<channel_id>\d+)\^(?P<frequency>\d+)\^(?P<power>\d+)\^(?P<snr>\d+)\^(?P<corrected>\d+)\^(?P<uncorrectables>\d+)\^";
const UPSTREAM_CHANNEL_REGEX: &str = r"(?:\d+)\^(?P<lock_status>\w+)\^(?P<modulation>[\w\d -]+)\^(?P<channel_id>\d+)\^(?P<width>\d+)\^(?P<frequency>\d+)\^(?P<power>[\d.]+)\^";
fn channel_parser<'de, D>(deserializer: D) -> Result<Vec<Channel>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let mut channels: Vec<Channel> = Vec::new();
    let downstream_channel_regex = Regex::new(DOWNSTREAM_CHANNEL_PATTERN).unwrap();
    let upstream_channel_regex = Regex::new(UPSTREAM_CHANNEL_REGEX).unwrap();

    for line in s.split("|+|") {
        let captures: Captures;
        let mut is_downstream_channel: bool = false;
        if downstream_channel_regex.is_match(line) {
            captures = downstream_channel_regex.captures(line).unwrap();
            is_downstream_channel = true
        } else if upstream_channel_regex.is_match(line) {
            captures = upstream_channel_regex.captures(line).unwrap();
        } else {
            return Err(format!("Unable to match {} with any channel regex", line).as_str())
                .map_err(Error::custom);
        }

        let channel_id: u8 = captures
            .name("channel_id")
            .unwrap()
            .as_str()
            .parse::<u8>()
            .unwrap();
        let lock_status: bool = matches!(captures.name("lock_status").unwrap().as_str(), "Locked");
        let modulation: Modulation = match captures.name("modulation").unwrap().as_str() {
            "QAM256" => Modulation::QAM256,
            "OFDM PLC" => Modulation::OFDMPLC,
            "SC-QAM" => Modulation::SCQAM,
            _ => Modulation::Unknown,
        };
        let frequency: u32 = captures
            .name("frequency")
            .unwrap()
            .as_str()
            .parse()
            .unwrap();

        // different types, or different values
        if is_downstream_channel {
            let power: u8 = captures.name("power").unwrap().as_str().parse().unwrap();
            let snr: u8 = captures.name("snr").unwrap().as_str().parse().unwrap();
            let corrected: u32 = captures
                .name("corrected")
                .unwrap()
                .as_str()
                .parse()
                .unwrap();
            let uncorrectables: u32 = captures
                .name("uncorrectables")
                .unwrap()
                .as_str()
                .parse()
                .unwrap();
            channels.push(Channel::Downstream(DownstreamChannel {
                channel_id,
                lock_status,
                modulation,
                frequency,
                power,
                snr,
                corrected,
                uncorrectables,
            }))
        } else {
            let width: u32 = captures.name("width").unwrap().as_str().parse().unwrap();
            let power: f64 = captures.name("power").unwrap().as_str().parse().unwrap();
            channels.push(Channel::Upstream(UpstreamChannel {
                channel_id,
                lock_status,
                modulation,
                frequency,
                power,
                width,
            }))
        }
    }

    Ok(channels)
}

pub trait HasResult {
    fn get_result(&self) -> String;
}

macro_rules! impl_has_result {
    ($($t:ty),+ $(,)?) => ($(
        impl HasResult for $t {
            fn get_result(&self) -> String {
                return self.result.clone();
            }
        }
    )+)
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct LoginWithChallengeResponse {
    #[serde(rename = "LoginResult")]
    result: String,
}
impl_has_result!(LoginWithChallengeResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct LoginResponse {
    pub public_key: String,
    pub challenge: String,
    pub cookie: String,
    #[serde(rename = "LoginResult")]
    result: String,
}
impl_has_result!(LoginResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StatusStartupSequenceResponse {
    pub customer_conn_d_s_freq: String,
    pub customer_conn_d_s_comment: String,
    pub customer_conn_connectivity_status: String,
    pub customer_conn_connectivity_comment: String,
    pub customer_conn_boot_status: String,
    pub customer_conn_boot_comment: String,
    pub customer_conn_configuration_file_status: String,
    pub customer_conn_configuration_file_comment: String,
    pub customer_conn_security_status: String,
    pub customer_conn_security_comment: String,
    #[serde(rename = "GetCustomerStatusStartupSequenceResult")]
    result: String,
}
impl_has_result!(StatusStartupSequenceResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StatusConnectionInfoResponse {
    #[serde(deserialize_with = "duration_deserializer")]
    pub customer_conn_system_up_time: Duration,
    #[serde(deserialize_with = "timestamp_deserializer")]
    pub customer_cur_system_time: DateTime<Utc>,
    pub customer_conn_network_access: String,
    #[serde(rename = "GetCustomerStatusConnectionInfoResult")]
    result: String,
}
impl_has_result!(StatusConnectionInfoResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ArrisDeviceStatusResponse {
    pub firmware_version: String,
    pub internet_connection: String,
    pub downstream_frequency: String,
    pub downstream_signal_power: String,
    pub downstream_signal_snr: String,
    #[serde(rename = "GetArrisDeviceStatusResult")]
    result: String,
}
impl_has_result!(ArrisDeviceStatusResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ArrisRegisterInfoResponse {
    pub mac_address: String, // TODO: maybe make this a specific type?
    pub serial_number: String,
    pub model_name: String,
    #[serde(rename = "GetArrisRegisterInfoResult")]
    result: String,
}
impl_has_result!(ArrisRegisterInfoResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StatusDownstreamChannelInfo {
    #[serde(deserialize_with = "channel_parser")]
    pub customer_conn_downstream_channel: Vec<Channel>,
    #[serde(rename = "GetCustomerStatusDownstreamChannelInfoResult")]
    result: String,
}
impl_has_result!(StatusDownstreamChannelInfo);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StatusUpstreamChannelInfo {
    #[serde(deserialize_with = "channel_parser")]
    pub customer_conn_upstream_channel: Vec<Channel>,
    #[serde(rename = "GetCustomerStatusUpstreamChannelInfoResult")]
    result: String,
}
impl_has_result!(StatusUpstreamChannelInfo);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct GetMultipleHNAPsMetricsResponse {
    pub get_arris_device_status_response: ArrisDeviceStatusResponse,
    pub get_arris_register_info_response: ArrisRegisterInfoResponse,
    pub get_customer_status_connection_info_response: StatusConnectionInfoResponse,
    pub get_customer_status_downstream_channel_info_response: StatusDownstreamChannelInfo,
    pub get_customer_status_upstream_channel_info_response: StatusUpstreamChannelInfo,
    pub get_customer_status_startup_sequence_response: StatusStartupSequenceResponse,
    #[serde(rename = "GetMultipleHNAPsResult")]
    result: String,
}
impl_has_result!(GetMultipleHNAPsMetricsResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct StatusLogResponse {
    #[serde(deserialize_with = "log_parser")]
    pub customer_status_log_list: Vec<LogEntry>,
    #[serde(rename = "GetCustomerStatusLogResult")]
    result: String,
}
impl_has_result!(StatusLogResponse);

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct GetMultipleHNAPsLogsResponse {
    pub get_customer_status_log_response: StatusLogResponse,
    #[serde(rename = "GetMultipleHNAPsResult")]
    result: String,
}
impl_has_result!(GetMultipleHNAPsLogsResponse);
