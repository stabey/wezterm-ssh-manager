import { afterEach, describe, expect, test } from "bun:test"
import { PassThrough, Readable, Writable } from "node:stream"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "node:os"

import type { ClientChannel, ConnectConfig, SFTPWrapper, Stats } from "ssh2"

import type { Profile } from "../src/types.ts"
import {
  connectSftp,
  connectionFromProfile,
  LocalFileProvider,
  RemoteFileProvider,
  SftpAbortError,
  SftpCredentialRequiredError,
  type SshClientAdapter,
  downloadFile,
  uploadFile,
} from "../src/sftp/index.ts"

const temporaryDirectories: string[] = []

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })))
})

async function temporaryDirectory(): Promise<string> {
  const path = await mkdtemp(join(tmpdir(), "sshmgr-sftp-test-"))
  temporaryDirectories.push(path)
  return path
}

function profile(overrides: Partial<Profile> = {}): Profile {
  return {
    id: "target",
    name: "target",
    group: "test",
    editable: true,
    source: "store",
    host: "target.test",
    user: "alice",
    port: 22,
    auth: "password",
    hasPassword: false,
    jumpHost: "",
    icon: "",
    color: "",
    ...overrides,
  }
}

function notFound(path: string): Error & { code: number } {
  return Object.assign(new Error(`No such file: ${path}`), { code: 2 })
}

const DIRECTORY_MODE = 0o040755
const FILE_MODE = 0o100644

function stats(kind: "directory" | "file", size = 0): Stats {
  const mode = kind === "directory" ? DIRECTORY_MODE : FILE_MODE
  return {
    mode,
    uid: 1,
    gid: 1,
    size,
    atime: 1_700_000_000,
    mtime: 1_700_000_001,
    isDirectory: () => kind === "directory",
    isFile: () => kind === "file",
    isBlockDevice: () => false,
    isCharacterDevice: () => false,
    isSymbolicLink: () => false,
    isFIFO: () => false,
    isSocket: () => false,
  }
}

type FakeNode = { kind: "directory"; data?: never } | { kind: "file"; data: Buffer }

class FakeSftp {
  readonly nodes = new Map<string, FakeNode>([["/", { kind: "directory" }]])
  ended = false
  supportsPosixRename = true
  readonly failRenameFrom = new Set<string>()

  addDirectory(path: string): void {
    this.nodes.set(path, { kind: "directory" })
  }

  addFile(path: string, data: string | Buffer): void {
    this.nodes.set(path, { kind: "file", data: Buffer.from(data) })
  }

  lstat(path: string, callback: (error: Error | undefined, value: Stats) => void): void {
    queueMicrotask(() => {
      const node = this.nodes.get(path)
      if (!node) callback(notFound(path), stats("file"))
      else callback(undefined, stats(node.kind, node.kind === "file" ? node.data.length : 0))
    })
  }

  readdir(
    directory: string,
    callback: (
      error: Error | undefined,
      entries: Array<{ filename: string; longname: string; attrs: Stats }>,
    ) => void,
  ): void {
    queueMicrotask(() => {
      const parent = this.nodes.get(directory)
      if (!parent || parent.kind !== "directory") {
        callback(notFound(directory), [])
        return
      }
      const prefix = directory === "/" ? "/" : `${directory}/`
      const entries = [...this.nodes.entries()].flatMap(([path, node]) => {
        if (!path.startsWith(prefix)) return []
        const tail = path.slice(prefix.length)
        if (!tail || tail.includes("/")) return []
        return [{
          filename: tail,
          longname: tail,
          attrs: stats(node.kind, node.kind === "file" ? node.data.length : 0),
        }]
      })
      callback(undefined, entries)
    })
  }

  mkdir(
    path: string,
    attributesOrCallback: object | ((error?: Error | null) => void),
    maybeCallback?: (error?: Error | null) => void,
  ): void {
    const callback =
      typeof attributesOrCallback === "function" ? attributesOrCallback : maybeCallback
    queueMicrotask(() => {
      if (this.nodes.has(path)) callback?.(Object.assign(new Error("exists"), { code: 4 }))
      else {
        this.addDirectory(path)
        callback?.()
      }
    })
  }

  rename(from: string, to: string, callback: (error?: Error | null) => void): void {
    queueMicrotask(() => {
      if (this.failRenameFrom.delete(from)) {
        callback(new Error(`injected rename failure: ${from}`))
        return
      }
      const node = this.nodes.get(from)
      if (!node) callback(notFound(from))
      else {
        this.nodes.delete(from)
        this.nodes.set(to, node)
        callback()
      }
    })
  }

  ext_openssh_rename(from: string, to: string, callback: (error?: Error | null) => void): void {
    if (!this.supportsPosixRename) {
      queueMicrotask(() => callback(Object.assign(new Error("Operation unsupported"), { code: 8 })))
      return
    }
    this.rename(from, to, callback)
  }

  realpath(path: string, callback: (error: Error | undefined, resolved: string) => void): void {
    queueMicrotask(() => callback(undefined, path === "." ? "/home/alice" : path))
  }

  unlink(path: string, callback: (error?: Error | null) => void): void {
    queueMicrotask(() => {
      const node = this.nodes.get(path)
      if (!node || node.kind !== "file") callback(notFound(path))
      else {
        this.nodes.delete(path)
        callback()
      }
    })
  }

  rmdir(path: string, callback: (error?: Error | null) => void): void {
    queueMicrotask(() => {
      const prefix = `${path}/`
      if ([...this.nodes.keys()].some((candidate) => candidate.startsWith(prefix))) {
        callback(new Error("directory not empty"))
      } else {
        this.nodes.delete(path)
        callback()
      }
    })
  }

  createReadStream(path: string): Readable {
    const node = this.nodes.get(path)
    if (!node || node.kind !== "file") {
      return Readable.from((async function* () { throw notFound(path) })())
    }
    return Readable.from(node.data)
  }

  createWriteStream(path: string): Writable {
    const chunks: Buffer[] = []
    return new Writable({
      write(chunk: Buffer, _encoding, done) {
        chunks.push(Buffer.from(chunk))
        setImmediate(done)
      },
      final: (done) => {
        this.addFile(path, Buffer.concat(chunks))
        done()
      },
    })
  }

  utimes(
    _path: string,
    _atime: Date | number,
    _mtime: Date | number,
    callback: (error?: Error | null) => void,
  ): void {
    queueMicrotask(callback)
  }

  end(): void {
    this.ended = true
  }

  wrapper(): SFTPWrapper {
    return this as unknown as SFTPWrapper
  }
}

describe("profile compatibility", () => {
  test("prefers normalized sftp details and maps password override plus a jump agent", () => {
    const target = profile({
      host: "summary-would-be-wrong.test",
      jumpHost: "jump",
      sftp: {
        host: "files.example.com",
        user: "deploy",
        port: 2222,
        auth: "password",
        jumpHost: "jump",
      },
    })
    const jump = profile({
      id: "jump",
      name: "jump",
      host: "jump.summary",
      auth: "agent",
      jumpHost: "",
      sftp: {
        host: "bastion.example",
        user: "ops",
        auth: "agent",
        identityAgent: "/tmp/custom-agent.sock",
      },
    })

    const result = connectionFromProfile(target, [target, jump], { password: "secret" })

    expect(result.supported).toBe(true)
    expect(result.connection.host).toBe("files.example.com")
    expect(result.connection.port).toBe(2222)
    expect(result.connection.authentication?.password).toBe("secret")
    expect(result.connection.jump?.host).toBe("bastion.example")
    expect(result.connection.jump?.authentication?.agent).toBe("/tmp/custom-agent.sock")
    expect(result.issues).toEqual([])
  })

  test("reports a missing password and unsupported proxy command", () => {
    const target = profile({
      sftp: {
        host: "target.test",
        auth: "password",
        proxyCommand: "ssh proxy nc %h %p",
        privateKeys: ["~/.ssh/id_one", "~/.ssh/id_two"],
      },
    })
    const result = connectionFromProfile(target, [target])

    expect(result.supported).toBe(false)
    expect(result.issues.map((issue) => [issue.field, issue.severity])).toContainEqual([
      "password",
      "needs-input",
    ])
    expect(result.issues.map((issue) => [issue.field, issue.severity])).toContainEqual([
      "proxyCommand",
      "unsupported",
    ])
    expect(result.connection.authentication?.privateKeys).toHaveLength(2)
  })

  test("resolves password_env without executing password_cmd", () => {
    const target = profile({
      sftp: { host: "target.test", auth: "password", password_env: "TEST_SFTP_PASSWORD" },
    })
    const result = connectionFromProfile(target, [target], {
      environment: { TEST_SFTP_PASSWORD: "from-env" },
    })
    expect(result.connection.authentication?.password).toBe("from-env")
    expect(result.issues).toEqual([])
  })

  test("does not let a global agent suppress a password prompt", () => {
    const target = profile({
      sftp: {
        host: "target.test",
        auth: "password",
        identityAgent: "/tmp/global-agent.sock",
      },
    })

    const result = connectionFromProfile(target, [target])

    expect(result.connection.authentication?.agent).toBeUndefined()
    expect(result.issues.map((issue) => [issue.field, issue.severity])).toContainEqual([
      "password",
      "needs-input",
    ])
  })
})

describe("local and remote file providers", () => {
  test("lists, renames, and recursively removes local paths", async () => {
    const root = await temporaryDirectory()
    const provider = new LocalFileProvider()
    await provider.mkdir(join(root, "directory", "nested"), { recursive: true })
    await writeFile(join(root, "z.txt"), "z")
    await writeFile(join(root, "a.txt"), "a")

    expect((await provider.list(root)).map((entry) => entry.name)).toEqual([
      "directory",
      "a.txt",
      "z.txt",
    ])
    await provider.rename(join(root, "a.txt"), join(root, "renamed.txt"))
    expect((await provider.stat(join(root, "renamed.txt"))).size).toBe(1)
    await provider.remove(join(root, "directory"), { recursive: true })
    expect((await provider.list(root)).map((entry) => entry.name)).toEqual([
      "renamed.txt",
      "z.txt",
    ])
  })

  test("honors an already-aborted operation", async () => {
    const controller = new AbortController()
    controller.abort("test")
    await expect(new LocalFileProvider().list(".", { signal: controller.signal })).rejects.toBeInstanceOf(
      SftpAbortError,
    )
  })

  test("lists directories first and recursively removes a remote tree", async () => {
    const fake = new FakeSftp()
    fake.addDirectory("/work")
    fake.addDirectory("/work/folder")
    fake.addFile("/work/file.txt", "file")
    fake.addFile("/work/folder/nested.txt", "nested")
    const provider = new RemoteFileProvider(fake.wrapper())

    expect((await provider.list("/work")).map((entry) => entry.name)).toEqual([
      "folder",
      "file.txt",
    ])
    await provider.mkdir("/work/new/deep", { recursive: true })
    expect(fake.nodes.get("/work/new/deep")?.kind).toBe("directory")
    await provider.remove("/work/folder", { recursive: true })
    expect(fake.nodes.has("/work/folder")).toBe(false)
    expect(fake.nodes.has("/work/folder/nested.txt")).toBe(false)
    expect(await provider.realpath()).toBe("/home/alice")
  })

  test("restores the remote destination when a portable overwrite fails", async () => {
    const fake = new FakeSftp()
    fake.supportsPosixRename = false
    fake.addFile("/old.txt", "old")
    fake.addFile("/temporary.txt", "new")
    fake.failRenameFrom.add("/temporary.txt")
    const provider = new RemoteFileProvider(fake.wrapper())

    await expect(provider.replace("/temporary.txt", "/old.txt")).rejects.toThrow(
      "injected rename failure",
    )

    expect(fake.nodes.get("/old.txt")).toEqual({ kind: "file", data: Buffer.from("old") })
    expect([...fake.nodes.keys()].some((path) => path.endsWith(".backup"))).toBe(false)
  })
})

describe("streaming transfers", () => {
  test("uploads and downloads atomically with byte progress", async () => {
    const root = await temporaryDirectory()
    const fake = new FakeSftp()
    fake.addDirectory("/remote")
    const source = join(root, "source.bin")
    const downloaded = join(root, "nested", "downloaded.bin")
    const contents = Buffer.from("cross-platform sftp data")
    await writeFile(source, contents)
    const uploadProgress: number[] = []

    await uploadFile(fake.wrapper(), source, "/remote/uploaded.bin", {
      onProgress: (progress) => uploadProgress.push(progress.transferredBytes),
    })
    expect(fake.nodes.get("/remote/uploaded.bin")).toEqual({ kind: "file", data: contents })
    expect(uploadProgress.at(-1)).toBe(contents.length)

    await downloadFile(fake.wrapper(), "/remote/uploaded.bin", downloaded)
    expect(await readFile(downloaded)).toEqual(contents)
  })

  test("cancels an in-flight upload and removes the temporary remote file", async () => {
    const root = await temporaryDirectory()
    const fake = new FakeSftp()
    fake.addDirectory("/remote")
    const source = join(root, "large.bin")
    await writeFile(source, Buffer.alloc(1024 * 1024, 7))
    const controller = new AbortController()

    await expect(
      uploadFile(fake.wrapper(), source, "/remote/cancelled.bin", {
        signal: controller.signal,
        onProgress: (progress) => {
          if (progress.transferredBytes > 0) controller.abort("cancel-test")
        },
      }),
    ).rejects.toBeInstanceOf(SftpAbortError)

    expect(fake.nodes.has("/remote/cancelled.bin")).toBe(false)
    expect([...fake.nodes.keys()].some((path) => path.endsWith(".part"))).toBe(false)
  })
})

class FakeClient implements SshClientAdapter {
  readonly configs: ConnectConfig[] = []
  ended = false
  #ready = new Set<() => void>()
  #errors = new Set<(error: Error) => void>()
  #close = new Set<() => void>()
  #end = new Set<() => void>()

  constructor(private readonly fakeSftp: FakeSftp) {}

  connect(config: ConnectConfig): void {
    this.configs.push(config)
    queueMicrotask(() => this.#ready.forEach((listener) => listener()))
  }

  end(): void {
    this.ended = true
  }

  once(event: "ready", listener: () => void): this
  once(event: "error", listener: (error: Error) => void): this
  once(event: "close" | "end", listener: () => void): this
  once(event: "ready" | "error" | "close" | "end", listener: (() => void) | ((error: Error) => void)): this {
    if (event === "ready") this.#ready.add(listener as () => void)
    else if (event === "error") this.#errors.add(listener as (error: Error) => void)
    else if (event === "close") this.#close.add(listener as () => void)
    else this.#end.add(listener as () => void)
    return this
  }

  on(event: "error", listener: (error: Error) => void): this
  on(event: "close" | "end", listener: () => void): this
  on(event: "error" | "close" | "end", listener: (() => void) | ((error: Error) => void)): this {
    if (event === "error") this.#errors.add(listener as (error: Error) => void)
    else if (event === "close") this.#close.add(listener as () => void)
    else this.#end.add(listener as () => void)
    return this
  }

  off(event: "ready", listener: () => void): this
  off(event: "error", listener: (error: Error) => void): this
  off(event: "close" | "end", listener: () => void): this
  off(event: "ready" | "error" | "close" | "end", listener: (() => void) | ((error: Error) => void)): this {
    if (event === "ready") this.#ready.delete(listener as () => void)
    else if (event === "error") this.#errors.delete(listener as (error: Error) => void)
    else if (event === "close") this.#close.delete(listener as () => void)
    else this.#end.delete(listener as () => void)
    return this
  }

  emitError(error: Error): void {
    for (const listener of this.#errors) listener(error)
  }

  sftp(callback: (error: Error | undefined, sftp: SFTPWrapper) => void): void {
    queueMicrotask(() => callback(undefined, this.fakeSftp.wrapper()))
  }

  forwardOut(
    _sourceIP: string,
    _sourcePort: number,
    _destinationIP: string,
    _destinationPort: number,
    callback: (error: Error | undefined, channel: ClientChannel) => void,
  ): void {
    queueMicrotask(() => callback(undefined, new PassThrough() as unknown as ClientChannel))
  }
}

describe("SSH connection setup", () => {
  test("uses the conventional OpenSSH agent named pipe on Windows", async () => {
    const fakeSftp = new FakeSftp()
    const client = new FakeClient(fakeSftp)
    const session = await connectSftp(
      {
        host: "target.test",
        username: "alice",
        authentication: { agent: true },
      },
      {},
      {
        createClient: () => client,
        platform: "win32",
        environment: {},
      },
    )

    const authHandler = client.configs[0]?.authHandler
    expect(Array.isArray(authHandler)).toBe(true)
    expect(authHandler as unknown[]).toContainEqual({
      type: "agent",
      username: "alice",
      agent: "\\\\.\\pipe\\openssh-ssh-agent",
    })
    session.close()
  })

  test("opens a separate target connection through one jump host", async () => {
    const fakeSftp = new FakeSftp()
    const clients: FakeClient[] = []
    const session = await connectSftp(
      {
        host: "target.example.com",
        username: "target-user",
        authentication: { password: "target-password" },
        jump: {
          host: "jump.example",
          username: "jump-user",
          authentication: { agent: "/tmp/agent.sock" },
        },
      },
      {},
      {
        createClient: () => {
          const client = new FakeClient(fakeSftp)
          clients.push(client)
          return client
        },
        platform: "darwin",
        environment: {},
      },
    )

    expect(clients).toHaveLength(2)
    expect(clients[0]?.configs[0]?.host).toBe("jump.example")
    expect(clients[1]?.configs[0]?.host).toBe("target.example.com")
    expect(clients[1]?.configs[0]?.sock).toBeDefined()
    session.close()
    expect(clients.every((client) => client.ended)).toBe(true)
    expect(fakeSftp.ended).toBe(true)
  })

  test("marks a live session closed after a transport error", async () => {
    const fakeSftp = new FakeSftp()
    const client = new FakeClient(fakeSftp)
    const session = await connectSftp(
      { host: "target.test", username: "alice", authentication: { password: "secret" } },
      {},
      { createClient: () => client },
    )
    let reason: Error | undefined
    session.onDisconnected((error) => { reason = error })

    client.emitError(new Error("transport lost"))

    expect(session.closed).toBe(true)
    expect(reason?.message).toBe("transport lost")
  })

  test("fails before network I/O when credentials are absent", async () => {
    let madeClient = false
    await expect(
      connectSftp(
        { host: "target.test" },
        {},
        {
          createClient: () => {
            madeClient = true
            return new FakeClient(new FakeSftp())
          },
        },
      ),
    ).rejects.toBeInstanceOf(SftpCredentialRequiredError)
    expect(madeClient).toBe(true)
  })
})
