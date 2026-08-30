import { resolve } from "node:path"
import { cleanupRuntime, createRuntime, replaceFile } from "./runtime.ts"

export type CliCommand =
  | { kind: "run"; snapshotPath: string }
  | { kind: "create-runtime" }
  | { kind: "cleanup-runtime"; runtimeDir: string }
  | { kind: "replace-file"; source: string; destination: string }
  | { kind: "help" }

export function parseArgs(argv: string[]): CliCommand {
  if (argv.includes("--create-runtime")) return { kind: "create-runtime" }
  const cleanupIndex = argv.indexOf("--cleanup-runtime")
  const cleanupTarget = cleanupIndex >= 0 ? argv[cleanupIndex + 1] : undefined
  if (cleanupTarget) {
    return { kind: "cleanup-runtime", runtimeDir: resolve(cleanupTarget) }
  }
  const replaceIndex = argv.indexOf("--replace-file")
  const replaceSource = replaceIndex >= 0 ? argv[replaceIndex + 1] : undefined
  const replaceDestination = replaceIndex >= 0 ? argv[replaceIndex + 2] : undefined
  if (replaceSource && replaceDestination) {
    return {
      kind: "replace-file",
      source: resolve(replaceSource),
      destination: resolve(replaceDestination),
    }
  }
  const snapshotIndex = argv.indexOf("--snapshot")
  const snapshotTarget = snapshotIndex >= 0 ? argv[snapshotIndex + 1] : undefined
  if (snapshotTarget) {
    return { kind: "run", snapshotPath: resolve(snapshotTarget) }
  }
  return { kind: "help" }
}

export async function runHelper(command: Exclude<CliCommand, { kind: "run" | "help" }>): Promise<void> {
  switch (command.kind) {
    case "create-runtime":
      process.stdout.write(JSON.stringify(await createRuntime()))
      return
    case "cleanup-runtime":
      await cleanupRuntime(command.runtimeDir)
      return
    case "replace-file":
      await replaceFile(command.source, command.destination)
      return
  }
}

export const HELP = `wezterm-ssh-manager OpenTUI

Usage:
  sshmgr-tui --snapshot <runtime/snapshot.json>
  sshmgr-tui --create-runtime
  sshmgr-tui --cleanup-runtime <runtime-dir>
  sshmgr-tui --replace-file <source> <destination>
`
