import type { ClientChannel, SFTPWrapper } from "ssh2"

import type { SshClientAdapter } from "./connection.ts"
import { LocalFileProvider } from "./local-provider.ts"
import { RemoteFileProvider } from "./remote-provider.ts"
import { downloadFile, uploadFile } from "./transfer.ts"
import type { OperationOptions, TransferOptions } from "./types.ts"

export class SftpSession {
  readonly local = new LocalFileProvider()
  readonly remote: RemoteFileProvider
  #closed = false
  #transportClosed = false
  #disconnectReason: Error | undefined
  #disconnectListeners = new Set<(error?: Error) => void>()
  #onTransportError = (error: Error) => this.#disconnect(error)
  #onTransportEnd = () => this.#disconnect()

  constructor(
    readonly sftp: SFTPWrapper,
    private readonly clients: readonly SshClientAdapter[],
    private readonly jumpChannel?: ClientChannel,
  ) {
    this.remote = new RemoteFileProvider(sftp)
    for (const client of clients) {
      client.on("error", this.#onTransportError)
      client.on("close", this.#onTransportEnd)
      client.on("end", this.#onTransportEnd)
    }
  }

  get closed(): boolean {
    return this.#closed
  }

  upload(localPath: string, remotePath: string, options?: TransferOptions): Promise<void> {
    return uploadFile(this.sftp, localPath, remotePath, options)
  }

  download(remotePath: string, localPath: string, options?: TransferOptions): Promise<void> {
    return downloadFile(this.sftp, remotePath, localPath, options)
  }

  remoteHome(options?: OperationOptions): Promise<string> {
    return this.remote.realpath(".", options)
  }

  onDisconnected(listener: (error?: Error) => void): () => void {
    if (this.#transportClosed) queueMicrotask(() => listener(this.#disconnectReason))
    else this.#disconnectListeners.add(listener)
    return () => this.#disconnectListeners.delete(listener)
  }

  close(): void {
    if (this.#closed) return
    this.#closed = true
    this.#detachTransportListeners()
    this.#closeResources()
  }

  [Symbol.dispose](): void {
    this.close()
  }

  #disconnect(error?: Error): void {
    if (this.#closed) return
    this.#closed = true
    this.#transportClosed = true
    this.#disconnectReason = error
    this.#detachTransportListeners()
    this.#closeResources()
    for (const listener of this.#disconnectListeners) listener(error)
    this.#disconnectListeners.clear()
  }

  #detachTransportListeners(): void {
    for (const client of this.clients) {
      client.off("error", this.#onTransportError)
      client.off("close", this.#onTransportEnd)
      client.off("end", this.#onTransportEnd)
    }
  }

  #closeResources(): void {
    try { this.sftp.end() } catch {}
    try { this.jumpChannel?.destroy() } catch {}
    for (const client of this.clients) {
      try { client.end() } catch {}
    }
  }
}
