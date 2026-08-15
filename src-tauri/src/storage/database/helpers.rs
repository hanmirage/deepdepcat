use crate::core::types::SessionStatus;
use chrono::{DateTime, Utc};

pub fn parse_session_status(s: String) -> SessionStatus {
    match s.as_str() {
        "active" => SessionStatus::Active,
        "idle" => SessionStatus::Idle,
        "archived" => SessionStatus::Archived,
        "error" => SessionStatus::Error,
        _ => SessionStatus::Active,
    }
}

pub fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
