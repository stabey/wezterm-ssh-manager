import { afterEach, describe, expect, test } from "bun:test"
import { chmod, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { parseArgs } from "../src/cli.ts"
import { RequestProtocol } from "../src/protocol.ts"
import { cleanupRuntime, createRuntime, replaceFile, TOKEN_PATTERN } from "../src/runtime.ts"

const created: string[] = []

afterEach(async () => {
  await Promise.all(created.splice(0).map((path) => rm(path, { recursive: true, force: true })))
})

describe("request protocol", () => {
  test("writes an authenticated one-shot request before emitting the OSC wakeup", async () => {
    const runtime = await mkdtemp(join(tmpdir(), "wezterm-sshmgr-test-"))
    created.push(runtime)
    if (process.platform !== "win32") await chmod(runtime, 0o700)
    const token = "a".repeat(64)
    let osc = ""
    const protocol = new RequestProtocol(runtime, token, (value) => {
      osc = value
    })

    const envelope = protocol.emit({ op: "connect", id: "prod/db", where: "tab" })
    expect(envelope).toMatchObject({ v: 2, token, seq: 1 })
    const request = JSON.parse(await readFile(join(runtime, envelope.request), "utf8"))
    expect(request).toEqual({ op: "connect", id: "prod/db", where: "tab", _session: token, _seq: 1 })
    expect(osc.startsWith("\u001b]1337;SetUserVar=sshmgr=")).toBe(true)
    expect(osc.endsWith("\u0007\u001b]1337;SetUserVar=sshmgr=\u0007")).toBe(true)
  })

  test("increments sequence numbers", async () => {
    const runtime = await mkdtemp(join(tmpdir(), "wezterm-sshmgr-test-"))
    created.push(runtime)
    if (process.platform !== "win32") await chmod(runtime, 0o700)
    const protocol = new RequestProtocol(runtime, "b".repeat(64), () => undefined)
    expect(protocol.emit({ op: "reload" }).seq).toBe(1)
    expect(protocol.emit({ op: "hide" }).seq).toBe(2)
  })
})

describe("runtime helpers", () => {
  test("creates a compatible runtime directory and token", async () => {
    const context = await createRuntime()
    created.push(context.runtime_dir)
    expect(TOKEN_PATTERN.test(context.token)).toBe(true)
    expect(context.runtime_dir.split(/[\\/]/).at(-1)?.startsWith("wezterm-sshmgr-")).toBe(true)
  })

  test("cleanup removes only protocol-owned files", async () => {
    const context = await createRuntime()
    created.push(context.runtime_dir)
    await writeFile(join(context.runtime_dir, "snapshot.json"), "{}")
    await writeFile(join(context.runtime_dir, `request-1-${"a".repeat(32)}.json`), "{}")
    await writeFile(join(context.runtime_dir, "keep.txt"), "keep")
    await cleanupRuntime(context.runtime_dir)
    expect(await readdir(context.runtime_dir)).toEqual(["keep.txt"])
  })

  test("cleanup removes an empty owned runtime directory", async () => {
    const context = await createRuntime()
    await writeFile(join(context.runtime_dir, "snapshot.json"), "{}")
    await cleanupRuntime(context.runtime_dir)
    await expect(stat(context.runtime_dir)).rejects.toMatchObject({ code: "ENOENT" })
  })

  test("replace helper overwrites an existing snapshot", async () => {
    const context = await createRuntime()
    created.push(context.runtime_dir)
    const source = join(context.runtime_dir, "snapshot.json.tmp-test")
    const destination = join(context.runtime_dir, "snapshot.json")
    await writeFile(destination, "old")
    await writeFile(source, "new")

    await replaceFile(source, destination)

    expect(await readFile(destination, "utf8")).toBe("new")
    await expect(stat(source)).rejects.toThrow()
  })

  test("parses normal and helper CLI modes", () => {
    expect(parseArgs(["--create-runtime"])).toEqual({ kind: "create-runtime" })
    expect(parseArgs(["--snapshot", "./snapshot.json"])).toMatchObject({ kind: "run" })
    expect(parseArgs(["--cleanup-runtime", "/tmp/wezterm-sshmgr-one"])).toMatchObject({
      kind: "cleanup-runtime",
    })
    expect(parseArgs(["--replace-file", "a", "b"])).toMatchObject({ kind: "replace-file" })
  })
})
