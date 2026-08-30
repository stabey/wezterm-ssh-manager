export type JsonObject = Record<string, unknown>

/** Password-free connection details emitted for every snapshot profile. */
export interface SftpProfileData {
  host?: string
  user?: string
  port?: number
  auth?: string
  privateKeys?: string[]
  password_env?: string
  identityAgent?: string
  jumpHost?: string
  proxyCommand?: string
  readyTimeout?: number
  keepaliveInterval?: number
  keepaliveCountMax?: number
  host_key_policy?: string
  ssh_options?: JsonObject
}

export interface Profile {
  id: string
  name: string
  group: string
  editable: boolean
  source: "store" | "import" | string
  host: string
  user: string
  port: number
  auth: string
  hasPassword: boolean
  jumpHost: string
  icon: string
  color: string
  raw?: JsonObject
  sftp?: SftpProfileData
}

export interface Snapshot {
  storePath: string
  defaultWhere: "tab" | "window" | string
  groups: string[]
  profiles: Profile[]
}

export type ManagerRequest =
  | { op: "connect"; id: string; where: string }
  | { op: "quick"; target: string; where: string }
  | { op: "hide" }
  | { op: "upsert"; id: string | null; raw: JsonObject }
  | { op: "delete"; id: string }
  | { op: "copy_in"; id: string }
  | { op: "reload" }

export interface RequestEnvelope {
  v: 2
  token: string
  seq: number
  request: string
}

export type MainPage = "manager" | "sftp"
export type ManagerFocus = "groups" | "hosts" | "details"

export interface ProfileDraft {
  originalId: string | null
  name: string
  group: string
  host: string
  port: string
  user: string
  auth: string
  password: string
  jumpHost: string
  raw: JsonObject
}
