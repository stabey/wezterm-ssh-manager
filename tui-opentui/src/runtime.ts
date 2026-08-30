import { chmod, mkdtemp, readdir, rename, rm, rmdir, stat } from "node:fs/promises"
import { randomBytes } from "node:crypto"
import { tmpdir } from "node:os"
import { basename, dirname, join, resolve } from "node:path"

export const TOKEN_PATTERN = /^[0-9a-f]{64}$/
export const REQUEST_PATTERN = /^request-[1-9][0-9]*-[0-9a-f]{32}\.json$/

export interface RuntimeContext {
  runtime_dir: string
  token: string
}

export async function createRuntime(): Promise<RuntimeContext> {
  const runtimeDir = await mkdtemp(join(tmpdir(), "wezterm-sshmgr-"))
  if (process.platform !== "win32") await chmod(runtimeDir, 0o700)
  return { runtime_dir: runtimeDir, token: randomBytes(32).toString("hex") }
}

export async function validateRuntime(snapshotPath: string, token: string): Promise<string> {
  if (!TOKEN_PATTERN.test(token)) throw new Error("invalid sshmgr TUI session token")
  const absolute = resolve(snapshotPath)
  if (basename(absolute) !== "snapshot.json") throw new Error("snapshot must be named snapshot.json")
  const snapshotStat = await stat(absolute)
  if (!snapshotStat.isFile()) throw new Error("snapshot not found")
  const runtimeDir = dirname(absolute)
  if (!basename(runtimeDir).startsWith("wezterm-sshmgr-")) throw new Error("invalid runtime directory")
  const runtimeStat = await stat(runtimeDir)
  if (!runtimeStat.isDirectory()) throw new Error("invalid runtime directory")
  if (process.platform !== "win32" && (runtimeStat.mode & 0o077) !== 0) {
    throw new Error("runtime directory is not private")
  }
  return runtimeDir
}

export async function cleanupRuntime(runtimeDir: string): Promise<void> {
  const absolute = resolve(runtimeDir)
  let runtimeStat
  try {
    runtimeStat = await stat(absolute)
  } catch {
    return
  }
  if (!runtimeStat.isDirectory() || !basename(absolute).startsWith("wezterm-sshmgr-")) return
  if (process.platform !== "win32" && (runtimeStat.mode & 0o077) !== 0) return

  for (const entry of await readdir(absolute)) {
    if (
      entry === "snapshot.json" ||
      entry.startsWith("snapshot.json.tmp-") ||
      entry.startsWith("snapshot.json.backup-") ||
      REQUEST_PATTERN.test(entry)
    ) {
      await rm(join(absolute, entry), { force: true }).catch(() => undefined)
    }
  }
  await rmdir(absolute).catch(() => undefined)
}

export async function replaceFile(source: string, destination: string): Promise<void> {
  const resolvedSource = resolve(source)
  const resolvedDestination = resolve(destination)
  try {
    await rename(resolvedSource, resolvedDestination)
    return
  } catch (firstError) {
    const backup = `${resolvedDestination}.backup-${randomBytes(12).toString("hex")}`
    let backedUp = false
    try {
      await rename(resolvedDestination, backup)
      backedUp = true
      await rename(resolvedSource, resolvedDestination)
      await rm(backup, { force: true })
    } catch (replacementError) {
      if (backedUp) {
        try {
          await rename(backup, resolvedDestination)
        } catch (restoreError) {
          throw new Error(
            `cannot replace ${resolvedDestination}; original snapshot remains at ${backup}: ${String(restoreError)}`,
            { cause: replacementError },
          )
        }
      }
      throw backedUp ? replacementError : firstError
    }
  }
}
