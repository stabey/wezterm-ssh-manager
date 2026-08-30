import { readFile } from "node:fs/promises"
import { homedir, userInfo } from "node:os"
import { isAbsolute, join } from "node:path"

import { Client } from "ssh2"
import type { AnyAuthMethod, ClientChannel, ConnectConfig, SFTPWrapper } from "ssh2"

import {
  SftpAbortError,
  SftpCredentialRequiredError,
  SftpManagerError,
  throwIfAborted,
} from "./errors.ts"
import { SftpSession } from "./session.ts"
import type { PrivateKeySource, SftpAuthentication, SftpConnectionOptions } from "./types.ts"

export interface SshClientAdapter {
  connect(config: ConnectConfig): void
  end(): void
  once(event: "ready", listener: () => void): this
  once(event: "error", listener: (error: Error) => void): this
  once(event: "close" | "end", listener: () => void): this
  on(event: "error", listener: (error: Error) => void): this
  on(event: "close" | "end", listener: () => void): this
  off(event: "ready", listener: () => void): this
  off(event: "error", listener: (error: Error) => void): this
  off(event: "close" | "end", listener: () => void): this
  sftp(callback: (error: Error | undefined, sftp: SFTPWrapper) => void): void
  forwardOut(
    sourceIP: string,
    sourcePort: number,
    destinationIP: string,
    destinationPort: number,
    callback: (error: Error | undefined, channel: ClientChannel) => void,
  ): void
}

export interface ConnectionDependencies {
  createClient?: () => SshClientAdapter
  readPrivateKey?: (path: string) => Promise<Buffer>
  platform?: NodeJS.Platform
  environment?: Readonly<Record<string, string | undefined>>
}

function expandHome(path: string): string {
  if (path === "~") return homedir()
  if (path.startsWith("~/") || path.startsWith("~\\")) return join(homedir(), path.slice(2))
  return path
}

function agentSocket(
  requested: boolean | string | undefined,
  platform: NodeJS.Platform,
  environment: Readonly<Record<string, string | undefined>>,
): string | undefined {
  if (typeof requested === "string") return requested
  if (!requested) return undefined
  return environment.SSH_AUTH_SOCK ||
    (platform === "win32" ? "\\\\.\\pipe\\openssh-ssh-agent" : undefined)
}

async function loadPrivateKey(
  source: PrivateKeySource,
  reader: (path: string) => Promise<Buffer>,
): Promise<{ key: Buffer | string; passphrase?: string }> {
  if (source.data !== undefined) {
    const key = typeof source.data === "string" ? source.data : Buffer.from(source.data)
    return { key, ...(source.passphrase ? { passphrase: source.passphrase } : {}) }
  }
  if (!source.path) throw new SftpCredentialRequiredError("target", "privateKey")
  const expanded = expandHome(source.path)
  const key = await reader(isAbsolute(expanded) ? expanded : expanded)
  return { key, ...(source.passphrase ? { passphrase: source.passphrase } : {}) }
}

async function connectConfig(
  options: SftpConnectionOptions,
  role: "target" | "jump",
  dependencies: Required<Pick<ConnectionDependencies, "readPrivateKey" | "platform" | "environment">>,
): Promise<ConnectConfig> {
  const authentication = options.authentication ?? {}
  const loadedKeys = await Promise.all(
    (authentication.privateKeys ?? []).map((source) => loadPrivateKey(source, dependencies.readPrivateKey)),
  )
  const agent = agentSocket(authentication.agent, dependencies.platform, dependencies.environment)

  if (!authentication.password && loadedKeys.length === 0 && authentication.agent && !agent) {
    throw new SftpCredentialRequiredError(role, "agent")
  }
  if (!authentication.password && loadedKeys.length === 0 && !agent) {
    throw new SftpCredentialRequiredError(role, "authentication")
  }

  const username = options.username || userInfo().username
  const authHandler: AnyAuthMethod[] = [
    ...(authentication.password
      ? [{ type: "password" as const, username, password: authentication.password }]
      : []),
    ...loadedKeys.map((loaded) => ({
      type: "publickey" as const,
      username,
      key: loaded.key,
      ...(loaded.passphrase ? { passphrase: loaded.passphrase } : {}),
    })),
    ...(agent ? [{ type: "agent" as const, username, agent }] : []),
  ]

  return {
    host: options.host,
    port: options.port ?? 22,
    username,
    authHandler,
    ...(options.readyTimeoutMs === undefined ? {} : { readyTimeout: options.readyTimeoutMs }),
    ...(options.keepaliveIntervalMs === undefined
      ? {}
      : { keepaliveInterval: options.keepaliveIntervalMs }),
    ...(options.keepaliveCountMax === undefined
      ? {}
      : { keepaliveCountMax: options.keepaliveCountMax }),
    ...(options.hostVerifier ? { hostVerifier: options.hostVerifier } : {}),
  }
}

function waitForReady(
  client: SshClientAdapter,
  config: ConnectConfig,
  signal?: AbortSignal,
): Promise<void> {
  throwIfAborted(signal)
  return new Promise<void>((resolve, reject) => {
    const cleanup = () => {
      client.off("ready", onReady)
      client.off("error", onError)
      signal?.removeEventListener("abort", onAbort)
    }
    const onReady = () => {
      cleanup()
      // ssh2 may emit transport errors after the handshake. Keep an error
      // listener installed so a dropped connection never becomes an
      // uncaught EventEmitter error that terminates the TUI process.
      client.on("error", () => undefined)
      resolve()
    }
    const onError = (error: Error) => {
      cleanup()
      reject(error)
    }
    const onAbort = () => {
      cleanup()
      client.end()
      reject(new SftpAbortError("SFTP connection was cancelled", signal?.reason))
    }
    client.once("ready", onReady)
    client.once("error", onError)
    signal?.addEventListener("abort", onAbort, { once: true })
    try {
      client.connect(config)
    } catch (error) {
      cleanup()
      reject(error)
    }
  })
}

function openSftp(client: SshClientAdapter, signal?: AbortSignal): Promise<SFTPWrapper> {
  throwIfAborted(signal)
  return new Promise<SFTPWrapper>((resolve, reject) => {
    const onAbort = () => {
      client.end()
      reject(new SftpAbortError("Opening SFTP was cancelled", signal?.reason))
    }
    signal?.addEventListener("abort", onAbort, { once: true })
    client.sftp((error, sftp) => {
      signal?.removeEventListener("abort", onAbort)
      if (signal?.aborted) reject(new SftpAbortError("Opening SFTP was cancelled", signal.reason))
      else if (error) reject(error)
      else resolve(sftp)
    })
  })
}

function forwardToTarget(
  jump: SshClientAdapter,
  target: SftpConnectionOptions,
  signal?: AbortSignal,
): Promise<ClientChannel> {
  throwIfAborted(signal)
  return new Promise<ClientChannel>((resolve, reject) => {
    const onAbort = () => reject(new SftpAbortError("Jump host forwarding was cancelled", signal?.reason))
    signal?.addEventListener("abort", onAbort, { once: true })
    jump.forwardOut("127.0.0.1", 0, target.host, target.port ?? 22, (error, channel) => {
      signal?.removeEventListener("abort", onAbort)
      if (signal?.aborted) reject(new SftpAbortError("Jump host forwarding was cancelled", signal.reason))
      else if (error) reject(error)
      else resolve(channel)
    })
  })
}

/** Create a new SSH/SFTP connection. Existing WezTerm SSH panes are never reused. */
export async function connectSftp(
  options: SftpConnectionOptions,
  operation: { signal?: AbortSignal } = {},
  dependencies: ConnectionDependencies = {},
): Promise<SftpSession> {
  if (!options.host) throw new SftpManagerError("INVALID_CONNECTION", "SFTP host is required")
  if (options.jump?.jump) {
    throw new SftpManagerError("UNSUPPORTED_JUMP_CHAIN", "Only one SFTP jump host is supported")
  }
  const createClient = dependencies.createClient ?? (() => new Client() as SshClientAdapter)
  const resolvedDependencies = {
    readPrivateKey: dependencies.readPrivateKey ?? readFile,
    platform: dependencies.platform ?? process.platform,
    environment: dependencies.environment ?? process.env,
  }
  const clients: SshClientAdapter[] = []
  let jumpChannel: ClientChannel | undefined
  try {
    let socket: ClientChannel | undefined
    if (options.jump) {
      const jump = createClient()
      clients.push(jump)
      await waitForReady(
        jump,
        await connectConfig(options.jump, "jump", resolvedDependencies),
        operation.signal,
      )
      socket = await forwardToTarget(jump, options, operation.signal)
      jumpChannel = socket
    }

    const target = createClient()
    clients.unshift(target)
    const targetConfig = await connectConfig(options, "target", resolvedDependencies)
    if (socket) targetConfig.sock = socket
    await waitForReady(target, targetConfig, operation.signal)
    const sftp = await openSftp(target, operation.signal)
    return new SftpSession(sftp, clients, jumpChannel)
  } catch (error) {
    jumpChannel?.destroy()
    for (const client of clients) client.end()
    if (error instanceof SftpManagerError) throw error
    throw new SftpManagerError("CONNECTION_FAILED", `SFTP connection failed: ${String(error)}`, {
      cause: error,
    })
  }
}
