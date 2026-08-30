import solidPlugin from "@opentui/solid/bun-plugin"
import { mkdir } from "node:fs/promises"
import { join } from "node:path"

type TargetName = "macos-arm64" | "macos-x64" | "windows-x64"

const targets: Record<TargetName, { bun: Bun.Build.Target; filename: string }> = {
  "macos-arm64": { bun: "bun-darwin-arm64", filename: "sshmgr-tui-macos-arm64" },
  "macos-x64": { bun: "bun-darwin-x64", filename: "sshmgr-tui-macos-x64" },
  "windows-x64": { bun: "bun-windows-x64", filename: "sshmgr-tui-windows-x64.exe" },
}

const currentTarget = (): TargetName => {
  if (process.platform === "darwin" && process.arch === "arm64") return "macos-arm64"
  if (process.platform === "darwin") return "macos-x64"
  if (process.platform === "win32") return "windows-x64"
  throw new Error(`当前构建脚本尚未声明 ${process.platform}/${process.arch} 的产物名`)
}

const requestedIndex = process.argv.indexOf("--target")
const requested = requestedIndex >= 0 ? process.argv[requestedIndex + 1] : undefined
const names: TargetName[] =
  !requested || requested === "current"
    ? [currentTarget()]
    : requested === "all"
      ? ["macos-arm64", "macos-x64", "windows-x64"]
      : requested in targets
        ? [requested as TargetName]
        : (() => {
            throw new Error(`未知 target：${requested}`)
          })()

await mkdir("dist", { recursive: true })

for (const name of names) {
  const target = targets[name]
  const outfile = join("dist", target.filename)
  process.stdout.write(`building ${name} -> ${outfile}\n`)
  const result = await Bun.build({
    entrypoints: ["src/index.tsx"],
    target: "bun",
    plugins: [solidPlugin],
    // ssh2 treats cpu-features as an optional accelerator and catches a
    // missing module. Its package contains a native require that cannot be
    // embedded in Bun's single-file executable, so leave that one optional.
    external: ["cpu-features"],
    minify: true,
    env: "disable",
    compile: {
      target: target.bun,
      outfile,
      autoloadDotenv: false,
      autoloadBunfig: false,
      ...(name === "windows-x64" ? { windows: { title: "SSH Manager" } } : {}),
    },
  })
  if (!result.success) {
    for (const log of result.logs) process.stderr.write(`${log}\n`)
    throw new Error(`${name} 构建失败`)
  }
}
