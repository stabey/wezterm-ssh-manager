import { render } from "@opentui/solid"
import { App } from "./App.tsx"
import { HELP, parseArgs, runHelper } from "./cli.ts"
import { RequestProtocol } from "./protocol.ts"
import { cleanupRuntime, validateRuntime } from "./runtime.ts"
import { readSnapshot } from "./snapshot.ts"
import { theme } from "./theme.ts"

async function main(): Promise<number> {
  const command = parseArgs(process.argv.slice(2))
  if (command.kind === "help") {
    process.stdout.write(HELP)
    return 2
  }
  if (command.kind !== "run") {
    await runHelper(command)
    return 0
  }

  const token = process.env.WEZTERM_SSHMGR_SESSION_TOKEN ?? ""
  delete process.env.WEZTERM_SSHMGR_SESSION_TOKEN
  const runtimeDir = await validateRuntime(command.snapshotPath, token)
  const snapshot = await readSnapshot(command.snapshotPath)
  const protocol = new RequestProtocol(runtimeDir, token)

  try {
    await new Promise<void>((resolve, reject) => {
      render(() => <App snapshotPath={command.snapshotPath} initialSnapshot={snapshot} protocol={protocol} />, {
        backgroundColor: theme.bg,
        useMouse: true,
        enableMouseMovement: true,
        exitOnCtrlC: true,
        clearOnShutdown: true,
        onDestroy: resolve,
      }).catch(reject)
    })
  } finally {
    await cleanupRuntime(runtimeDir)
  }
  return 0
}

try {
  process.exitCode = await main()
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
  process.exitCode = 2
}
