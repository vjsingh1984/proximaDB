// Log parsing workers for different formats
//
// Provides:
// - Multi-format parsing (JSON, syslog, CEF, etc.)
// - Field extraction
// - Timestamp normalization
// - Parallel processing

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};

use crate::proto::proximadb_v1::{IngestionFormat, LogEntry, Severity, SqlValue};

/// Log parser for different formats
pub struct LogParser {
    /// Default timezone for logs without timezone
    #[allow(dead_code)]
    default_timezone: chrono_tz::Tz,
}

impl LogParser {
    /// Create a new log parser
    pub fn new() -> Self {
        Self {
            default_timezone: chrono_tz::UTC,
        }
    }

    /// Parse a batch of logs
    pub fn parse_batch(&self, logs: &[LogEntry], format: IngestionFormat) -> Result<Vec<LogEntry>> {
        logs.iter()
            .map(|log| self.parse_single(log, format))
            .collect()
    }

    /// Parse a single log entry
    fn parse_single(&self, log: &LogEntry, format: IngestionFormat) -> Result<LogEntry> {
        match format {
            IngestionFormat::Unspecified | IngestionFormat::Json | IngestionFormat::Ndjson => {
                // Already structured, just normalize
                self.normalize_log(log.clone())
            }
            IngestionFormat::SyslogRfc3164 | IngestionFormat::SyslogRfc5424 => {
                self.parse_syslog(log)
            }
            IngestionFormat::Otlp => self.parse_otlp(log),
            IngestionFormat::Fluent => self.parse_fluent(log),
            IngestionFormat::Cef => self.parse_cef(log),
            IngestionFormat::Leef => self.parse_leef(log),
            IngestionFormat::Ocsf => self.parse_ocsf(log),
        }
    }

    /// Normalize a log entry (timestamps, severity, etc.)
    fn normalize_log(&self, mut log: LogEntry) -> Result<LogEntry> {
        // Ensure timestamp is set
        if log.timestamp_ns == 0 {
            log.timestamp_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        }

        // Normalize severity
        if log.severity == Severity::Unspecified as i32 {
            log.severity = self.infer_severity(&log.message) as i32;
        }

        Ok(log)
    }

    /// Parse syslog format (RFC 3164/5424)
    fn parse_syslog(&self, log: &LogEntry) -> Result<LogEntry> {
        let message = &log.message;
        let mut parsed = log.clone();

        // Try RFC 5424 first, then RFC 3164
        if let Some(entry) = self.try_parse_rfc5424(message) {
            parsed = entry;
        } else if let Some(entry) = self.try_parse_rfc3164(message) {
            parsed = entry;
        }

        self.normalize_log(parsed)
    }

    /// Try to parse RFC 5424 syslog format
    fn try_parse_rfc5424(&self, message: &str) -> Option<LogEntry> {
        // RFC 5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID STRUCTURED-DATA MSG
        // Example: <34>1 2003-10-11T22:14:15.003Z mymachine.example.com su - ID47 - 'su root' failed

        if !message.starts_with('<') {
            return None;
        }

        let pri_end = message.find('>')?;
        let pri: u8 = message[1..pri_end].parse().ok()?;

        let rest = &message[pri_end + 1..];
        if !rest.starts_with('1') {
            return None; // Not RFC 5424
        }

        let parts: Vec<&str> = rest.splitn(7, ' ').collect();
        if parts.len() < 7 {
            return None;
        }

        let severity = self.pri_to_severity(pri);
        let timestamp = self.parse_timestamp(parts[1]).ok()?;

        Some(LogEntry {
            timestamp_ns: timestamp,
            severity: severity as i32,
            message: parts[6].to_string(),
            fields: HashMap::new(),
            source: Some(parts[2].to_string()),
            service: Some(parts[3].to_string()),
        })
    }

    /// Try to parse RFC 3164 syslog format
    fn try_parse_rfc3164(&self, message: &str) -> Option<LogEntry> {
        // RFC 3164: <PRI>TIMESTAMP HOSTNAME TAG: MSG
        // Example: <34>Oct 11 22:14:15 mymachine su: 'su root' failed

        if !message.starts_with('<') {
            return None;
        }

        let pri_end = message.find('>')?;
        let pri: u8 = message[1..pri_end].parse().ok()?;

        let rest = &message[pri_end + 1..];
        let severity = self.pri_to_severity(pri);

        // Parse timestamp (Mmm dd hh:mm:ss)
        if rest.len() < 15 {
            return None;
        }

        let timestamp_str = &rest[..15];
        let timestamp = self.parse_syslog_timestamp(timestamp_str).ok()?;

        let remaining = rest[15..].trim_start();
        let parts: Vec<&str> = remaining.splitn(2, ':').collect();

        let (hostname, tag) = if parts.len() >= 1 {
            let host_tag: Vec<&str> = parts[0].splitn(2, ' ').collect();
            if host_tag.len() >= 2 {
                (host_tag[0], host_tag[1])
            } else {
                (parts[0], "")
            }
        } else {
            ("", "")
        };

        let msg = if parts.len() >= 2 {
            parts[1].trim()
        } else {
            remaining
        };

        Some(LogEntry {
            timestamp_ns: timestamp,
            severity: severity as i32,
            message: msg.to_string(),
            fields: HashMap::new(),
            source: Some(hostname.to_string()),
            service: Some(tag.to_string()),
        })
    }

    /// Parse OTLP format (OpenTelemetry)
    fn parse_otlp(&self, log: &LogEntry) -> Result<LogEntry> {
        // OTLP logs are already structured, just normalize
        self.normalize_log(log.clone())
    }

    /// Parse Fluent format (Fluent Bit/Fluentd)
    fn parse_fluent(&self, log: &LogEntry) -> Result<LogEntry> {
        // Fluent forward protocol logs are JSON, just normalize
        self.normalize_log(log.clone())
    }

    /// Parse CEF format (Common Event Format - ArcSight)
    fn parse_cef(&self, log: &LogEntry) -> Result<LogEntry> {
        let message = &log.message;
        let mut parsed = log.clone();

        // CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extension
        if message.starts_with("CEF:") {
            let parts: Vec<&str> = message[4..].splitn(8, '|').collect();
            if parts.len() >= 7 {
                parsed.source = Some(format!("{} {}", parts[1], parts[2]));
                parsed.message = parts[5].to_string();
                parsed.severity = self.cef_severity_to_severity(parts[6]) as i32;

                // Parse extension key-value pairs
                if parts.len() >= 8 {
                    let extension = self.parse_cef_extension(parts[7]);
                    parsed.fields = extension
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k,
                                SqlValue {
                                    value: Some(
                                        crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                            v,
                                        ),
                                    ),
                                },
                            )
                        })
                        .collect();
                }
            }
        }

        self.normalize_log(parsed)
    }

    /// Parse LEEF format (Log Event Extended Format - IBM QRadar)
    fn parse_leef(&self, log: &LogEntry) -> Result<LogEntry> {
        let message = &log.message;
        let mut parsed = log.clone();

        // LEEF:Version|Vendor|Product|Version|EventID|Extension
        if message.starts_with("LEEF:") {
            let parts: Vec<&str> = message[5..].splitn(6, '|').collect();
            if parts.len() >= 5 {
                parsed.source = Some(format!("{} {}", parts[1], parts[2]));
                parsed.message = parts[4].to_string();

                // Parse extension key-value pairs
                if parts.len() >= 6 {
                    let extension = self.parse_leef_extension(parts[5]);
                    parsed.fields = extension
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k,
                                SqlValue {
                                    value: Some(
                                        crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                            v,
                                        ),
                                    ),
                                },
                            )
                        })
                        .collect();
                }
            }
        }

        self.normalize_log(parsed)
    }

    /// Parse OCSF format (Open Cybersecurity Schema Framework)
    fn parse_ocsf(&self, log: &LogEntry) -> Result<LogEntry> {
        // OCSF is JSON-based, just normalize
        self.normalize_log(log.clone())
    }

    /// Convert syslog PRI to severity
    fn pri_to_severity(&self, pri: u8) -> Severity {
        let severity = pri & 0x07;
        match severity {
            0 => Severity::Fatal, // Emergency
            1 => Severity::Fatal, // Alert
            2 => Severity::Fatal, // Critical
            3 => Severity::Error, // Error
            4 => Severity::Warn,  // Warning
            5 => Severity::Info,  // Notice
            6 => Severity::Info,  // Informational
            7 => Severity::Debug, // Debug
            _ => Severity::Info,
        }
    }

    /// Convert CEF severity string to Severity
    fn cef_severity_to_severity(&self, sev: &str) -> Severity {
        match sev.trim().parse::<u8>() {
            Ok(n) if n <= 3 => Severity::Info,
            Ok(n) if n <= 6 => Severity::Warn,
            Ok(n) if n <= 8 => Severity::Error,
            Ok(_) => Severity::Fatal,
            Err(_) => match sev.to_lowercase().as_str() {
                "low" => Severity::Info,
                "medium" => Severity::Warn,
                "high" => Severity::Error,
                "very-high" | "critical" => Severity::Fatal,
                _ => Severity::Info,
            },
        }
    }

    /// Infer severity from message content
    fn infer_severity(&self, message: &str) -> Severity {
        let lower = message.to_lowercase();
        if lower.contains("fatal") || lower.contains("critical") || lower.contains("emergency") {
            Severity::Fatal
        } else if lower.contains("error") || lower.contains("fail") || lower.contains("exception") {
            Severity::Error
        } else if lower.contains("warn") || lower.contains("alert") {
            Severity::Warn
        } else if lower.contains("debug") || lower.contains("trace") {
            Severity::Debug
        } else {
            Severity::Info
        }
    }

    /// Parse ISO 8601 timestamp
    fn parse_timestamp(&self, ts: &str) -> Result<i64> {
        let dt = DateTime::parse_from_rfc3339(ts).context("Failed to parse RFC 3339 timestamp")?;
        Ok(dt.timestamp_nanos_opt().unwrap_or(0))
    }

    /// Parse syslog timestamp (Mmm dd hh:mm:ss)
    fn parse_syslog_timestamp(&self, ts: &str) -> Result<i64> {
        let now = Utc::now();
        let year = now.format("%Y").to_string();
        let full_ts = format!("{} {}", ts, year);

        let naive = NaiveDateTime::parse_from_str(&full_ts, "%b %d %H:%M:%S %Y")
            .context("Failed to parse syslog timestamp")?;

        Ok(naive.and_utc().timestamp_nanos_opt().unwrap_or(0))
    }

    /// Parse CEF extension key-value pairs
    fn parse_cef_extension(&self, ext: &str) -> HashMap<String, String> {
        let mut attrs = HashMap::new();
        let mut current_key = String::new();
        let mut current_value = String::new();
        let mut in_value = false;

        for part in ext.split(' ') {
            if let Some(eq_pos) = part.find('=') {
                if in_value && !current_key.is_empty() {
                    attrs.insert(current_key.clone(), current_value.trim().to_string());
                }
                current_key = part[..eq_pos].to_string();
                current_value = part[eq_pos + 1..].to_string();
                in_value = true;
            } else if in_value {
                current_value.push(' ');
                current_value.push_str(part);
            }
        }

        if in_value && !current_key.is_empty() {
            attrs.insert(current_key, current_value.trim().to_string());
        }

        attrs
    }

    /// Parse LEEF extension key-value pairs
    fn parse_leef_extension(&self, ext: &str) -> HashMap<String, String> {
        let mut attrs = HashMap::new();
        // LEEF uses tab as separator by default
        let separator = if ext.contains('\t') { '\t' } else { ' ' };

        for pair in ext.split(separator) {
            if let Some(eq_pos) = pair.find('=') {
                let key = pair[..eq_pos].to_string();
                let value = pair[eq_pos + 1..].to_string();
                attrs.insert(key, value);
            }
        }

        attrs
    }
}

impl Default for LogParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_new() {
        let parser = LogParser::new();
        assert_eq!(parser.default_timezone, chrono_tz::UTC);
    }

    #[test]
    fn test_infer_severity() {
        let parser = LogParser::new();

        assert_eq!(
            parser.infer_severity("Fatal error occurred"),
            Severity::Fatal
        );
        assert_eq!(
            parser.infer_severity("Error: something failed"),
            Severity::Error
        );
        assert_eq!(
            parser.infer_severity("Warning: low disk space"),
            Severity::Warn
        );
        assert_eq!(
            parser.infer_severity("Debug: processing request"),
            Severity::Debug
        );
        assert_eq!(parser.infer_severity("Normal log message"), Severity::Info);
    }

    #[test]
    fn test_pri_to_severity() {
        let parser = LogParser::new();

        assert_eq!(parser.pri_to_severity(0), Severity::Fatal); // Emergency
        assert_eq!(parser.pri_to_severity(3), Severity::Error); // Error
        assert_eq!(parser.pri_to_severity(4), Severity::Warn); // Warning
        assert_eq!(parser.pri_to_severity(6), Severity::Info); // Informational
        assert_eq!(parser.pri_to_severity(7), Severity::Debug); // Debug
    }

    #[test]
    fn test_cef_severity() {
        let parser = LogParser::new();

        assert_eq!(parser.cef_severity_to_severity("1"), Severity::Info);
        assert_eq!(parser.cef_severity_to_severity("5"), Severity::Warn);
        assert_eq!(parser.cef_severity_to_severity("7"), Severity::Error);
        assert_eq!(parser.cef_severity_to_severity("10"), Severity::Fatal);
        assert_eq!(parser.cef_severity_to_severity("High"), Severity::Error);
    }

    #[test]
    fn test_parse_cef_extension() {
        let parser = LogParser::new();
        let ext = "src=10.0.0.1 dst=10.0.0.2 spt=1234";
        let attrs = parser.parse_cef_extension(ext);

        assert_eq!(attrs.get("src"), Some(&"10.0.0.1".to_string()));
        assert_eq!(attrs.get("dst"), Some(&"10.0.0.2".to_string()));
        assert_eq!(attrs.get("spt"), Some(&"1234".to_string()));
    }
}
