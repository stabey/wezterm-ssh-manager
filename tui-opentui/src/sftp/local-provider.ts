import { lstat, mkdir, readdir, rename, rm, rmdir } from "node:fs/promises"
import { basename, join } from "node:path"

import { asOperationError, throwIfAborted } from "./errors.ts"
import type {
  FileEntry,
  FileKind,
  FileProvider,
  MkdirOptions,
  OperationOptions,
  RemoveOptions,
} from "./types.ts"

function localKind(stats: Awaited<ReturnType<typeof lstat>>): FileKind {
  if (stats.isDirectory()) return "directory"
  if (stats.isFile()) return "file"
  if (stats.isSymbolicLink()) return "symlink"
  return "other"
}

function sortEntries(entries: FileEntry[]): FileEntry[] {
  return entries.sort((left, right) => {
    if (left.kind === "directory" && right.kind !== "directory") return -1
    if (left.kind !== "directory" && right.kind === "directory") return 1
    return left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: "base" })
  })
}

export class LocalFileProvider implements FileProvider {
  readonly kind = "local" as const

  async list(directory: string, options: OperationOptions = {}): Promise<FileEntry[]> {
    throwIfAborted(options.signal)
    try {
      const names = await readdir(directory)
      const entries: FileEntry[] = []
      for (const name of names) {
        throwIfAborted(options.signal)
        entries.push(await this.stat(join(directory, name), options))
      }
      return sortEntries(entries)
    } catch (error) {
      throw asOperationError(error, "list local directory", directory)
    }
  }

  async stat(path: string, options: OperationOptions = {}): Promise<FileEntry> {
    throwIfAborted(options.signal)
    try {
      const stats = await lstat(path)
      throwIfAborted(options.signal)
      return {
        name: basename(path),
        path,
        kind: localKind(stats),
        size: stats.size,
        modifiedAt: Number.isFinite(stats.mtimeMs) ? stats.mtime : null,
        mode: stats.mode,
      }
    } catch (error) {
      throw asOperationError(error, "stat local path", path)
    }
  }

  async mkdir(path: string, options: MkdirOptions = {}): Promise<void> {
    throwIfAborted(options.signal)
    try {
      await mkdir(path, {
        recursive: options.recursive ?? false,
        ...(options.mode === undefined ? {} : { mode: options.mode }),
      })
      throwIfAborted(options.signal)
    } catch (error) {
      throw asOperationError(error, "create local directory", path)
    }
  }

  async rename(from: string, to: string, options: OperationOptions = {}): Promise<void> {
    throwIfAborted(options.signal)
    try {
      await rename(from, to)
      throwIfAborted(options.signal)
    } catch (error) {
      throw asOperationError(error, "rename local path", from)
    }
  }

  async remove(path: string, options: RemoveOptions = {}): Promise<void> {
    throwIfAborted(options.signal)
    try {
      const entry = await lstat(path)
      if (entry.isDirectory() && !options.recursive) await rmdir(path)
      else await rm(path, { recursive: options.recursive ?? false, force: false })
      throwIfAborted(options.signal)
    } catch (error) {
      throw asOperationError(error, "remove local path", path)
    }
  }
}
