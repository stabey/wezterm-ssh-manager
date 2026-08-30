import { closeSync, constants, fsyncSync, openSync, unlinkSync, writeFileSync } from "node:fs"
import { randomBytes } from "node:crypto"
import { basename, join, resolve } from "node:path"
import { REQUEST_PATTERN, TOKEN_PATTERN } from "./runtime.ts"
import type { ManagerRequest, RequestEnvelope } from "./types.ts"

export type TerminalWriter = (data: string) => void

const defaultWriter: TerminalWriter = (data) => process.stdout.write(data)

export class RequestProtocol {
  private sequence = 0

  constructor(
    private readonly runtimeDir: string,
    private readonly token: string,
    private readonly writeTerminal: TerminalWriter = defaultWriter,
  ) {
    if (!basename(resolve(runtimeDir)).startsWith("wezterm-sshmgr-")) throw new Error("invalid runtime directory")
    if (!TOKEN_PATTERN.test(token)) throw new Error("invalid session token")
  }

  emit(message: ManagerRequest): RequestEnvelope {
    this.sequence += 1
    const sequence = this.sequence
    let request = ""
    let path = ""
    let fd: number | undefined
    for (let attempt = 0; attempt < 8; attempt += 1) {
      request = `request-${sequence}-${randomBytes(16).toString("hex")}.json`
      if (!REQUEST_PATTERN.test(request)) continue
      path = join(this.runtimeDir, request)
      try {
        fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o600)
        break
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
      }
    }
    if (fd === undefined) throw new Error("cannot allocate a unique sshmgr request")

    try {
      const body = JSON.stringify({ ...message, _session: this.token, _seq: sequence })
      writeFileSync(fd, body, { encoding: "utf8" })
      fsyncSync(fd)
    } catch (error) {
      closeSync(fd)
      unlinkSync(path)
      throw error
    }
    closeSync(fd)

    const envelope: RequestEnvelope = { v: 2, token: this.token, seq: sequence, request }
    const encoded = Buffer.from(JSON.stringify(envelope), "ascii").toString("base64")
    try {
      this.writeTerminal(`\u001b]1337;SetUserVar=sshmgr=${encoded}\u0007\u001b]1337;SetUserVar=sshmgr=\u0007`)
    } catch (error) {
      try {
        unlinkSync(path)
      } catch {
        // The request may already have been consumed by the Lua side.
      }
      throw error
    }
    return envelope
  }
}
