import type { Profile } from "../types.ts"

export type FileKind = "directory" | "file" | "symlink" | "other"

export interface FileEntry {
  name: string
  path: string
  kind: FileKind
  size: number
  modifiedAt: Date | null
  mode: number | null
}

export interface OperationOptions {
  signal?: AbortSignal
}

export interface MkdirOptions extends OperationOptions {
  recursive?: boolean
  mode?: number
}

export interface RemoveOptions extends OperationOptions {
  recursive?: boolean
}

export interface FileProvider {
  readonly kind: "local" | "remote"
  list(directory: string, options?: OperationOptions): Promise<FileEntry[]>
  stat(path: string, options?: OperationOptions): Promise<FileEntry>
  mkdir(path: string, options?: MkdirOptions): Promise<void>
  rename(from: string, to: string, options?: OperationOptions): Promise<void>
  remove(path: string, options?: RemoveOptions): Promise<void>
}

export type TransferDirection = "upload" | "download"
export type TransferPhase = "starting" | "transferring" | "finishing" | "completed"

export interface TransferProgress {
  direction: TransferDirection
  phase: TransferPhase
  source: string
  destination: string
  transferredBytes: number
  totalBytes: number | null
  percent: number | null
  elapsedMs: number
  bytesPerSecond: number
}

export interface TransferOptions extends OperationOptions {
  /** Refuse to replace an existing destination unless explicitly enabled. */
  overwrite?: boolean
  /** Create the destination's parent directories. Defaults to true. */
  createParents?: boolean
  /** Write to a sibling temporary file and rename it after completion. */
  atomic?: boolean
  /** Try to retain the source modification time. */
  preserveTimes?: boolean
  onProgress?: (progress: TransferProgress) => void
}

export interface PrivateKeySource {
  path?: string
  data?: string | Uint8Array
  passphrase?: string
}

export interface SftpAuthentication {
  password?: string
  privateKeys?: readonly PrivateKeySource[]
  /** true means SSH_AUTH_SOCK (or Windows OpenSSH's conventional named pipe). */
  agent?: boolean | string
}

export interface SftpConnectionOptions {
  host: string
  port?: number
  username?: string
  authentication?: SftpAuthentication
  readyTimeoutMs?: number
  keepaliveIntervalMs?: number
  keepaliveCountMax?: number
  jump?: SftpConnectionOptions
  /** ssh2-compatible host key verifier. Known-hosts loading remains the caller's job. */
  hostVerifier?: (key: Buffer) => boolean
}

export interface CredentialOverrides {
  password?: string
  privateKeys?: readonly PrivateKeySource[]
  agent?: boolean | string
}

export interface ProfileConnectionOverrides extends CredentialOverrides {
  jump?: CredentialOverrides
  environment?: Readonly<Record<string, string | undefined>>
}

export type CompatibilityIssueSeverity = "needs-input" | "warning" | "unsupported"

export interface CompatibilityIssue {
  field: string
  severity: CompatibilityIssueSeverity
  message: string
}

export interface ProfileConnectionResult {
  connection: SftpConnectionOptions
  issues: CompatibilityIssue[]
  supported: boolean
}

export type ProfileLookup =
  | readonly Profile[]
  | ((idOrName: string) => Profile | undefined)
