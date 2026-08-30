import type { JsonObject, Profile, ProfileDraft, SftpProfileData, Snapshot } from "./types.ts"

export const ALL_GROUPS = "__all__"

const objectOrEmpty = (value: unknown): JsonObject =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? (value as JsonObject) : {}

const stringValue = (value: unknown, fallback = ""): string =>
  typeof value === "string" ? value : value === null || value === undefined ? fallback : String(value)

const numberValue = (value: unknown, fallback: number): number => {
  const parsed = typeof value === "number" ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

export function normalizeSnapshot(value: unknown): Snapshot {
  const input = objectOrEmpty(value)
  const rawProfiles = Array.isArray(input.profiles)
    ? input.profiles
    : Object.values(objectOrEmpty(input.profiles))
  const rawGroups = Array.isArray(input.groups) ? input.groups : Object.values(objectOrEmpty(input.groups))

  const profiles = rawProfiles
    .filter((item): item is JsonObject => item !== null && typeof item === "object" && !Array.isArray(item))
    .map((item): Profile => {
      const id = stringValue(item.id)
      const raw = item.raw === undefined ? undefined : structuredClone(objectOrEmpty(item.raw))
      const sftp = item.sftp === undefined
        ? undefined
        : (structuredClone(objectOrEmpty(item.sftp)) as SftpProfileData)
      return {
        id,
        name: stringValue(item.name, id || "?"),
        group: stringValue(item.group),
        editable: item.editable !== false,
        source: stringValue(item.source, "store"),
        host: stringValue(item.host),
        user: stringValue(item.user),
        port: numberValue(item.port, 22),
        auth: stringValue(item.auth),
        hasPassword: item.has_password === true || item.hasPassword === true,
        jumpHost: stringValue(item.jumpHost),
        icon: stringValue(item.icon),
        color: stringValue(item.color),
        ...(raw === undefined ? {} : { raw }),
        ...(sftp === undefined ? {} : { sftp }),
      }
    })

  const groups = rawGroups.map((group) => stringValue(group)).filter(Boolean)
  for (const profile of profiles) {
    if (profile.group && !groups.includes(profile.group)) groups.push(profile.group)
  }

  return {
    storePath: stringValue(input.store_path ?? input.storePath),
    defaultWhere: stringValue(input.default_where ?? input.defaultWhere, "tab"),
    groups,
    profiles,
  }
}

export function profileTarget(profile: Profile): string {
  const host = profile.host || "?"
  const prefix = profile.user ? `${profile.user}@` : ""
  const suffix = profile.port && profile.port !== 22 ? `:${profile.port}` : ""
  return `${prefix}${host}${suffix}`
}

export function profileMatches(profile: Profile, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return true
  return [profile.id, profile.name, profile.group, profile.host, profile.user, profile.jumpHost]
    .join(" ")
    .toLocaleLowerCase()
    .includes(needle)
}

export function visibleProfiles(snapshot: Snapshot, group: string, query: string): Profile[] {
  return snapshot.profiles.filter(
    (profile) => (group === ALL_GROUPS || profile.group === group) && profileMatches(profile, query),
  )
}

export interface GroupSummary {
  id: string
  label: string
  count: number
}

export function groupSummaries(snapshot: Snapshot): GroupSummary[] {
  const counts = new Map<string, number>()
  for (const profile of snapshot.profiles) {
    counts.set(profile.group, (counts.get(profile.group) ?? 0) + 1)
  }
  const groups = snapshot.groups.filter(Boolean)
  for (const group of counts.keys()) {
    if (group && !groups.includes(group)) groups.push(group)
  }
  return [
    { id: ALL_GROUPS, label: "全部", count: snapshot.profiles.length },
    ...groups.map((group) => ({ id: group, label: group, count: counts.get(group) ?? 0 })),
  ]
}

export function parseTarget(spec: string): { host: string; user?: string; port?: number } {
  const trimmed = spec.trim()
  let rest = trimmed
  let user: string | undefined
  const at = rest.indexOf("@")
  if (at >= 0) {
    user = rest.slice(0, at) || undefined
    rest = rest.slice(at + 1)
  }

  if (rest.startsWith("[") && rest.includes("]")) {
    const close = rest.indexOf("]")
    const host = rest.slice(1, close)
    const suffix = rest.slice(close + 1)
    const maybePort = suffix.startsWith(":") ? Number(suffix.slice(1)) : undefined
    const validPort = maybePort !== undefined && Number.isInteger(maybePort) && maybePort > 0 ? maybePort : undefined
    return {
      host,
      ...(user ? { user } : {}),
      ...(validPort === undefined ? {} : { port: validPort }),
    }
  }

  const colon = rest.lastIndexOf(":")
  if (colon > 0 && rest.indexOf(":") === colon) {
    const maybePort = Number(rest.slice(colon + 1))
    if (Number.isInteger(maybePort) && maybePort > 0) {
      return { host: rest.slice(0, colon), ...(user ? { user } : {}), port: maybePort }
    }
  }
  return { host: rest, ...(user ? { user } : {}) }
}

const getPath = (object: JsonObject, path: string): unknown => {
  let current: unknown = object
  for (const key of path.split(".")) {
    if (current === null || typeof current !== "object" || Array.isArray(current)) return undefined
    current = (current as JsonObject)[key]
  }
  return current
}

const setPath = (object: JsonObject, path: string, value: unknown): void => {
  const parts = path.split(".")
  let current = object
  for (const key of parts.slice(0, -1)) {
    const existing = current[key]
    if (existing === null || typeof existing !== "object" || Array.isArray(existing)) current[key] = {}
    current = current[key] as JsonObject
  }
  const last = parts.at(-1)
  if (!last) return
  if (value === undefined || value === null || value === "") delete current[last]
  else current[last] = value
}

export function draftFromProfile(profile?: Profile, initialGroup = ""): ProfileDraft {
  const raw = structuredClone(profile?.raw ?? {})
  const options = objectOrEmpty(raw.options)
  const field = (flat: string, nested = flat): unknown => options[nested] ?? raw[flat]
  return {
    originalId: profile?.id || null,
    name: stringValue(raw.name, profile?.name ?? ""),
    group: stringValue(raw.group, profile?.group ?? initialGroup),
    host: stringValue(field("host"), profile?.host ?? ""),
    port: String(field("port") ?? profile?.port ?? 22),
    user: stringValue(field("user"), profile?.user ?? ""),
    auth: stringValue(field("auth"), profile?.auth ?? ""),
    password: "",
    jumpHost: stringValue(field("jumpHost"), profile?.jumpHost ?? ""),
    raw,
  }
}

export function draftFromTarget(target: string, initialGroup = ""): ProfileDraft {
  const parsed = parseTarget(target)
  return {
    originalId: null,
    name: parsed.host,
    group: initialGroup,
    host: parsed.host,
    port: String(parsed.port ?? 22),
    user: parsed.user ?? "",
    auth: "",
    password: "",
    jumpHost: "",
    raw: {},
  }
}

export function rawFromDraft(draft: ProfileDraft): { raw?: JsonObject; error?: string } {
  const host = draft.host.trim()
  if (!host) return { error: "主机不能为空" }
  const portText = draft.port.trim()
  const port = portText ? Number(portText) : 22
  if (!Number.isInteger(port) || port < 1 || port > 65535) return { error: "端口需要是 1–65535 的整数" }

  const raw = structuredClone(draft.raw)
  const nested = raw.options !== undefined
  const path = (name: string): string => (nested ? `options.${name}` : name)
  setPath(raw, "name", draft.name.trim() || host)
  setPath(raw, "group", draft.group.trim())
  setPath(raw, path("host"), host)
  setPath(raw, path("port"), port === 22 ? undefined : port)
  setPath(raw, path("user"), draft.user.trim())
  setPath(raw, path("auth"), draft.auth.trim())
  if (draft.password !== "") setPath(raw, path("password"), draft.password)
  setPath(raw, path("jumpHost"), draft.jumpHost.trim())
  return { raw }
}

export function cloneDraft(draft: ProfileDraft): ProfileDraft {
  return { ...draft, raw: structuredClone(draft.raw) }
}

export function draftFieldValue(raw: JsonObject, path: string): unknown {
  return getPath(raw, path)
}
