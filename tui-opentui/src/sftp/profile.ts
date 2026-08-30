import type { Profile } from "../types.ts"
import type {
  CompatibilityIssue,
  CredentialOverrides,
  PrivateKeySource,
  ProfileConnectionOverrides,
  ProfileConnectionResult,
  ProfileLookup,
  SftpAuthentication,
  SftpConnectionOptions,
} from "./types.ts"

type UnknownObject = Record<string, unknown>

function object(value: unknown): UnknownObject {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as UnknownObject)
    : {}
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined
}

function number(value: unknown): number | undefined {
  const parsed = typeof value === "number" ? value : Number(value)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined
}

function boolean(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined
}

function optionsOf(profile: Profile): UnknownObject {
  const raw = object(profile.raw)
  const nested = object(raw.options)
  const fallback = Object.keys(nested).length > 0 ? nested : raw
  return profile.sftp ? { ...fallback, ...profile.sftp } : fallback
}

function privateKeys(value: unknown): PrivateKeySource[] {
  const items = Array.isArray(value) ? value : typeof value === "string" ? [value] : []
  return items.flatMap((item) => {
    if (typeof item === "string" && item !== "") return [{ path: item }]
    const candidate = object(item)
    const path = text(candidate.path)
    const data = text(candidate.data)
    const passphrase = text(candidate.passphrase)
    if (!path && !data) return []
    return [{ ...(path ? { path } : {}), ...(data ? { data } : {}), ...(passphrase ? { passphrase } : {}) }]
  })
}

function authFor(
  profile: Profile,
  overrides: CredentialOverrides,
  environment: Readonly<Record<string, string | undefined>>,
  issues: CompatibilityIssue[],
  prefix: "" | "jump.",
): SftpAuthentication {
  const raw = optionsOf(profile)
  const mode = text(raw.auth) || profile.auth || ""
  const keys = overrides.privateKeys ?? privateKeys(raw.privateKeys ?? raw.private_keys)
  const passwordEnvironment = text(raw.password_env)
  const passwordFromEnvironment = passwordEnvironment ? environment[passwordEnvironment] : undefined
  const password = overrides.password ?? passwordFromEnvironment
  const configuredAgent = text(raw.identityAgent) ?? boolean(raw.agent)
  const requestedAgent = overrides.agent ?? (
    mode === "agent"
      ? configuredAgent ?? true
      : mode === "" || mode === "publicKey"
        ? configuredAgent
        : undefined
  )

  if (raw.password_cmd !== undefined && password === undefined) {
    issues.push({
      field: `${prefix}password_cmd`,
      severity: "needs-input",
      message: "password_cmd is not executed by the TUI; prompt for a password or resolve it before connecting",
    })
  }
  if ((mode === "password" || mode === "keyboardInteractive") && !password) {
    issues.push({
      field: `${prefix}password`,
      severity: "needs-input",
      message: `${prefix ? "Jump host" : "Target"} password must be supplied by the UI`,
    })
  }
  if (mode === "publicKey" && keys.length === 0 && requestedAgent === undefined) {
    issues.push({
      field: `${prefix}privateKeys`,
      severity: "needs-input",
      message: `${prefix ? "Jump host" : "Target"} private key is not present in the snapshot`,
    })
  }
  if (mode === "agent" && requestedAgent === undefined) {
    issues.push({
      field: `${prefix}agent`,
      severity: "needs-input",
      message: `${prefix ? "Jump host" : "Target"} SSH agent must be enabled`,
    })
  }
  if (mode === "keyboardInteractive") {
    issues.push({
      field: `${prefix}auth`,
      severity: "unsupported",
      message: "Interactive challenge/response authentication is not supported by the SFTP client yet",
    })
  } else if (mode && !["password", "publicKey", "agent"].includes(mode)) {
    issues.push({
      field: `${prefix}auth`,
      severity: "unsupported",
      message: `Authentication mode ${mode} is not supported by the SFTP client`,
    })
  }

  return {
    ...(password ? { password } : {}),
    ...(keys.length > 0 ? { privateKeys: keys } : {}),
    ...(requestedAgent === undefined ? {} : { agent: requestedAgent }),
  }
}

function lookupProfile(lookup: ProfileLookup, key: string): Profile | undefined {
  if (typeof lookup === "function") return lookup(key)
  return lookup.find((profile) => profile.id === key || profile.name === key)
}

function parseTarget(value: string): { host: string; port?: number; username?: string } | null {
  let rest = value.trim()
  if (!rest) return null
  let username: string | undefined
  const at = rest.lastIndexOf("@")
  if (at >= 0) {
    username = rest.slice(0, at) || undefined
    rest = rest.slice(at + 1)
  }
  let host = rest
  let port: number | undefined
  const bracketed = rest.match(/^\[([^\]]+)](?::(\d+))?$/)
  if (bracketed?.[1]) {
    host = bracketed[1]
    port = number(bracketed[2])
  } else {
    const colon = rest.lastIndexOf(":")
    if (colon > 0 && rest.indexOf(":") === colon) {
      const parsedPort = number(rest.slice(colon + 1))
      if (parsedPort) {
        host = rest.slice(0, colon)
        port = parsedPort
      }
    }
  }
  if (!host) return null
  return { host, ...(port ? { port } : {}), ...(username ? { username } : {}) }
}

function addUnsupportedOptions(profile: Profile, issues: CompatibilityIssue[], prefix = ""): void {
  const raw = optionsOf(profile)
  for (const key of ["proxyCommand", "proxy_command", "socksProxyHost", "httpProxyHost"] as const) {
    if (raw[key] !== undefined && raw[key] !== "") {
      issues.push({
        field: `${prefix}${key}`,
        severity: "unsupported",
        message: `${key} is not supported by the integrated SFTP client`,
      })
    }
  }
  if (raw.algorithms !== undefined) {
    issues.push({
      field: `${prefix}algorithms`,
      severity: "warning",
      message: "Custom OpenSSH algorithm lists are not mapped to ssh2 yet and will be ignored",
    })
  }
  if (raw.ssh_options !== undefined) {
    issues.push({
      field: `${prefix}ssh_options`,
      severity: "warning",
      message: "Arbitrary OpenSSH -o options are not interpreted by ssh2 and will be ignored",
    })
  }
  if (raw.host_key_policy !== undefined) {
    issues.push({
      field: `${prefix}host_key_policy`,
      severity: "warning",
      message: "OpenSSH host-key policy is not mapped automatically; provide a hostVerifier when required",
    })
  }
}

function baseConnection(
  profile: Profile,
  overrides: CredentialOverrides,
  environment: Readonly<Record<string, string | undefined>>,
  issues: CompatibilityIssue[],
  prefix: "" | "jump.",
): SftpConnectionOptions {
  const raw = optionsOf(profile)
  const host = text(raw.host) || profile.host || profile.name
  const username = text(raw.user) || profile.user
  const port = number(raw.port) || profile.port || 22
  const readyTimeout = number(raw.readyTimeout ?? raw.ready_timeout)
  const keepaliveInterval = number(raw.keepaliveInterval ?? raw.keepalive_interval)
  const keepaliveCountMax = number(raw.keepaliveCountMax ?? raw.keepalive_count_max)
  return {
    host,
    port,
    ...(username ? { username } : {}),
    authentication: authFor(profile, overrides, environment, issues, prefix),
    ...(readyTimeout ? { readyTimeoutMs: readyTimeout > 300 ? readyTimeout : readyTimeout * 1000 } : {}),
    ...(keepaliveInterval
      ? { keepaliveIntervalMs: keepaliveInterval > 300 ? keepaliveInterval : keepaliveInterval * 1000 }
      : {}),
    ...(keepaliveCountMax ? { keepaliveCountMax } : {}),
  }
}

/** Convert the manager's sanitized snapshot profile into explicit ssh2 settings. */
export function connectionFromProfile(
  profile: Profile,
  lookup: ProfileLookup,
  overrides: ProfileConnectionOverrides = {},
): ProfileConnectionResult {
  const issues: CompatibilityIssue[] = []
  const environment = overrides.environment ?? process.env
  const connection = baseConnection(profile, overrides, environment, issues, "")
  addUnsupportedOptions(profile, issues)

  const raw = optionsOf(profile)
  const jumpSpec = text(raw.jumpHost ?? raw.jump_host) || profile.jumpHost
  if (jumpSpec) {
    if (jumpSpec.includes(",")) {
      issues.push({
        field: "jumpHost",
        severity: "unsupported",
        message: "Only one jump host is currently supported; comma-separated chains require OpenSSH",
      })
    } else {
      const jumpProfile = lookupProfile(lookup, jumpSpec)
      if (jumpProfile) {
        const jumpRaw = optionsOf(jumpProfile)
        if (text(jumpRaw.jumpHost ?? jumpRaw.jump_host) || jumpProfile.jumpHost) {
          issues.push({
            field: "jumpHost",
            severity: "unsupported",
            message: "Nested jump profiles are not supported; select a profile with a single jump host",
          })
        }
        const jump = baseConnection(
          jumpProfile,
          overrides.jump ?? {},
          environment,
          issues,
          "jump.",
        )
        addUnsupportedOptions(jumpProfile, issues, "jump.")
        connection.jump = jump
      } else {
        const parsed = parseTarget(jumpSpec)
        if (!parsed) {
          issues.push({
            field: "jumpHost",
            severity: "unsupported",
            message: `Cannot parse jump host ${jumpSpec}`,
          })
        } else {
          const jumpAuth: SftpAuthentication = {
            ...(overrides.jump?.password ? { password: overrides.jump.password } : {}),
            ...(overrides.jump?.privateKeys ? { privateKeys: overrides.jump.privateKeys } : {}),
            ...(overrides.jump?.agent === undefined ? { agent: true } : { agent: overrides.jump.agent }),
          }
          connection.jump = {
            host: parsed.host,
            port: parsed.port ?? 22,
            ...(parsed.username ? { username: parsed.username } : {}),
            authentication: jumpAuth,
          }
        }
      }
    }
  }

  return {
    connection,
    issues,
    supported: !issues.some((issue) => issue.severity === "unsupported"),
  }
}
