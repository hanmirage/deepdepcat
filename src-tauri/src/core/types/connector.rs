//! Connector and plugin types — used by the Settings view (stub backend).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub service: String,
    pub status: String,
    pub icon: Option<String>,
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub resource: String,
    pub resource_type: String,
    pub access: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub installed: bool,
    pub enabled: bool,
}
