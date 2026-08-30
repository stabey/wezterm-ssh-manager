import { posix } from "node:path"
import { randomUUID } from "node:crypto"

import type { FileEntryWithStats as Ssh2FileEntry, SFTPWrapper, Stats } from "ssh2"

import { asOperationError, isNotFoundError, SftpAbortError, throwIfAborted } from "./errors.ts"
import type {
  FileEntry,
  FileKind,
  FileProvider,
  MkdirOptions,
  OperationOptions,
  RemoveOptions,
} from "./types.ts"

function call<T>(
  invoke: (callback: (error: Error | undefined, value: T) => void) => void,
  signal?: AbortSignal,
): Promise<T> {
  throwIfAborted(signal)
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => reject(new SftpAbortError("SFTP operation was cancelled", signal?.reason))
    signal?.addEventListener("abort", onAbort, { once: true })
    try {
      invoke((error, value) => {
        signal?.removeEventListener("abort", onAbort)
        if (signal?.aborted) {
          reject(new SftpAbortError("SFTP operation was cancelled", signal.reason))
        } else if (error) {
          reject(error)
        } else {
          resolve(value)
        }
      })
    } catch (error) {
      signal?.removeEventListener("abort", onAbort)
      reject(error)
    }
  })
}

function callVoid(
  invoke: (callback: (error?: Error | null) => void) => void,
  signal?: AbortSignal,
): Promise<void> {
  return call<void>((callback) => invoke((error) => callback(error ?? undefined, undefined)), signal)
}

function remoteKind(attributes: Stats): FileKind {
  if (attributes.isDirectory()) return "directory"
  if (attributes.isFile()) return "file"
  if (attributes.isSymbolicLink()) return "symlink"
  return "other"
}

function entryFromAttributes(path: string, name: string, attributes: Stats): FileEntry {
  return {
    name,
    path,
    kind: remoteKind(attributes),
    size: Number(attributes.size) || 0,
    modifiedAt: attributes.mtime ? new Date(attributes.mtime * 1000) : null,
    mode: typeof attributes.mode === "number" ? attributes.mode : null,
  }
}

function childPath(parent: string, child: string): string {
  return parent === "." || parent === "" ? child : posix.join(parent, child)
}

function sortEntries(entries: FileEntry[]): FileEntry[] {
  return entries.sort((left, right) => {
    if (left.kind === "directory" && right.kind !== "directory") return -1
    if (left.kind !== "directory" && right.kind === "directory") return 1
    return left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" })
  })
}

function causeOf(error: unknown): unknown {
  let current = error
  while (current instanceof Error && "cause" in current && current.cause !== undefined) {
    current = current.cause
  }
  return current
}

function unsupportedExtension(error: unknown): boolean {
  const cause = causeOf(error)
  const code = cause && typeof cause === "object" ? (cause as { code?: unknown }).code : undefined
  return code === 8 || (cause instanceof Error && cause.message.includes("does not support this extended request"))
}

export class RemoteFileProvider implements FileProvider {
  readonly kind = "remote" as const

  constructor(readonly sftp: SFTPWrapper) {}

  async list(directory: string, options: OperationOptions = {}): Promise<FileEntry[]> {
    try {
      const list = await call<Ssh2FileEntry[]>(
        (callback) => this.sftp.readdir(directory, callback),
        options.signal,
      )
      throwIfAborted(options.signal)
      return sortEntries(
        list
          .filter((entry) => entry.filename !== "." && entry.filename !== "..")
          .map((entry) =>
            entryFromAttributes(childPath(directory, entry.filename), entry.filename, entry.attrs),
          ),
      )
    } catch (error) {
      throw asOperationError(error, "list remote directory", directory)
    }
  }

  async stat(path: string, options: OperationOptions = {}): Promise<FileEntry> {
    try {
      const attributes = await call<Stats>(
        (callback) => this.sftp.lstat(path, callback),
        options.signal,
      )
      return entryFromAttributes(path, posix.basename(path), attributes)
    } catch (error) {
      throw asOperationError(error, "stat remote path", path)
    }
  }

  async realpath(path = ".", options: OperationOptions = {}): Promise<string> {
    try {
      return await call<string>((callback) => this.sftp.realpath(path, callback), options.signal)
    } catch (error) {
      throw asOperationError(error, "resolve remote path", path)
    }
  }

  async mkdir(path: string, options: MkdirOptions = {}): Promise<void> {
    try {
      if (!options.recursive) {
        await this.mkdirOne(path, options)
        return
      }

      const normalized = posix.normalize(path)
      const absolute = normalized.startsWith("/")
      const parts = normalized.split("/").filter(Boolean)
      let current = absolute ? "/" : ""
      for (const part of parts) {
        throwIfAborted(options.signal)
        current = current === "/" ? `/${part}` : current ? `${current}/${part}` : part
        try {
          await this.mkdirOne(current, options)
        } catch (error) {
          const existing = await this.stat(current, options).catch(() => null)
          if (!existing || existing.kind !== "directory") throw error
        }
      }
    } catch (error) {
      throw asOperationError(error, "create remote directory", path)
    }
  }

  async rename(from: string, to: string, options: OperationOptions = {}): Promise<void> {
    try {
      await callVoid((callback) => this.sftp.rename(from, to, callback), options.signal)
    } catch (error) {
      throw asOperationError(error, "rename remote path", from)
    }
  }

  /** Atomically replace when the server implements OpenSSH's posix-rename extension. */
  async replace(from: string, to: string, options: OperationOptions = {}): Promise<void> {
    try {
      await callVoid(
        (callback) => this.sftp.ext_openssh_rename(from, to, callback),
        options.signal,
      )
    } catch (extensionError) {
      if (!unsupportedExtension(extensionError)) {
        throw asOperationError(extensionError, "replace remote path", to)
      }
      throwIfAborted(options.signal)
      const backup = posix.join(
        posix.dirname(to),
        `.${posix.basename(to)}.sshmgr-${randomUUID()}.backup`,
      )
      let backedUp = false
      try {
        await this.rename(to, backup, options)
        backedUp = true
      } catch (error) {
        if (!isNotFoundError(causeOf(error))) {
          throw asOperationError(error, "back up remote destination", to)
        }
      }
      try {
        await this.rename(from, to, options)
        if (backedUp) await this.remove(backup).catch(() => undefined)
      } catch (fallbackError) {
        if (backedUp) {
          try {
            await this.rename(backup, to)
          } catch (restoreError) {
            throw asOperationError(
              restoreError,
              `restore remote destination; original remains at ${backup}`,
              to,
            )
          }
        }
        throw asOperationError(
          fallbackError,
          `replace remote path (posix-rename also failed: ${String(extensionError)})`,
          to,
        )
      }
    }
  }

  async remove(path: string, options: RemoveOptions = {}): Promise<void> {
    try {
      throwIfAborted(options.signal)
      const entry = await this.stat(path, options)
      if (entry.kind !== "directory") {
        await callVoid((callback) => this.sftp.unlink(path, callback), options.signal)
        return
      }

      if (options.recursive) {
        for (const child of await this.list(path, options)) {
          await this.remove(child.path, options)
        }
      }
      await callVoid((callback) => this.sftp.rmdir(path, callback), options.signal)
    } catch (error) {
      throw asOperationError(error, "remove remote path", path)
    }
  }

  private async mkdirOne(path: string, options: MkdirOptions): Promise<void> {
    await callVoid(
      (callback) =>
        options.mode === undefined
          ? this.sftp.mkdir(path, callback)
          : this.sftp.mkdir(path, { mode: options.mode }, callback),
      options.signal,
    )
  }
}
