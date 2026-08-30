import { createHash } from "node:crypto"
import { createReadStream } from "node:fs"
import { chmod, copyFile, cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises"
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = dirname(fileURLToPath(import.meta.url))
const tuiRoot = dirname(scriptDir)
const repositoryRoot = dirname(tuiRoot)
const defaultDist = join(tuiRoot, "dist")

const packageManifest = JSON.parse(await readFile(join(tuiRoot, "package.json"), "utf8")) as {
  version?: unknown
}
if (
  typeof packageManifest.version !== "string" ||
  !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(packageManifest.version)
) {
  throw new Error("package.json must contain a valid semantic version")
}

const PROJECT_NAME = "wezterm-ssh-manager"
const PROJECT_VERSION = packageManifest.version
const REQUIRED_BUN_VERSION = "1.3.13"
const BUN_TAG = "bun-v1.3.13"
const WEBKIT_REVISION = "4d5e75ebd84a14edbc7ae264245dcd77fe597c10"

type TargetName = "macos-arm64" | "macos-x64" | "windows-x64"

type TargetSpec = {
  executable: string
  archiveExtension: ".tar.gz" | ".zip"
}

const targets: Record<TargetName, TargetSpec> = {
  "macos-arm64": {
    executable: "sshmgr-tui-macos-arm64",
    archiveExtension: ".tar.gz",
  },
  "macos-x64": {
    executable: "sshmgr-tui-macos-x64",
    archiveExtension: ".tar.gz",
  },
  "windows-x64": {
    executable: "sshmgr-tui-windows-x64.exe",
    archiveExtension: ".zip",
  },
}

const requiredLicenseFiles = [
  "bun-1.3.13/LICENSE.md",
  "bun-1.3.13/LGPL-2.0.txt",
  "bun-1.3.13/LGPL-2.1.txt",
  "npm/ansi-regex-6.2.2-LICENSE",
  "npm/asn1-0.2.6-LICENSE",
  "npm/bcrypt-pbkdf-1.0.2-LICENSE",
  "npm/bun-ffi-structs-0.3.1-LICENSE",
  "npm/diff-9.0.0-LICENSE",
  "npm/emoji-regex-10.6.0-LICENSE",
  "npm/entities-7.0.1-LICENSE",
  "npm/get-east-asian-width-1.4.0-LICENSE",
  "npm/marked-17.0.1-LICENSE",
  "npm/opentui-core-0.5.9-LICENSE",
  "npm/opentui-solid-0.5.9-LICENSE",
  "npm/safer-buffer-2.1.2-LICENSE",
  "npm/solid-js-1.9.12-LICENSE",
  "npm/ssh2-1.17.0-LICENSE",
  "npm/string-width-7.2.0-LICENSE",
  "npm/strip-ansi-7.1.2-LICENSE",
  "npm/tweetnacl-0.14.5-LICENSE",
  "npm/web-tree-sitter-0.25.10-LICENSE",
  "opentui-native-0.5.9/AUTHORS-LIBWEBP",
  "opentui-native-0.5.9/LICENSE",
  "opentui-native-0.5.9/LICENSE-GHOSTTY",
  "opentui-native-0.5.9/LICENSE-LCMS2",
  "opentui-native-0.5.9/LICENSE-LIBWEBP",
  "opentui-native-0.5.9/LICENSE-STB",
  "opentui-native-0.5.9/LICENSE-WUFFS",
  "opentui-native-0.5.9/PATENTS-LIBWEBP",
  "tree-sitter/helix-MPL-2.0.txt",
  "tree-sitter/nvim-treesitter-LICENSE",
  "tree-sitter/tree-sitter-javascript-0.25.0-LICENSE",
  "tree-sitter/tree-sitter-markdown-0.5.1-LICENSE",
  "tree-sitter/tree-sitter-typescript-0.23.2-LICENSE",
  "tree-sitter/tree-sitter-zig-1.1.2-LICENSE",
] as const

type Options = {
  executable?: string
  outputRoot: string
  requestedTarget?: TargetName
  sourceCommit?: string
  allowBunVersionMismatch: boolean
}

const usage = `Usage:
  bun run scripts/package.ts [--target TARGET] [--executable PATH]
                             [--output DIRECTORY] [--source-commit SHA]
                             [--allow-bun-version-mismatch]

Without --target or --executable, every known executable found in dist/ is
packaged. TARGET is one of: ${Object.keys(targets).join(", ")}.
`

function parseOptions(argv: string[]): Options {
  const options: Options = {
    outputRoot: join(defaultDist, "packages"),
    allowBunVersionMismatch: false,
  }

  const nextValue = (index: number, flag: string): string => {
    const value = argv[index + 1]
    if (!value || value.startsWith("--")) throw new Error(`${flag} requires a value`)
    return value
  }

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    switch (arg) {
      case "--executable":
        options.executable = resolve(process.cwd(), nextValue(index, arg))
        index += 1
        break
      case "--output":
        options.outputRoot = resolve(process.cwd(), nextValue(index, arg))
        index += 1
        break
      case "--source-commit":
        options.sourceCommit = nextValue(index, arg)
        index += 1
        break
      case "--target": {
        const target = nextValue(index, arg)
        if (!(target in targets)) throw new Error(`unknown target: ${target}`)
        options.requestedTarget = target as TargetName
        index += 1
        break
      }
      case "--allow-bun-version-mismatch":
        options.allowBunVersionMismatch = true
        break
      case "--help":
      case "-h":
        process.stdout.write(usage)
        process.exit(0)
      default:
        throw new Error(`unknown argument: ${arg}`)
    }
  }

  if (options.executable && !options.requestedTarget) {
    const executableName = basename(options.executable)
    const inferred = Object.entries(targets).find(([, spec]) => spec.executable === executableName)?.[0]
    if (!inferred) {
      throw new Error(`cannot infer target from executable name: ${executableName}`)
    }
    options.requestedTarget = inferred as TargetName
  }

  return options
}

async function isNonEmptyFile(path: string): Promise<boolean> {
  try {
    const details = await stat(path)
    return details.isFile() && details.size > 0
  } catch {
    return false
  }
}

async function validateInputs(licenseRoot: string): Promise<void> {
  const requiredDocuments = [
    join(repositoryRoot, "LICENSE"),
    join(repositoryRoot, "THIRD_PARTY_NOTICES.md"),
    join(repositoryRoot, "REBUILDING.md"),
    ...requiredLicenseFiles.map((path) => join(licenseRoot, path)),
  ]

  const missing: string[] = []
  for (const path of requiredDocuments) {
    if (!(await isNonEmptyFile(path))) missing.push(relative(repositoryRoot, path))
  }
  if (missing.length > 0) {
    throw new Error(`required compliance files are missing or empty:\n${missing.join("\n")}`)
  }
}

function assertOutputChild(outputRoot: string, candidate: string): void {
  const child = relative(outputRoot, candidate)
  if (!child || child.startsWith("..") || isAbsolute(child) || dirname(candidate) !== outputRoot) {
    throw new Error(`refusing to replace path outside the package output directory: ${candidate}`)
  }
}

async function sha256(path: string): Promise<string> {
  const hash = createHash("sha256")
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest("hex")
}

function buildTimestamp(): string {
  const rawEpoch = process.env.SOURCE_DATE_EPOCH
  if (!rawEpoch) return new Date().toISOString()
  const epoch = Number(rawEpoch)
  if (!Number.isFinite(epoch) || epoch < 0) {
    throw new Error(`invalid SOURCE_DATE_EPOCH: ${rawEpoch}`)
  }
  return new Date(epoch * 1000).toISOString()
}

async function runCommand(
  command: string[],
  cwd: string,
  env: Record<string, string | undefined> = process.env,
): Promise<void> {
  const child = Bun.spawn({ cmd: command, cwd, env, stdout: "inherit", stderr: "inherit" })
  const exitCode = await child.exited
  if (exitCode !== 0) throw new Error(`${command[0]} exited with status ${exitCode}`)
}

async function createArchive(
  target: TargetName,
  stageDir: string,
  archivePath: string,
  outputRoot: string,
): Promise<void> {
  assertOutputChild(outputRoot, archivePath)
  await rm(archivePath, { force: true })

  if (targets[target].archiveExtension === ".tar.gz") {
    const tar = Bun.which("tar")
    if (!tar) throw new Error("tar is required to create macOS release archives")
    await runCommand([tar, "-czf", archivePath, "-C", outputRoot, basename(stageDir)], outputRoot)
    return
  }

  if (process.platform === "win32") {
    const powershell = Bun.which("pwsh") ?? Bun.which("powershell.exe") ?? Bun.which("powershell")
    if (!powershell) throw new Error("PowerShell is required to create Windows release archives")
    const command = [
      powershell,
      "-NoLogo",
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$ErrorActionPreference='Stop'; Compress-Archive -LiteralPath $env:SSHMGR_PACKAGE_STAGE -DestinationPath $env:SSHMGR_PACKAGE_ARCHIVE -CompressionLevel Optimal -Force",
    ]
    await runCommand(command, outputRoot, {
      ...process.env,
      SSHMGR_PACKAGE_STAGE: stageDir,
      SSHMGR_PACKAGE_ARCHIVE: archivePath,
    })
    return
  }

  const zip = Bun.which("zip")
  if (!zip) throw new Error("zip is required to create Windows archives on this host")
  await runCommand([zip, "-qry", archivePath, basename(stageDir)], outputRoot)
}

async function packageTarget(
  target: TargetName,
  executablePath: string,
  options: Options,
  licenseRoot: string,
): Promise<string> {
  if (!(await isNonEmptyFile(executablePath))) {
    throw new Error(`executable is missing or empty: ${executablePath}`)
  }
  const expectedName = targets[target].executable
  if (basename(executablePath) !== expectedName) {
    throw new Error(`${target} executable must be named ${expectedName}`)
  }

  const packageName = `sshmgr-tui-${PROJECT_VERSION}-${target}`
  const stageDir = join(options.outputRoot, packageName)
  const archivePath = join(options.outputRoot, `${packageName}${targets[target].archiveExtension}`)
  assertOutputChild(options.outputRoot, stageDir)

  await rm(stageDir, { recursive: true, force: true })
  await mkdir(stageDir, { recursive: true })
  await copyFile(executablePath, join(stageDir, expectedName))
  if (target !== "windows-x64") await chmod(join(stageDir, expectedName), 0o755)

  await copyFile(join(repositoryRoot, "LICENSE"), join(stageDir, "LICENSE"))
  await copyFile(
    join(repositoryRoot, "THIRD_PARTY_NOTICES.md"),
    join(stageDir, "THIRD_PARTY_NOTICES.md"),
  )
  await copyFile(join(repositoryRoot, "REBUILDING.md"), join(stageDir, "REBUILDING.md"))
  await cp(licenseRoot, join(stageDir, "licenses"), { recursive: true, force: true })

  const sourceCommit =
    options.sourceCommit ?? process.env.GITHUB_SHA ?? process.env.SOURCE_COMMIT ?? null
  const metadata = {
    schemaVersion: 1,
    project: PROJECT_NAME,
    version: PROJECT_VERSION,
    target,
    executable: expectedName,
    executableSha256: await sha256(executablePath),
    sourceCommit,
    createdAt: buildTimestamp(),
    bunVersion: Bun.version,
    bunRevision: Bun.revision,
    bunTag: BUN_TAG,
    webkitRevision: WEBKIT_REVISION,
  }
  await writeFile(join(stageDir, "BUILD-METADATA.json"), `${JSON.stringify(metadata, null, 2)}\n`)

  await createArchive(target, stageDir, archivePath, options.outputRoot)
  if (!(await isNonEmptyFile(archivePath))) throw new Error(`archive was not created: ${archivePath}`)
  process.stdout.write(`packaged ${target}: ${archivePath}\n`)
  return archivePath
}

async function main(): Promise<void> {
  const options = parseOptions(process.argv.slice(2))
  if (Bun.version !== REQUIRED_BUN_VERSION && !options.allowBunVersionMismatch) {
    throw new Error(
      `packaging requires Bun ${REQUIRED_BUN_VERSION}; found ${Bun.version}. ` +
        "Use --allow-bun-version-mismatch only for a locally rebuilt Bun runtime.",
    )
  }

  const licenseRoot = join(tuiRoot, "licenses")
  await validateInputs(licenseRoot)
  await mkdir(options.outputRoot, { recursive: true })

  const selected: Array<[TargetName, string]> = []
  if (options.requestedTarget) {
    const spec = targets[options.requestedTarget]
    selected.push([
      options.requestedTarget,
      options.executable ?? join(defaultDist, spec.executable),
    ])
  } else {
    for (const [target, spec] of Object.entries(targets) as Array<[TargetName, TargetSpec]>) {
      const candidate = join(defaultDist, spec.executable)
      if (await isNonEmptyFile(candidate)) selected.push([target, candidate])
    }
  }

  if (selected.length === 0) {
    throw new Error("no known executable found in dist/; run the build first or pass --executable")
  }

  for (const [target, executable] of selected) {
    await packageTarget(target, executable, options, licenseRoot)
  }
}

try {
  await main()
} catch (error) {
  const message = error instanceof Error ? error.message : String(error)
  process.stderr.write(`package failed: ${message}\n\n${usage}`)
  process.exit(1)
}
