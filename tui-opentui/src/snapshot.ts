import { readFile, stat } from "node:fs/promises"
import { normalizeSnapshot } from "./model.ts"
import type { Snapshot } from "./types.ts"

export async function readSnapshot(path: string): Promise<Snapshot> {
  const source = await readFile(path, "utf8")
  return normalizeSnapshot(JSON.parse(source))
}

export function watchSnapshot(
  path: string,
  onSnapshot: (snapshot: Snapshot) => void,
  onError: (error: Error) => void,
  intervalMs = 400,
): () => void {
  let lastMtime = 0
  let reading = false
  const poll = async () => {
    if (reading) return
    reading = true
    try {
      const info = await stat(path)
      if (info.mtimeMs > lastMtime) {
        const snapshot = await readSnapshot(path)
        lastMtime = info.mtimeMs
        onSnapshot(snapshot)
      }
    } catch (error) {
      onError(error instanceof Error ? error : new Error(String(error)))
    } finally {
      reading = false
    }
  }
  void poll()
  const timer = setInterval(() => void poll(), intervalMs)
  return () => clearInterval(timer)
}
