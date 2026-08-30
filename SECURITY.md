# Security policy

## Supported versions

Security fixes are made on the latest tagged release and the default branch.

## Reporting a vulnerability

Please use GitHub's **Security → Report a vulnerability** form so credentials,
host names, profile files, terminal logs, and reproduction details are not
posted to a public issue. If private vulnerability reporting is unavailable,
open a public issue containing no sensitive data and ask the maintainer for a
private contact channel.

Before attaching logs or profiles, remove passwords, tokens, private keys,
public IP addresses, internal host names, user names, and OSC payloads.

## Current security boundaries

The integrated SFTP client in v1.0.0 is intended for a trusted personal
environment. It does not yet load `known_hosts`, and OpenTUI 0.5.9 does not
mask password input. These limitations are also documented in the README.
