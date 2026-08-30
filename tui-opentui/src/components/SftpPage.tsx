import { For } from "solid-js"
import { useTerminalDimensions } from "@opentui/solid"
import { theme } from "../theme.ts"

export type SftpSide = "local" | "remote"

export interface SftpEntryView {
  name: string
  path: string
  kind: "directory" | "file" | "link" | "other"
  size?: number
  modified?: string
}

export interface SftpPanelView {
  path: string
  entries: SftpEntryView[]
  selected: number
  loading?: boolean
  error?: string
}

export interface SftpPageProps {
  active: SftpSide
  local: SftpPanelView
  remote: SftpPanelView
  connected?: boolean
  hostLabel?: string | undefined
  busy?: boolean | undefined
  progress?: string | undefined
  onActivate?: ((side: SftpSide) => void) | undefined
  onSelect?: ((side: SftpSide, index: number) => void) | undefined
}

const humanSize = (size?: number): string => {
  if (size === undefined) return ""
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} K`
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} M`
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} G`
}

function FilePanel(props: {
  side: SftpSide
  title: string
  panel: SftpPanelView
  active: boolean
  onActivate?: ((side: SftpSide) => void) | undefined
  onSelect?: ((side: SftpSide, index: number) => void) | undefined
}) {
  const dimensions = useTerminalDimensions()
  const visibleEntries = () => {
    const count = Math.max(3, dimensions().height - 10)
    const start = Math.max(
      0,
      Math.min(props.panel.selected - Math.floor(count / 2), Math.max(0, props.panel.entries.length - count)),
    )
    return props.panel.entries.slice(start, start + count).map((entry, offset) => ({ entry, index: start + offset }))
  }
  return (
    <box
      flexGrow={1}
      width="50%"
      height="100%"
      flexDirection="column"
      border
      borderStyle="rounded"
      borderColor={props.active ? theme.blue : theme.border}
      title={`${props.active ? "●" : "○"} ${props.title}`}
      titleColor={props.active ? theme.blue : theme.subtext}
      backgroundColor={theme.surface}
      onMouseDown={() => props.onActivate?.(props.side)}
    >
      <text fg={theme.lavender} bg={theme.elevated} height={1} truncate>
        {` ${props.panel.path}`}
      </text>
      <box height={1} flexDirection="row" paddingX={1}>
        <text fg={theme.muted} width="65%">名称</text>
        <text fg={theme.muted} width="18%">大小</text>
        <text fg={theme.muted} flexGrow={1}>修改时间</text>
      </box>
      <box flexGrow={1} flexDirection="column" overflow="hidden">
        <For each={visibleEntries()}>
          {(row) => {
            const selected = () => row.index === props.panel.selected
            const marker = () => (row.entry.kind === "directory" ? "▸" : row.entry.kind === "link" ? "↗" : " ")
            return (
              <box
                height={1}
                flexDirection="row"
                paddingX={1}
                backgroundColor={selected() ? theme.selected : theme.surface}
                onMouseDown={() => {
                  props.onActivate?.(props.side)
                  props.onSelect?.(props.side, row.index)
                }}
              >
                <text fg={selected() ? theme.blue : theme.text} width="65%" truncate>
                  {`${selected() ? "›" : " "} ${marker()} ${row.entry.name}`}
                </text>
                <text fg={theme.subtext} width="18%">{row.entry.kind === "directory" ? "<DIR>" : humanSize(row.entry.size)}</text>
                <text fg={theme.muted} flexGrow={1} truncate>{row.entry.modified ?? ""}</text>
              </box>
            )
          }}
        </For>
        {props.panel.loading ? <text fg={theme.yellow}> 正在读取目录…</text> : null}
        {props.panel.error ? <text fg={theme.red}>{` ${props.panel.error}`}</text> : null}
        {!props.panel.loading && !props.panel.error && props.panel.entries.length === 0 ? (
          <text fg={theme.muted}> （空目录）</text>
        ) : null}
      </box>
    </box>
  )
}

export function SftpPage(props: SftpPageProps) {
  return (
    <box flexGrow={1} flexDirection="column" backgroundColor={theme.bg}>
      <box height={2} paddingX={1} flexDirection="row" alignItems="center">
        <text fg={props.connected ? theme.green : props.busy ? theme.yellow : theme.muted}>
          {props.connected ? "● 已连接" : props.busy ? "◌ 正在处理" : "○ 尚未连接"}
        </text>
        <text fg={theme.subtext}>{props.hostLabel ? `  ${props.hostLabel}` : "  从 SSH Manager 选择主机后按 s"}</text>
        <text flexGrow={1} />
        <text fg={theme.yellow}>{props.progress ?? ""}</text>
      </box>
      <box flexGrow={1} flexDirection="row" gap={1} paddingX={1}>
        <FilePanel
          side="local"
          title="本地"
          panel={props.local}
          active={props.active === "local"}
          onActivate={props.onActivate}
          onSelect={props.onSelect}
        />
        <FilePanel
          side="remote"
          title="远端"
          panel={props.remote}
          active={props.active === "remote"}
          onActivate={props.onActivate}
          onSelect={props.onSelect}
        />
      </box>
      <box height={2} paddingX={1} alignItems="center">
        <text fg={theme.muted}>Tab 切栏  ↑↓/jk 选择  Enter 打开  Backspace 上级  u 上传  d 下载  m 新建目录  r 改名  x 删除  F5 刷新  c 取消  Esc 返回</text>
      </box>
    </box>
  )
}
