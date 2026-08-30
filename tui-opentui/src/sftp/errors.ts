export class SftpManagerError extends Error {
  readonly code: string
  readonly path: string | null

  constructor(code: string, message: string, options: { path?: string; cause?: unknown } = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause })
    this.name = "SftpManagerError"
    this.code = code
    this.path = options.path ?? null
  }
}

export class SftpAbortError extends SftpManagerError {
  constructor(message = "SFTP operation was cancelled", cause?: unknown) {
    super("ABORTED", message, cause === undefined ? {} : { cause })
    this.name = "SftpAbortError"
  }
}

export class SftpCredentialRequiredError extends SftpManagerError {
  readonly role: "target" | "jump"
  readonly method: "password" | "privateKey" | "agent" | "authentication"

  constructor(
    role: "target" | "jump",
    method: "password" | "privateKey" | "agent" | "authentication",
  ) {
    const label = role === "jump" ? "jump host" : "target host"
    super("CREDENTIAL_REQUIRED", `${label} requires ${method} credentials`)
    this.name = "SftpCredentialRequiredError"
    this.role = role
    this.method = method
  }
}

export class SftpUnsupportedProfileError extends SftpManagerError {
  readonly fields: readonly string[]

  constructor(fields: readonly string[]) {
    super("UNSUPPORTED_PROFILE", `Unsupported SFTP profile options: ${fields.join(", ")}`)
    this.name = "SftpUnsupportedProfileError"
    this.fields = fields
  }
}

export function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw new SftpAbortError("SFTP operation was cancelled", signal.reason)
}

export function asOperationError(error: unknown, operation: string, path?: string): SftpManagerError {
  if (error instanceof SftpManagerError) return error
  const detail = error instanceof Error ? error.message : String(error)
  return new SftpManagerError(
    "OPERATION_FAILED",
    `${operation}${path ? ` ${path}` : ""} failed: ${detail}`,
    { ...(path === undefined ? {} : { path }), cause: error },
  )
}

export function isNotFoundError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false
  const code = (error as { code?: unknown }).code
  return code === "ENOENT" || code === 2
}
