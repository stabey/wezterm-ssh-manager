use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;

/// Password-free connection details emitted for every snapshot profile.
///
/// `extra` is intentionally retained: the Lua snapshot may contain newer SSH
/// options that an older TUI does not know about yet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SftpProfileData {
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u32>,
    pub auth: Option<String>,
    #[serde(rename = "privateKeys", alias = "private_keys")]
    pub private_keys: Vec<String>,
    pub password_env: Option<String>,
    #[serde(rename = "identityAgent", alias = "identity_agent")]
    pub identity_agent: Option<String>,
    #[serde(rename = "jumpHost", alias = "jump_host")]
    pub jump_host: Option<String>,
    #[serde(rename = "proxyCommand", alias = "proxy_command")]
    pub proxy_command: Option<String>,
    #[serde(rename = "readyTimeout", alias = "ready_timeout")]
    pub ready_timeout: Option<u64>,
    #[serde(rename = "keepaliveInterval", alias = "keepalive_interval")]
    pub keepalive_interval: Option<u64>,
    #[serde(rename = "keepaliveCountMax", alias = "keepalive_count_max")]
    pub keepalive_count_max: Option<u32>,
    pub host_key_policy: Option<String>,
    pub ssh_options: Option<JsonObject>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub group: String,
    pub editable: bool,
    pub source: String,
    pub host: String,
    pub user: String,
    pub port: u32,
    pub auth: String,
    pub has_password: bool,
    pub jump_host: String,
    pub icon: String,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sftp: Option<SftpProfileData>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub store_path: String,
    pub default_where: String,
    pub groups: Vec<String>,
    pub profiles: Vec<Profile>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum ManagerRequest {
    #[serde(rename = "connect")]
    Connect {
        id: String,
        #[serde(rename = "where")]
        where_: String,
    },
    #[serde(rename = "quick")]
    Quick {
        target: String,
        #[serde(rename = "where")]
        where_: String,
    },
    #[serde(rename = "hide")]
    Hide,
    #[serde(rename = "upsert")]
    Upsert { id: Option<String>, raw: JsonObject },
    #[serde(rename = "delete")]
    Delete { id: String },
    #[serde(rename = "copy_in")]
    CopyIn { id: String },
    #[serde(rename = "reload")]
    Reload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub v: u8,
    pub token: String,
    pub seq: u64,
    pub request: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MainPage {
    #[default]
    Manager,
    Sftp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManagerFocus {
    Groups,
    #[default]
    Hosts,
    Details,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileDraft {
    pub original_id: Option<String>,
    pub name: String,
    pub group: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub auth: String,
    pub password: String,
    pub jump_host: String,
    pub raw: JsonObject,
}
