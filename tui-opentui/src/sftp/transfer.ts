import { createReadStream, createWriteStream } from "node:fs"
import { access, lstat, mkdir, rename, rm, utimes } from "node:fs/promises"
import { basename, dirname, join } from "node:path"
import { posix } from "node:path"
import { randomUUID } from "node:crypto"
import { Transform } from "node:stream"
import { pipeline } from "node:stream/promises"

import type { SFTPWrapper } from "ssh2"

import {
  asOperationError,
  isNotFoundError,
  SftpAbortError,
  SftpManagerError,
  throwIfAborted,
} from "./errors.ts"
import { RemoteFileProvider } from "./remote-provider.ts"
import type { TransferDirection, TransferOptions, TransferPhase, TransferProgress } from "./types.ts"

function report(
  direction: TransferDirection,
  phase: TransferPhase,
  source: string,
  destination: string,
  transferredBytes: number,
  totalBytes: number | null,
  startedAt: number,
  callback?: (progress: TransferProgress) => void,
): void {
  if (!callback) return
  const elapsedMs = Math.max(0, Date.now() - startedAt)
  const percent = totalBytes === null ? null : totalBytes === 0 ? 100 : (transferredBytes / totalBytes) * 100
  const progress: TransferProgress = {
    direction,
    phase,
    source,
    destination,
    transferredBytes,
    totalBytes,
    percent,
    elapsedMs,
    bytesPerSecond: elapsedMs === 0 ? 0 : (transferredBytes * 1000) / elapsedMs,
  }
  try {
    callback(progress)
  } catch {
    // UI progress rendering must never corrupt an in-flight transfer.
  }
}

function meter(
  direction: TransferDirection,
  source: string,
  destination: string,
  totalBytes: number | null,
  startedAt: number,
  callback?: (progress: TransferProgress) => void,
): { stream: Transform; bytes: () => number } {
  let transferred = 0
  const stream = new Transform({
    transform(chunk: Buffer, _encoding, done) {
      transferred += chunk.length
      report(
        direction,
        "transferring",
        source,
        destination,
        transferred,
        totalBytes,
        startedAt,
        callback,
      )
      done(null, chunk)
    },
  })
  return { stream, bytes: () => transferred }
}

async function localExists(path: string): Promise<boolean> {
  try {
    await access(path)
    return true
  } catch (error) {
    if (isNotFoundError(error)) return false
    throw error
  }
}

async function remoteExists(remote: RemoteFileProvider, path: string): Promise<boolean> {
  try {
    await remote.stat(path)
    return true
  } catch (error) {
    const cause = error instanceof Error && "cause" in error ? error.cause : error
    if (isNotFoundError(cause)) return false
    throw error
  }
}

async function replaceLocal(from: string, to: string, overwrite: boolean): Promise<void> {
  try {
    await rename(from, to)
  } catch (error) {
    if (!overwrite) throw error
    const code = error && typeof error === "object" ? (error as { code?: unknown }).code : undefined
    if (!["EEXIST", "EPERM", "EACCES", "ENOTEMPTY"].includes(String(code))) throw error
    // Windows does not reliably replace an existing path with rename(). Move
    // the old file aside first so a failed second rename can be rolled back.
    const backup = `${to}.sshmgr-${randomUUID()}.backup`
    await rename(to, backup)
    try {
      await rename(from, to)
      await rm(backup, { force: true })
    } catch (replacementError) {
      try {
        await rename(backup, to)
      } catch (restoreError) {
        throw new SftpManagerError(
          "REPLACE_FAILED",
          `Local replace failed; original remains at ${backup}: ${String(restoreError)}`,
          { path: to, cause: replacementError },
        )
      }
      throw replacementError
    }
  }
}

function localTemporary(path: string): string {
  return join(dirname(path), `.${basename(path)}.sshmgr-${randomUUID()}.part`)
}

function remoteTemporary(path: string): string {
  return posix.join(posix.dirname(path), `.${posix.basename(path)}.sshmgr-${randomUUID()}.part`)
}

export async function uploadFile(
  sftp: SFTPWrapper,
  localPath: string,
  remotePath: string,
  options: TransferOptions = {},
): Promise<void> {
  throwIfAborted(options.signal)
  const remote = new RemoteFileProvider(sftp)
  const overwrite = options.overwrite ?? false
  const atomic = options.atomic ?? true
  const temporary = atomic ? remoteTemporary(remotePath) : remotePath
  const startedAt = Date.now()
  let meterState: ReturnType<typeof meter> | undefined
  try {
    const source = await lstat(localPath)
    if (!source.isFile()) throw new SftpManagerError("NOT_A_FILE", `${localPath} is not a regular file`)
    if (!overwrite && (await remoteExists(remote, remotePath))) {
      throw new SftpManagerError("DESTINATION_EXISTS", `Remote destination already exists: ${remotePath}`, {
        path: remotePath,
      })
    }
    if (options.createParents ?? true) {
      const parent = posix.dirname(remotePath)
      if (parent && parent !== "." && parent !== "/") {
        await remote.mkdir(parent, {
          recursive: true,
          ...(options.signal ? { signal: options.signal } : {}),
        })
      }
    }

    report("upload", "starting", localPath, remotePath, 0, source.size, startedAt, options.onProgress)
    meterState = meter("upload", localPath, remotePath, source.size, startedAt, options.onProgress)
    await pipeline(
      createReadStream(localPath),
      meterState.stream,
      sftp.createWriteStream(temporary, { flags: "w" }),
      options.signal ? { signal: options.signal } : {},
    )
    report(
      "upload",
      "finishing",
      localPath,
      remotePath,
      meterState.bytes(),
      source.size,
      startedAt,
      options.onProgress,
    )
    if (options.preserveTimes) {
      await new Promise<void>((resolve, reject) =>
        sftp.utimes(temporary, source.atime, source.mtime, (error) => (error ? reject(error) : resolve())),
      )
    }
    if (atomic) {
      const operation = options.signal ? { signal: options.signal } : {}
      if (overwrite) await remote.replace(temporary, remotePath, operation)
      else await remote.rename(temporary, remotePath, operation)
    }
    report(
      "upload",
      "completed",
      localPath,
      remotePath,
      meterState.bytes(),
      source.size,
      startedAt,
      options.onProgress,
    )
  } catch (error) {
    if (temporary !== remotePath || options.signal?.aborted) {
      await remote.remove(temporary).catch(() => undefined)
    }
    if (options.signal?.aborted) throw new SftpAbortError("Upload was cancelled", options.signal.reason)
    throw asOperationError(error, "upload", remotePath)
  }
}

export async function downloadFile(
  sftp: SFTPWrapper,
  remotePath: string,
  localPath: string,
  options: TransferOptions = {},
): Promise<void> {
  throwIfAborted(options.signal)
  const remote = new RemoteFileProvider(sftp)
  const overwrite = options.overwrite ?? false
  const atomic = options.atomic ?? true
  const temporary = atomic ? localTemporary(localPath) : localPath
  const startedAt = Date.now()
  let meterState: ReturnType<typeof meter> | undefined
  try {
    const source = await remote.stat(
      remotePath,
      options.signal ? { signal: options.signal } : {},
    )
    if (source.kind !== "file") throw new SftpManagerError("NOT_A_FILE", `${remotePath} is not a regular file`)
    if (!overwrite && (await localExists(localPath))) {
      throw new SftpManagerError("DESTINATION_EXISTS", `Local destination already exists: ${localPath}`, {
        path: localPath,
      })
    }
    if (options.createParents ?? true) await mkdir(dirname(localPath), { recursive: true })

    report("download", "starting", remotePath, localPath, 0, source.size, startedAt, options.onProgress)
    meterState = meter("download", remotePath, localPath, source.size, startedAt, options.onProgress)
    await pipeline(
      sftp.createReadStream(remotePath),
      meterState.stream,
      createWriteStream(temporary, { flags: "w" }),
      options.signal ? { signal: options.signal } : {},
    )
    report(
      "download",
      "finishing",
      remotePath,
      localPath,
      meterState.bytes(),
      source.size,
      startedAt,
      options.onProgress,
    )
    if (atomic) {
      await replaceLocal(temporary, localPath, overwrite)
    }
    if (options.preserveTimes && source.modifiedAt) {
      await utimes(localPath, new Date(), source.modifiedAt)
    }
    report(
      "download",
      "completed",
      remotePath,
      localPath,
      meterState.bytes(),
      source.size,
      startedAt,
      options.onProgress,
    )
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => undefined)
    if (options.signal?.aborted) throw new SftpAbortError("Download was cancelled", options.signal.reason)
    throw asOperationError(error, "download", remotePath)
  }
}
