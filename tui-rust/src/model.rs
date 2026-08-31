use std::collections::HashMap;

use serde_json::{Map, Number, Value};

use crate::types::{JsonObject, Profile, ProfileDraft, SftpProfileData, Snapshot};

pub const ALL_GROUPS: &str = "__all__";

fn object_or_empty(value: Option<&Value>) -> JsonObject {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

fn string_value(value: Option<&Value>, fallback: &str) -> String {
    match value {
        None | Some(Value::Null) => fallback.to_owned(),
        Some(value) => js_string(value),
    }
}

fn number_value(value: Option<&Value>, fallback: u32) -> u32 {
    let parsed = match value {
        Some(Value::Number(value)) => value.as_f64(),
        Some(Value::String(value)) if value.trim().is_empty() => Some(0.0),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        Some(Value::Bool(value)) => Some(u8::from(*value) as f64),
        Some(Value::Null) => Some(0.0),
        _ => None,
    };
    match parsed {
        Some(value)
            if value.is_finite()
                && value.fract() == 0.0
                && value >= 0.0
                && value <= u32::MAX as f64 =>
        {
            value as u32
        }
        _ => fallback,
    }
}

fn object_values(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(values)) => values.iter().collect(),
        Some(Value::Object(values)) => values.values().collect(),
        _ => Vec::new(),
    }
}

pub fn normalize_snapshot(value: &Value) -> Snapshot {
    let input = value.as_object();
    let raw_profiles = object_values(input.and_then(|value| value.get("profiles")));
    let raw_groups = object_values(input.and_then(|value| value.get("groups")));

    let profiles = raw_profiles
        .into_iter()
        .filter_map(Value::as_object)
        .map(|item| {
            let id = string_value(item.get("id"), "");
            let raw = item.get("raw").map(|value| object_or_empty(Some(value)));
            let sftp = item.get("sftp").map(|value| {
                serde_json::from_value::<SftpProfileData>(Value::Object(object_or_empty(Some(
                    value,
                ))))
                .unwrap_or_default()
            });
            Profile {
                id: id.clone(),
                name: string_value(item.get("name"), if id.is_empty() { "?" } else { &id }),
                group: string_value(item.get("group"), ""),
                editable: !matches!(item.get("editable"), Some(Value::Bool(false))),
                source: string_value(item.get("source"), "store"),
                host: string_value(item.get("host"), ""),
                user: string_value(item.get("user"), ""),
                port: number_value(item.get("port"), 22),
                auth: string_value(item.get("auth"), ""),
                has_password: matches!(item.get("has_password"), Some(Value::Bool(true)))
                    || matches!(item.get("hasPassword"), Some(Value::Bool(true))),
                jump_host: string_value(item.get("jumpHost"), ""),
                icon: string_value(item.get("icon"), ""),
                color: string_value(item.get("color"), ""),
                raw,
                sftp,
            }
        })
        .collect::<Vec<_>>();

    let mut groups = raw_groups
        .into_iter()
        .map(|group| string_value(Some(group), ""))
        .filter(|group| !group.is_empty())
        .collect::<Vec<_>>();
    for profile in &profiles {
        if !profile.group.is_empty() && !groups.contains(&profile.group) {
            groups.push(profile.group.clone());
        }
    }

    Snapshot {
        store_path: string_value(
            input.and_then(|value| value.get("store_path").or_else(|| value.get("storePath"))),
            "",
        ),
        default_where: string_value(
            input.and_then(|value| {
                value
                    .get("default_where")
                    .or_else(|| value.get("defaultWhere"))
            }),
            "tab",
        ),
        groups,
        profiles,
    }
}

pub fn profile_target(profile: &Profile) -> String {
    let host = if profile.host.is_empty() {
        "?"
    } else {
        &profile.host
    };
    let prefix = if profile.user.is_empty() {
        String::new()
    } else {
        format!("{}@", profile.user)
    };
    let suffix = if profile.port != 0 && profile.port != 22 {
        format!(":{}", profile.port)
    } else {
        String::new()
    };
    format!("{prefix}{host}{suffix}")
}

pub fn profile_matches(profile: &Profile, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [
        profile.id.as_str(),
        profile.name.as_str(),
        profile.group.as_str(),
        profile.host.as_str(),
        profile.user.as_str(),
        profile.jump_host.as_str(),
    ]
    .join(" ")
    .to_lowercase()
    .contains(&needle)
}

pub fn visible_profiles<'a>(snapshot: &'a Snapshot, group: &str, query: &str) -> Vec<&'a Profile> {
    snapshot
        .profiles
        .iter()
        .filter(|profile| {
            (group == ALL_GROUPS || profile.group == group) && profile_matches(profile, query)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSummary {
    pub id: String,
    pub label: String,
    pub count: usize,
}

pub fn group_summaries(snapshot: &Snapshot) -> Vec<GroupSummary> {
    let mut counts = HashMap::<&str, usize>::new();
    for profile in &snapshot.profiles {
        *counts.entry(profile.group.as_str()).or_default() += 1;
    }
    let mut groups = snapshot
        .groups
        .iter()
        .filter(|group| !group.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    for profile in &snapshot.profiles {
        if !profile.group.is_empty() && !groups.contains(&profile.group) {
            groups.push(profile.group.clone());
        }
    }

    let mut summaries = vec![GroupSummary {
        id: ALL_GROUPS.to_owned(),
        label: "全部".to_owned(),
        count: snapshot.profiles.len(),
    }];
    summaries.extend(groups.into_iter().map(|group| GroupSummary {
        count: counts.get(group.as_str()).copied().unwrap_or_default(),
        id: group.clone(),
        label: group,
    }));
    summaries
}

#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u32>,
}

#[cfg(test)]
fn positive_integer(value: &str) -> Option<u32> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value.fract() == 0.0 && value > 0.0 && value <= u32::MAX as f64)
        .then_some(value as u32)
}

#[cfg(test)]
fn parse_target(spec: &str) -> ParsedTarget {
    let mut rest = spec.trim();
    let mut user = None;
    if let Some(at) = rest.find('@') {
        if at > 0 {
            user = Some(rest[..at].to_owned());
        }
        rest = &rest[at + 1..];
    }

    if rest.starts_with('[')
        && let Some(close) = rest.find(']')
    {
        let host = rest[1..close].to_owned();
        let suffix = &rest[close + 1..];
        let port = suffix.strip_prefix(':').and_then(positive_integer);
        return ParsedTarget { host, user, port };
    }

    if let Some(colon) = rest.rfind(':')
        && colon > 0
        && rest.find(':') == Some(colon)
        && let Some(port) = positive_integer(&rest[colon + 1..])
    {
        return ParsedTarget {
            host: rest[..colon].to_owned(),
            user,
            port: Some(port),
        };
    }
    ParsedTarget {
        host: rest.to_owned(),
        user,
        port: None,
    }
}

fn should_remove(value: &Value) -> bool {
    value.is_null() || matches!(value, Value::String(value) if value.is_empty())
}

fn set_path(object: &mut JsonObject, path: &str, value: Option<Value>) {
    let parts = path.split('.').collect::<Vec<_>>();
    let Some((last, parents)) = parts.split_last() else {
        return;
    };
    let mut current = object;
    for key in parents {
        let entry = current
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("object inserted above");
    }
    match value {
        None => {
            current.remove(*last);
        }
        Some(value) if should_remove(&value) => {
            current.remove(*last);
        }
        Some(value) => {
            current.insert((*last).to_owned(), value);
        }
    }
}

fn option_value<'a>(raw: &'a JsonObject, flat: &str, nested: &str) -> Option<&'a Value> {
    let nested_value = raw
        .get("options")
        .and_then(Value::as_object)
        .and_then(|options| options.get(nested));
    match nested_value {
        Some(Value::Null) | None => raw.get(flat),
        value => value,
    }
}

pub fn draft_from_profile(profile: Option<&Profile>, initial_group: &str) -> ProfileDraft {
    let raw = profile
        .and_then(|profile| profile.raw.clone())
        .unwrap_or_default();
    let profile_name = profile.map(|profile| profile.name.as_str()).unwrap_or("");
    let profile_group = profile
        .map(|profile| profile.group.as_str())
        .unwrap_or(initial_group);
    let profile_host = profile.map(|profile| profile.host.as_str()).unwrap_or("");
    let profile_user = profile.map(|profile| profile.user.as_str()).unwrap_or("");
    let profile_auth = profile.map(|profile| profile.auth.as_str()).unwrap_or("");
    let profile_jump = profile
        .map(|profile| profile.jump_host.as_str())
        .unwrap_or("");
    let profile_port = profile.map(|profile| profile.port).unwrap_or(22);

    ProfileDraft {
        original_id: profile
            .map(|profile| profile.id.clone())
            .filter(|id| !id.is_empty()),
        name: string_value(raw.get("name"), profile_name),
        group: string_value(raw.get("group"), profile_group),
        host: string_value(option_value(&raw, "host", "host"), profile_host),
        port: option_value(&raw, "port", "port")
            .filter(|value| !value.is_null())
            .map(js_string)
            .unwrap_or_else(|| profile_port.to_string()),
        user: string_value(option_value(&raw, "user", "user"), profile_user),
        auth: string_value(option_value(&raw, "auth", "auth"), profile_auth),
        password: String::new(),
        jump_host: string_value(option_value(&raw, "jumpHost", "jumpHost"), profile_jump),
        raw,
    }
}

#[cfg(test)]
fn draft_from_target(target: &str, initial_group: &str) -> ProfileDraft {
    let parsed = parse_target(target);
    ProfileDraft {
        original_id: None,
        name: parsed.host.clone(),
        group: initial_group.to_owned(),
        host: parsed.host,
        port: parsed.port.unwrap_or(22).to_string(),
        user: parsed.user.unwrap_or_default(),
        auth: String::new(),
        password: String::new(),
        jump_host: String::new(),
        raw: JsonObject::new(),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawDraftResult {
    pub raw: Option<JsonObject>,
    pub error: Option<String>,
}

impl RawDraftResult {
    fn success(raw: JsonObject) -> Self {
        Self {
            raw: Some(raw),
            error: None,
        }
    }

    fn error(error: impl Into<String>) -> Self {
        Self {
            raw: None,
            error: Some(error.into()),
        }
    }
}

pub fn raw_from_draft(draft: &ProfileDraft) -> RawDraftResult {
    let host = draft.host.trim();
    if host.is_empty() {
        return RawDraftResult::error("主机不能为空");
    }
    let port_text = draft.port.trim();
    let port = if port_text.is_empty() {
        Some(22)
    } else {
        port_text.parse::<f64>().ok().and_then(|port| {
            (port.is_finite() && port.fract() == 0.0 && (1.0..=65535.0).contains(&port))
                .then_some(port as u32)
        })
    };
    let Some(port @ 1..=65535) = port else {
        return RawDraftResult::error("端口需要是 1–65535 的整数");
    };

    let mut raw = draft.raw.clone();
    let nested = raw.contains_key("options");
    let path = |name: &str| {
        if nested {
            format!("options.{name}")
        } else {
            name.to_owned()
        }
    };
    set_path(
        &mut raw,
        "name",
        Some(Value::String(if draft.name.trim().is_empty() {
            host.to_owned()
        } else {
            draft.name.trim().to_owned()
        })),
    );
    set_path(
        &mut raw,
        "group",
        Some(Value::String(draft.group.trim().to_owned())),
    );
    set_path(
        &mut raw,
        &path("host"),
        Some(Value::String(host.to_owned())),
    );
    set_path(
        &mut raw,
        &path("port"),
        (port != 22).then(|| Value::Number(Number::from(port))),
    );
    set_path(
        &mut raw,
        &path("user"),
        Some(Value::String(draft.user.trim().to_owned())),
    );
    set_path(
        &mut raw,
        &path("auth"),
        Some(Value::String(draft.auth.trim().to_owned())),
    );
    if !draft.password.is_empty() {
        set_path(
            &mut raw,
            &path("password"),
            Some(Value::String(draft.password.clone())),
        );
    }
    set_path(
        &mut raw,
        &path("jumpHost"),
        Some(Value::String(draft.jump_host.trim().to_owned())),
    );
    RawDraftResult::success(raw)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixture() -> Snapshot {
        normalize_snapshot(&json!({
            "store_path": "/tmp/profiles.lua",
            "default_where": "tab",
            "groups": ["prod"],
            "profiles": [
                {
                    "id": "prod/db",
                    "name": "数据库",
                    "group": "prod",
                    "editable": true,
                    "host": "10.0.0.8",
                    "user": "ops",
                    "port": 2222,
                    "has_password": true,
                    "sftp": {"host": "files.internal", "user": "ops", "privateKeys": ["~/.ssh/id_ed25519"]},
                    "raw": {"name": "数据库", "group": "prod", "options": {"host": "10.0.0.8", "user": "ops", "port": 2222}}
                },
                {"id": "lab", "name": "实验机", "group": "lab", "editable": false, "host": "lab.local", "port": 22}
            ]
        }))
    }

    #[test]
    fn normalizes_profiles_and_discovers_groups() {
        let snapshot = fixture();
        assert_eq!(snapshot.store_path, "/tmp/profiles.lua");
        assert_eq!(snapshot.groups, ["prod", "lab"]);
        assert!(snapshot.profiles[0].has_password);
        assert_eq!(
            snapshot.profiles[0]
                .sftp
                .as_ref()
                .and_then(|sftp| sftp.host.as_deref()),
            Some("files.internal")
        );
        assert_eq!(
            group_summaries(&snapshot),
            vec![
                GroupSummary {
                    id: ALL_GROUPS.into(),
                    label: "全部".into(),
                    count: 2
                },
                GroupSummary {
                    id: "prod".into(),
                    label: "prod".into(),
                    count: 1
                },
                GroupSummary {
                    id: "lab".into(),
                    label: "lab".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn filters_and_renders_targets() {
        let snapshot = fixture();
        assert_eq!(
            visible_profiles(&snapshot, "prod", "OPS")
                .into_iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            ["prod/db"]
        );
        assert_eq!(visible_profiles(&snapshot, ALL_GROUPS, "实验")[0].id, "lab");
        assert_eq!(profile_target(&snapshot.profiles[0]), "ops@10.0.0.8:2222");
        assert_eq!(profile_target(&snapshot.profiles[1]), "lab.local");
    }

    #[test]
    fn parses_targets_and_builds_drafts() {
        assert_eq!(
            parse_target("ops@example.com:2200"),
            ParsedTarget {
                host: "example.com".into(),
                user: Some("ops".into()),
                port: Some(2200)
            }
        );
        assert_eq!(
            parse_target("root@[2001:db8::1]:2222"),
            ParsedTarget {
                host: "2001:db8::1".into(),
                user: Some("root".into()),
                port: Some(2222)
            }
        );
        let draft = draft_from_target("ops@example.com:2200", "prod");
        assert_eq!(draft.host, "example.com");
        assert_eq!(draft.group, "prod");
        assert_eq!(draft.port, "2200");
    }

    #[test]
    fn preserves_nested_raw_data_and_blank_password() {
        let snapshot = fixture();
        let mut draft = draft_from_profile(Some(&snapshot.profiles[0]), "");
        draft.name = "DB".into();
        draft.password.clear();
        let result = raw_from_draft(&draft);
        assert_eq!(
            result.raw,
            json!({
                "name": "DB",
                "group": "prod",
                "options": {"host": "10.0.0.8", "user": "ops", "port": 2222}
            })
            .as_object()
            .cloned()
        );
        assert!(result.error.is_none());
    }

    #[test]
    fn validates_host_and_port() {
        let mut draft = draft_from_target("example.com", "");
        draft.port = "70000".into();
        assert_eq!(
            raw_from_draft(&draft).error.as_deref(),
            Some("端口需要是 1–65535 的整数")
        );
        draft.port = "22".into();
        draft.host.clear();
        assert_eq!(
            raw_from_draft(&draft).error.as_deref(),
            Some("主机不能为空")
        );
    }

    #[test]
    fn mirrors_javascript_number_and_nullish_draft_rules() {
        let mut draft = draft_from_target("example.com", "");
        draft.port = "22.0".into();
        assert!(raw_from_draft(&draft).error.is_none());
        assert_eq!(parse_target("host:2.2e1").port, Some(22));

        let mut profile = fixture().profiles.remove(0);
        profile.group.clear();
        profile.raw = Some(json!({"port": null}).as_object().expect("object").clone());
        let draft = draft_from_profile(Some(&profile), "selected-group");
        assert_eq!(draft.group, "");
        assert_eq!(draft.port, "2222");
    }
}
