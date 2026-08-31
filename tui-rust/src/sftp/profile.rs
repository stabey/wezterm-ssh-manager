use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::types::Profile;

use super::types::{
    AgentEndpoint, CompatibilityIssue, CompatibilityIssueSeverity, CredentialOverrides,
    PrivateKeySource, ProfileConnectionOverrides, ProfileConnectionResult, SftpAuthentication,
    SftpConnectionOptions,
};

pub fn connection_from_profile(
    profile: &Profile,
    profiles: &[Profile],
    overrides: &ProfileConnectionOverrides,
) -> ProfileConnectionResult {
    let mut issues = Vec::new();
    let environment = overrides
        .environment
        .clone()
        .unwrap_or_else(|| std::env::vars().collect::<HashMap<_, _>>());
    let mut connection = base_connection(
        profile,
        &overrides.credentials,
        &environment,
        &mut issues,
        "",
    );
    add_unsupported_options(profile, &mut issues, "");

    let options = options_of(profile);
    let jump_spec = text(options.get("jumpHost").or_else(|| options.get("jump_host")))
        .or_else(|| non_empty(&profile.jump_host));
    if let Some(jump_spec) = jump_spec {
        if jump_spec.contains(',') {
            issues.push(issue(
                "jumpHost",
                CompatibilityIssueSeverity::Unsupported,
                "Only one jump host is supported; comma-separated chains require OpenSSH",
            ));
        } else if let Some(jump_profile) = profiles
            .iter()
            .find(|candidate| candidate.id == jump_spec || candidate.name == jump_spec)
        {
            let jump_options = options_of(jump_profile);
            if text(
                jump_options
                    .get("jumpHost")
                    .or_else(|| jump_options.get("jump_host")),
            )
            .or_else(|| non_empty(&jump_profile.jump_host))
            .is_some()
            {
                issues.push(issue(
                    "jumpHost",
                    CompatibilityIssueSeverity::Unsupported,
                    "Nested jump profiles are not supported; select a profile with one jump host",
                ));
            }
            let jump = base_connection(
                jump_profile,
                &overrides.jump,
                &environment,
                &mut issues,
                "jump.",
            );
            add_unsupported_options(jump_profile, &mut issues, "jump.");
            connection.jump = Some(Box::new(jump));
        } else if let Some(parsed) = parse_target(&jump_spec) {
            connection.jump = Some(Box::new(SftpConnectionOptions {
                host: parsed.host,
                port: parsed.port.unwrap_or(22),
                username: parsed.username,
                authentication: SftpAuthentication {
                    password: overrides.jump.password.clone(),
                    private_keys: overrides.jump.private_keys.clone().unwrap_or_default(),
                    agent: overrides
                        .jump
                        .agent
                        .clone()
                        .or(Some(AgentEndpoint::Default)),
                },
                ..SftpConnectionOptions::default()
            }));
        } else {
            issues.push(issue(
                "jumpHost",
                CompatibilityIssueSeverity::Unsupported,
                format!("Cannot parse jump host {jump_spec}"),
            ));
        }
    }

    let supported = !issues
        .iter()
        .any(|item| item.severity == CompatibilityIssueSeverity::Unsupported);
    ProfileConnectionResult {
        connection,
        issues,
        supported,
    }
}

fn base_connection(
    profile: &Profile,
    overrides: &CredentialOverrides,
    environment: &HashMap<String, String>,
    issues: &mut Vec<CompatibilityIssue>,
    prefix: &str,
) -> SftpConnectionOptions {
    let options = options_of(profile);
    let host = text(options.get("host"))
        .or_else(|| non_empty(&profile.host))
        .unwrap_or_else(|| profile.name.clone());
    let username = text(options.get("user")).or_else(|| non_empty(&profile.user));
    let port = positive_number(options.get("port"))
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| (profile.port > 0).then_some(profile.port))
        .unwrap_or(22);
    let ready_timeout = duration_option(
        options
            .get("readyTimeout")
            .or_else(|| options.get("ready_timeout")),
    );
    let keepalive_interval = duration_option(
        options
            .get("keepaliveInterval")
            .or_else(|| options.get("keepalive_interval")),
    );
    let keepalive_count_max = positive_number(
        options
            .get("keepaliveCountMax")
            .or_else(|| options.get("keepalive_count_max")),
    )
    .and_then(|value| usize::try_from(value).ok());

    SftpConnectionOptions {
        host,
        port,
        username,
        authentication: authentication_for(
            profile,
            &options,
            overrides,
            environment,
            issues,
            prefix,
        ),
        ready_timeout,
        keepalive_interval,
        keepalive_count_max,
        jump: None,
        host_verifier: None,
    }
}

fn authentication_for(
    profile: &Profile,
    options: &Map<String, Value>,
    overrides: &CredentialOverrides,
    environment: &HashMap<String, String>,
    issues: &mut Vec<CompatibilityIssue>,
    prefix: &str,
) -> SftpAuthentication {
    let mode = text(options.get("auth"))
        .or_else(|| non_empty(&profile.auth))
        .unwrap_or_default();
    let private_keys = overrides.private_keys.clone().unwrap_or_else(|| {
        parse_private_keys(
            options
                .get("privateKeys")
                .or_else(|| options.get("private_keys")),
        )
    });
    let password_environment = text(options.get("password_env"));
    let password = overrides
        .password
        .as_deref()
        .and_then(non_empty)
        .or_else(|| {
            password_environment
                .as_ref()
                .and_then(|name| environment.get(name))
                .and_then(|value| non_empty(value))
        });

    let configured_agent = text(options.get("identityAgent"))
        .or_else(|| text(options.get("identity_agent")))
        .map(AgentEndpoint::Path)
        .or_else(|| match options.get("agent").and_then(Value::as_bool) {
            Some(true) => Some(AgentEndpoint::Default),
            _ => None,
        });
    let agent = overrides.agent.clone().or_else(|| match mode.as_str() {
        "agent" => configured_agent.or(Some(AgentEndpoint::Default)),
        "" | "publicKey" => configured_agent,
        _ => None,
    });

    if options.get("password_cmd").is_some() && password.is_none() {
        issues.push(issue(
            format!("{prefix}password_cmd"),
            CompatibilityIssueSeverity::NeedsInput,
            "password_cmd is not executed by the TUI; prompt for a password or resolve it before connecting",
        ));
    }
    if matches!(mode.as_str(), "password" | "keyboardInteractive") && password.is_none() {
        issues.push(issue(
            format!("{prefix}password"),
            CompatibilityIssueSeverity::NeedsInput,
            format!(
                "{} password must be supplied by the UI",
                if prefix.is_empty() {
                    "Target"
                } else {
                    "Jump host"
                }
            ),
        ));
    }
    if mode == "publicKey" && private_keys.is_empty() && agent.is_none() {
        issues.push(issue(
            format!("{prefix}privateKeys"),
            CompatibilityIssueSeverity::NeedsInput,
            format!(
                "{} private key is not present in the snapshot",
                if prefix.is_empty() {
                    "Target"
                } else {
                    "Jump host"
                }
            ),
        ));
    }
    if mode == "keyboardInteractive" {
        issues.push(issue(
            format!("{prefix}auth"),
            CompatibilityIssueSeverity::Unsupported,
            "Interactive challenge/response authentication is not supported by the SFTP client",
        ));
    } else if !mode.is_empty() && !matches!(mode.as_str(), "password" | "publicKey" | "agent") {
        issues.push(issue(
            format!("{prefix}auth"),
            CompatibilityIssueSeverity::Unsupported,
            format!("Authentication mode {mode} is not supported by the SFTP client"),
        ));
    }

    SftpAuthentication {
        password,
        private_keys,
        agent,
    }
}

fn add_unsupported_options(profile: &Profile, issues: &mut Vec<CompatibilityIssue>, prefix: &str) {
    let options = options_of(profile);
    for key in [
        "proxyCommand",
        "proxy_command",
        "socksProxyHost",
        "httpProxyHost",
    ] {
        if options
            .get(key)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            issues.push(issue(
                format!("{prefix}{key}"),
                CompatibilityIssueSeverity::Unsupported,
                format!("{key} is not supported by the integrated SFTP client"),
            ));
        }
    }
    if options.get("algorithms").is_some() {
        issues.push(issue(
            format!("{prefix}algorithms"),
            CompatibilityIssueSeverity::Warning,
            "Custom OpenSSH algorithm lists are not mapped to russh and will be ignored",
        ));
    }
    if options.get("ssh_options").is_some() {
        issues.push(issue(
            format!("{prefix}ssh_options"),
            CompatibilityIssueSeverity::Warning,
            "Arbitrary OpenSSH -o options are not interpreted by russh and will be ignored",
        ));
    }
    if options.get("host_key_policy").is_some() {
        issues.push(issue(
            format!("{prefix}host_key_policy"),
            CompatibilityIssueSeverity::Warning,
            "OpenSSH host-key policy is not mapped automatically; provide a host verifier when required",
        ));
    }
}

fn options_of(profile: &Profile) -> Map<String, Value> {
    let raw = profile.raw.clone().unwrap_or_default();
    let mut options = raw
        .get("options")
        .and_then(Value::as_object)
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or(raw);
    if let Some(sftp) = &profile.sftp {
        insert_some(&mut options, "host", sftp.host.as_ref());
        insert_some(&mut options, "user", sftp.user.as_ref());
        insert_some_number(&mut options, "port", sftp.port.map(u64::from));
        insert_some(&mut options, "auth", sftp.auth.as_ref());
        // The normalized SFTP projection is authoritative, including an empty
        // list. Otherwise stale raw profile keys could reappear after the Lua
        // snapshot deliberately resolved them to none.
        options.insert(
            "privateKeys".to_owned(),
            Value::Array(
                sftp.private_keys
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        insert_some(&mut options, "password_env", sftp.password_env.as_ref());
        insert_some(&mut options, "identityAgent", sftp.identity_agent.as_ref());
        insert_some(&mut options, "jumpHost", sftp.jump_host.as_ref());
        insert_some(&mut options, "proxyCommand", sftp.proxy_command.as_ref());
        insert_some_number(&mut options, "readyTimeout", sftp.ready_timeout);
        insert_some_number(&mut options, "keepaliveInterval", sftp.keepalive_interval);
        insert_some_number(
            &mut options,
            "keepaliveCountMax",
            sftp.keepalive_count_max.map(u64::from),
        );
        insert_some(
            &mut options,
            "host_key_policy",
            sftp.host_key_policy.as_ref(),
        );
        if let Some(ssh_options) = &sftp.ssh_options {
            options.insert("ssh_options".to_owned(), Value::Object(ssh_options.clone()));
        }
        options.extend(sftp.extra.clone());
    }
    options
}

fn parse_private_keys(value: Option<&Value>) -> Vec<PrivateKeySource> {
    let values = match value {
        Some(Value::Array(values)) => values.iter().collect::<Vec<_>>(),
        Some(value) => vec![value],
        None => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|value| match value {
            Value::String(path) if !path.is_empty() => Some(PrivateKeySource {
                path: Some(PathBuf::from(path)),
                ..PrivateKeySource::default()
            }),
            Value::Object(source) => {
                let path = text(source.get("path")).map(PathBuf::from);
                let data = text(source.get("data")).map(String::into_bytes);
                if path.is_none() && data.is_none() {
                    None
                } else {
                    Some(PrivateKeySource {
                        path,
                        data,
                        passphrase: text(source.get("passphrase")),
                    })
                }
            }
            _ => None,
        })
        .collect()
}

fn duration_option(value: Option<&Value>) -> Option<Duration> {
    let value = positive_number(value)?;
    Some(if value > 300 {
        Duration::from_millis(value)
    } else {
        Duration::from_secs(value)
    })
}

fn positive_number(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64().filter(|number| *number > 0).or_else(|| {
            number
                .as_f64()
                .filter(|number| number.is_finite() && *number > 0.0)
                .map(|number| number as u64)
        }),
        Some(Value::String(value)) => value.parse::<u64>().ok().filter(|number| *number > 0),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn insert_some(options: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        options.insert(key.to_owned(), Value::String(value.clone()));
    }
}

fn insert_some_number(options: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        options.insert(key.to_owned(), Value::Number(value.into()));
    }
}

fn issue(
    field: impl Into<String>,
    severity: CompatibilityIssueSeverity,
    message: impl Into<String>,
) -> CompatibilityIssue {
    CompatibilityIssue {
        field: field.into(),
        severity,
        message: message.into(),
    }
}

struct ParsedTarget {
    host: String,
    port: Option<u32>,
    username: Option<String>,
}

fn parse_target(value: &str) -> Option<ParsedTarget> {
    let mut rest = value.trim();
    if rest.is_empty() {
        return None;
    }
    let mut username = None;
    if let Some(at) = rest.rfind('@') {
        username = (!rest[..at].is_empty()).then(|| rest[..at].to_owned());
        rest = &rest[at + 1..];
    }
    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let close = bracketed.find(']')?;
        let host = bracketed[..close].to_owned();
        let suffix = &bracketed[close + 1..];
        let port = suffix
            .strip_prefix(':')
            .and_then(|port| port.parse::<u32>().ok())
            .filter(|port| *port > 0);
        (host, port)
    } else if let Some(colon) = rest.rfind(':') {
        if colon > 0 && rest.find(':') == Some(colon) {
            if let Some(port) = rest[colon + 1..]
                .parse::<u32>()
                .ok()
                .filter(|port| *port > 0)
            {
                (rest[..colon].to_owned(), Some(port))
            } else {
                (rest.to_owned(), None)
            }
        } else {
            (rest.to_owned(), None)
        }
    } else {
        (rest.to_owned(), None)
    };
    (!host.is_empty()).then_some(ParsedTarget {
        host,
        port,
        username,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SftpProfileData;

    fn profile(id: &str) -> Profile {
        Profile {
            id: id.to_owned(),
            name: id.to_owned(),
            group: String::new(),
            editable: true,
            source: "store".to_owned(),
            host: format!("{id}.summary.invalid"),
            user: String::new(),
            port: 22,
            auth: String::new(),
            has_password: false,
            jump_host: String::new(),
            icon: String::new(),
            color: String::new(),
            raw: None,
            sftp: None,
        }
    }

    #[test]
    fn normalized_projection_maps_target_and_jump_credentials() {
        let mut target = profile("target");
        target.sftp = Some(SftpProfileData {
            host: Some("files.internal".to_owned()),
            user: Some("deploy".to_owned()),
            port: Some(2222),
            auth: Some("password".to_owned()),
            jump_host: Some("jump".to_owned()),
            ready_timeout: Some(750),
            keepalive_interval: Some(15),
            keepalive_count_max: Some(4),
            ..SftpProfileData::default()
        });
        let mut jump = profile("jump");
        jump.sftp = Some(SftpProfileData {
            host: Some("bastion.internal".to_owned()),
            user: Some("ops".to_owned()),
            auth: Some("agent".to_owned()),
            identity_agent: Some("/tmp/test-agent.sock".to_owned()),
            ..SftpProfileData::default()
        });
        let overrides = ProfileConnectionOverrides {
            credentials: CredentialOverrides {
                password: Some("secret".to_owned()),
                ..CredentialOverrides::default()
            },
            ..ProfileConnectionOverrides::default()
        };

        let mapped = connection_from_profile(&target, &[target.clone(), jump], &overrides);

        assert!(mapped.supported);
        assert!(mapped.issues.is_empty());
        assert_eq!(mapped.connection.host, "files.internal");
        assert_eq!(mapped.connection.username.as_deref(), Some("deploy"));
        assert_eq!(mapped.connection.port, 2222);
        assert_eq!(
            mapped.connection.ready_timeout,
            Some(Duration::from_millis(750))
        );
        assert_eq!(
            mapped.connection.keepalive_interval,
            Some(Duration::from_secs(15))
        );
        assert_eq!(mapped.connection.keepalive_count_max, Some(4));
        assert_eq!(
            mapped.connection.authentication.password.as_deref(),
            Some("secret")
        );
        let jump = mapped.connection.jump.expect("mapped jump host");
        assert_eq!(jump.host, "bastion.internal");
        assert_eq!(jump.username.as_deref(), Some("ops"));
        assert_eq!(
            jump.authentication.agent,
            Some(AgentEndpoint::Path("/tmp/test-agent.sock".to_owned()))
        );
    }

    #[test]
    fn password_environment_and_compatibility_issues_are_reported() {
        let mut target = profile("target");
        target.sftp = Some(SftpProfileData {
            host: Some("target.test".to_owned()),
            auth: Some("password".to_owned()),
            private_keys: vec!["~/.ssh/id_one".to_owned(), "~/.ssh/id_two".to_owned()],
            password_env: Some("TEST_SFTP_PASSWORD".to_owned()),
            proxy_command: Some("ssh proxy nc %h %p".to_owned()),
            ..SftpProfileData::default()
        });
        let overrides = ProfileConnectionOverrides {
            environment: Some(HashMap::from([(
                "TEST_SFTP_PASSWORD".to_owned(),
                "from-env".to_owned(),
            )])),
            ..ProfileConnectionOverrides::default()
        };

        let mapped = connection_from_profile(&target, &[target.clone()], &overrides);

        assert_eq!(
            mapped.connection.authentication.password.as_deref(),
            Some("from-env")
        );
        assert_eq!(mapped.connection.authentication.private_keys.len(), 2);
        assert!(!mapped.supported);
        assert!(mapped.issues.iter().any(|issue| {
            issue.field == "proxyCommand"
                && issue.severity == CompatibilityIssueSeverity::Unsupported
        }));
        assert!(!mapped.issues.iter().any(|issue| issue.field == "password"));
    }

    #[test]
    fn empty_password_and_agent_do_not_suppress_password_prompt() {
        let mut target = profile("target");
        target.sftp = Some(SftpProfileData {
            host: Some("target.test".to_owned()),
            auth: Some("password".to_owned()),
            password_env: Some("EMPTY_PASSWORD".to_owned()),
            identity_agent: Some("/tmp/global-agent.sock".to_owned()),
            ..SftpProfileData::default()
        });
        let overrides = ProfileConnectionOverrides {
            environment: Some(HashMap::from([(
                "EMPTY_PASSWORD".to_owned(),
                String::new(),
            )])),
            ..ProfileConnectionOverrides::default()
        };

        let mapped = connection_from_profile(&target, &[target.clone()], &overrides);

        assert_eq!(mapped.connection.authentication.password, None);
        assert_eq!(mapped.connection.authentication.agent, None);
        assert!(mapped.issues.iter().any(|issue| {
            issue.field == "password" && issue.severity == CompatibilityIssueSeverity::NeedsInput
        }));
    }

    #[test]
    fn parses_a_single_inline_ipv6_jump_host() {
        let mut target = profile("target");
        target.sftp = Some(SftpProfileData {
            host: Some("target.test".to_owned()),
            auth: Some("password".to_owned()),
            jump_host: Some("ops@[2001:db8::10]:2200".to_owned()),
            ..SftpProfileData::default()
        });
        let overrides = ProfileConnectionOverrides {
            credentials: CredentialOverrides {
                password: Some("target-secret".to_owned()),
                ..CredentialOverrides::default()
            },
            jump: CredentialOverrides {
                password: Some("jump-secret".to_owned()),
                ..CredentialOverrides::default()
            },
            ..ProfileConnectionOverrides::default()
        };

        let mapped = connection_from_profile(&target, &[target.clone()], &overrides);
        let jump = mapped.connection.jump.expect("inline jump host");

        assert_eq!(jump.host, "2001:db8::10");
        assert_eq!(jump.port, 2200);
        assert_eq!(jump.username.as_deref(), Some("ops"));
        assert_eq!(jump.authentication.password.as_deref(), Some("jump-secret"));
    }
}
