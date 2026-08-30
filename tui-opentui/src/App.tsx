import type { KeyEvent } from "@opentui/core"
import { useKeyboard, useTerminalDimensions } from "@opentui/solid"
import { homedir } from "node:os"
import { basename, dirname, join } from "node:path"
import { posix } from "node:path"
import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js"
import { SftpPage, type SftpEntryView, type SftpPanelView, type SftpSide } from "./components/SftpPage.tsx"
import {
  ALL_GROUPS,
  draftFromProfile,
  draftFromTarget,
  groupSummaries,
  profileTarget,
  rawFromDraft,
  visibleProfiles,
} from "./model.ts"
import { RequestProtocol } from "./protocol.ts"
import {
  connectSftp,
  connectionFromProfile,
  LocalFileProvider,
  SftpCredentialRequiredError,
  SftpManagerError,
  type FileEntry,
  type FileProvider,
  type ProfileConnectionOverrides,
  type SftpSession,
  type TransferDirection,
  type TransferProgress,
} from "./sftp/index.ts"
import { watchSnapshot } from "./snapshot.ts"
import { theme } from "./theme.ts"
import type { ManagerFocus, MainPage, Profile, ProfileDraft, Snapshot } from "./types.ts"

type Modal =
  | { kind: "filter" }
  | { kind: "quick" }
  | { kind: "edit"; draft: ProfileDraft; field: number }
  | { kind: "delete"; profile: Profile }
  | { kind: "sftp-credentials"; profile: Profile; password: string; jumpPassword: string; field: number }
  | { kind: "sftp-input"; action: "mkdir"; side: SftpSide; value: string }
  | { kind: "sftp-input"; action: "rename"; side: SftpSide; value: string; entry: SftpEntryView }
  | { kind: "sftp-delete"; side: SftpSide; entry: SftpEntryView }
  | { kind: "sftp-overwrite"; direction: TransferDirection; source: string; destination: string }
  | null

interface AppProps {
  snapshotPath: string
  initialSnapshot: Snapshot
  protocol: RequestProtocol
}

const FORM_FIELDS: Array<{ key: keyof ProfileDraft; label: string; placeholder?: string }> = [
  { key: "name", label: "名称" },
  { key: "group", label: "分组", placeholder: "例如 prod/database" },
  { key: "host", label: "主机" },
  { key: "port", label: "端口" },
  { key: "user", label: "用户名" },
  { key: "auth", label: "认证", placeholder: "agent / publicKey / password" },
  { key: "password", label: "密码", placeholder: "留空则不修改" },
  { key: "jumpHost", label: "跳板机", placeholder: "profile 或 user@host:port" },
]

const clamp = (value: number, length: number): number => Math.max(0, Math.min(value, Math.max(0, length - 1)))
const move = (value: number, delta: number, length: number): number => clamp(value + delta, length)

const initialPanel = (path: string): SftpPanelView => ({ path, entries: [], selected: 0, loading: false, error: "" })

const errorText = (error: unknown): string => error instanceof Error ? error.message : String(error)

const progressText = (progress: TransferProgress): string => {
  const verb = progress.direction === "upload" ? "上传" : "下载"
  const amount = progress.totalBytes === null
    ? `${progress.transferredBytes} B`
    : `${progress.transferredBytes}/${progress.totalBytes} B`
  const percent = progress.percent === null ? "" : ` ${progress.percent.toFixed(0)}%`
  const speed = progress.bytesPerSecond > 0 ? ` · ${(progress.bytesPerSecond / 1024).toFixed(1)} KiB/s` : ""
  return `${verb} ${basename(progress.source)} · ${amount}${percent}${speed}`
}

const profileConnectionFingerprint = (profile: Profile, profiles: Profile[]): string => {
  const mapped = connectionFromProfile(profile, profiles, { environment: {} })
  return JSON.stringify({ connection: mapped.connection, issues: mapped.issues })
}

export function App(props: AppProps) {
  const dimensions = useTerminalDimensions()
  const [snapshot, setSnapshot] = createSignal(props.initialSnapshot)
  const [page, setPage] = createSignal<MainPage>("manager")
  const [focus, setFocus] = createSignal<ManagerFocus>("hosts")
  const [groupIndex, setGroupIndex] = createSignal(0)
  const [hostIndex, setHostIndex] = createSignal(0)
  const [filter, setFilter] = createSignal("")
  const [quickTarget, setQuickTarget] = createSignal("")
  const [modal, setModal] = createSignal<Modal>(null)
  const [status, setStatus] = createSignal("就绪")
  const [sftpSide, setSftpSide] = createSignal<SftpSide>("local")
  const [localPanel, setLocalPanel] = createSignal<SftpPanelView>(initialPanel(homedir()))
  const [remotePanel, setRemotePanel] = createSignal<SftpPanelView>(initialPanel("/"))
  const [connectedProfile, setConnectedProfile] = createSignal<Profile | null>(null)
  const [sftpBusy, setSftpBusy] = createSignal(false)
  const [transferProgress, setTransferProgress] = createSignal<TransferProgress | null>(null)
  const localProvider = new LocalFileProvider()
  let sftpSession: SftpSession | null = null
  let connectionController: AbortController | null = null
  let operationController: AbortController | null = null
  let localReadSequence = 0
  let remoteReadSequence = 0
  let localReadController: AbortController | null = null
  let remoteReadController: AbortController | null = null
  let connectedFingerprint: string | null = null
  let lastHostMouseDown = { index: -1, at: 0 }

  const groups = createMemo(() => groupSummaries(snapshot()))
  const selectedGroup = createMemo(() => groups()[clamp(groupIndex(), groups().length)]?.id ?? ALL_GROUPS)
  const hosts = createMemo(() => visibleProfiles(snapshot(), selectedGroup(), filter()))
  const selectedHost = createMemo(() => hosts()[clamp(hostIndex(), hosts().length)])
  const showDetails = createMemo(() => dimensions().width >= 102)
  const hostPageSize = createMemo(() => Math.max(4, dimensions().height - 10))
  const visibleHostRows = createMemo(() => {
    const rows = hosts()
    const count = hostPageSize()
    const start = Math.max(0, Math.min(hostIndex() - Math.floor(count / 2), rows.length - count))
    return rows.slice(start, start + count).map((profile, offset) => ({ profile, index: start + offset }))
  })

  createEffect(() => setGroupIndex((index) => clamp(index, groups().length)))
  createEffect(() => setHostIndex((index) => clamp(index, hosts().length)))

  const stopWatching = watchSnapshot(
    props.snapshotPath,
    (next) => {
      setSnapshot(next)
      setStatus(`已同步 ${next.profiles.length} 台主机`)
    },
    (error) => setStatus(`snapshot：${error.message}`),
  )
  onCleanup(stopWatching)
  onCleanup(() => {
    connectionController?.abort("TUI closed")
    operationController?.abort("TUI closed")
    localReadController?.abort("TUI closed")
    remoteReadController?.abort("TUI closed")
    sftpSession?.close()
  })

  const emit = (request: Parameters<RequestProtocol["emit"]>[0], success: string) => {
    try {
      props.protocol.emit(request)
      setStatus(success)
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error))
    }
  }

  const connect = (where = snapshot().defaultWhere) => {
    const profile = selectedHost()
    if (!profile) return setStatus("请先选择一台主机")
    emit({ op: "connect", id: profile.id, where }, `正在连接 ${profile.name}…`)
  }

  const selectHostFromMouse = (index: number, profile: Profile) => {
    const now = Date.now()
    const isDoubleClick = lastHostMouseDown.index === index && now - lastHostMouseDown.at <= 450
    lastHostMouseDown = { index, at: now }
    setFocus("hosts")
    setHostIndex(index)
    if (isDoubleClick) {
      lastHostMouseDown = { index: -1, at: 0 }
      emit({ op: "connect", id: profile.id, where: snapshot().defaultWhere }, `正在连接 ${profile.name}…`)
    }
  }

  const openEdit = (profile?: Profile) => {
    if (profile && !profile.editable) {
      emit({ op: "copy_in", id: profile.id }, "已请求复制到可编辑配置")
      return
    }
    const initialGroup = selectedGroup() === ALL_GROUPS ? "" : selectedGroup()
    setModal({ kind: "edit", draft: draftFromProfile(profile, initialGroup), field: 0 })
  }

  const updateDraft = (key: keyof ProfileDraft, value: string) => {
    setModal((current) => {
      if (!current || current.kind !== "edit") return current
      return { ...current, draft: { ...current.draft, [key]: value } }
    })
  }

  const saveDraft = () => {
    const current = modal()
    if (!current || current.kind !== "edit") return
    const result = rawFromDraft(current.draft)
    if (!result.raw) return setStatus(result.error ?? "表单无效")
    emit({ op: "upsert", id: current.draft.originalId, raw: result.raw }, "已请求保存")
    setModal(null)
  }

  const setPanelSelected = (side: SftpSide, selected: number) => {
    const setter = side === "local" ? setLocalPanel : setRemotePanel
    setter((panel) => ({ ...panel, selected: clamp(selected, panel.entries.length) }))
  }

  const panelFor = (side: SftpSide): SftpPanelView => side === "local" ? localPanel() : remotePanel()

  const updatePanel = (side: SftpSide, update: (panel: SftpPanelView) => SftpPanelView) => {
    if (side === "local") setLocalPanel(update)
    else setRemotePanel(update)
  }

  const providerFor = (side: SftpSide): FileProvider | null =>
    side === "local" ? localProvider : sftpSession?.remote ?? null

  const viewEntries = (side: SftpSide, directory: string, entries: FileEntry[]): SftpEntryView[] => {
    const parent = side === "local" ? dirname(directory) : posix.dirname(directory)
    const views: SftpEntryView[] = entries.map((entry) => ({
      name: entry.name,
      path: entry.path,
      kind: entry.kind === "symlink" ? "link" : entry.kind,
      ...(entry.kind === "directory" ? {} : { size: entry.size }),
      ...(entry.modifiedAt
        ? { modified: entry.modifiedAt.toLocaleString(undefined, { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }) }
        : {}),
    }))
    if (parent !== directory) views.unshift({ name: "..", path: parent, kind: "directory" })
    return views
  }

  const readPanel = async (side: SftpSide, directory: string): Promise<boolean> => {
    const provider = providerFor(side)
    if (!provider) {
      if (side === "remote") updatePanel(side, (panel) => ({ ...panel, entries: [], loading: false, error: "尚未连接 SFTP" }))
      return false
    }
    const previousController = side === "local" ? localReadController : remoteReadController
    previousController?.abort("new directory read")
    const controller = new AbortController()
    if (side === "local") localReadController = controller
    else remoteReadController = controller
    const sequence = side === "local" ? ++localReadSequence : ++remoteReadSequence
    updatePanel(side, (panel) => ({ ...panel, path: directory, entries: [], selected: 0, loading: true, error: "" }))
    try {
      const entries = await provider.list(directory, { signal: controller.signal })
      if (sequence !== (side === "local" ? localReadSequence : remoteReadSequence)) return false
      updatePanel(side, () => ({
        path: directory,
        entries: viewEntries(side, directory, entries),
        selected: 0,
        loading: false,
        error: "",
      }))
      return true
    } catch (error) {
      if (controller.signal.aborted || sequence !== (side === "local" ? localReadSequence : remoteReadSequence)) return false
      updatePanel(side, (panel) => ({ ...panel, entries: [], selected: 0, loading: false, error: errorText(error) }))
      setStatus(errorText(error))
      return false
    } finally {
      if (side === "local" && localReadController === controller) localReadController = null
      if (side === "remote" && remoteReadController === controller) remoteReadController = null
    }
  }

  onMount(() => void readPanel("local", localPanel().path))

  const navigateSftp = (direction: -1 | 1) => {
    const panel = panelFor(sftpSide())
    setPanelSelected(sftpSide(), move(panel.selected, direction, panel.entries.length))
  }

  const selectedSftpEntry = (side = sftpSide()): SftpEntryView | undefined => {
    const panel = panelFor(side)
    const entry = panel.entries[panel.selected]
    return entry?.name === ".." ? undefined : entry
  }

  const openSftpEntry = async () => {
    const side = sftpSide()
    const panel = panelFor(side)
    const entry = panel.entries[panel.selected]
    if (!entry) return
    if (entry.kind !== "directory") return setStatus(`${entry.name}：按 ${side === "local" ? "u" : "d"} 传输`)
    if (await readPanel(side, entry.path)) {
      setStatus(`${side === "local" ? "本地" : "远端"}：${entry.path}`)
    }
  }

  const parentDirectory = async () => {
    const side = sftpSide()
    const current = panelFor(side).path
    const parent = side === "local" ? dirname(current) : posix.dirname(current)
    if (parent !== current) await readPanel(side, parent)
  }

  const joinPanelPath = (side: SftpSide, name: string): string =>
    side === "local" ? join(panelFor(side).path, name) : posix.join(panelFor(side).path, name)

  const closeSftpSession = () => {
    operationController?.abort("SFTP session changed")
    operationController = null
    remoteReadController?.abort("SFTP session changed")
    remoteReadController = null
    remoteReadSequence += 1
    sftpSession?.close()
    sftpSession = null
    connectedFingerprint = null
    setConnectedProfile(null)
    setSftpBusy(false)
    setTransferProgress(null)
  }

  createEffect(() => {
    const connected = connectedProfile()
    const profiles = snapshot().profiles
    if (!connected || !sftpSession || !connectedFingerprint) return
    const current = profiles.find((profile) => profile.id === connected.id)
    if (!current || profileConnectionFingerprint(current, profiles) !== connectedFingerprint) {
      closeSftpSession()
      setRemotePanel((panel) => ({ ...panel, entries: [], loading: false, error: "配置已更新，请重新连接" }))
      setStatus("当前 SFTP 配置已更新，旧连接已关闭")
    }
  })

  const establishSftp = async (profile: Profile, overrides: ProfileConnectionOverrides = {}): Promise<void> => {
    const profilesAtStart = snapshot().profiles
    const fingerprintAtStart = profileConnectionFingerprint(profile, profilesAtStart)
    const mapped = connectionFromProfile(profile, profilesAtStart, overrides)
    const unsupported = mapped.issues.filter((issue) => issue.severity === "unsupported")
    if (!mapped.supported) {
      setStatus(unsupported.map((issue) => issue.message).join("；"))
      return
    }

    connectionController?.abort("new SFTP connection")
    closeSftpSession()
    const controller = new AbortController()
    connectionController = controller
    setSftpBusy(true)
    setStatus(`正在建立 SFTP：${profileTarget(profile)}…`)
    setRemotePanel(initialPanel("/"))
    try {
      const session = await connectSftp(mapped.connection, { signal: controller.signal })
      if (controller.signal.aborted) {
        session.close()
        return
      }
      const remoteHome = await session.remoteHome({ signal: controller.signal }).catch((error) => {
        if (controller.signal.aborted) throw error
        return "/"
      })
      if (session.closed) throw new Error("SFTP transport closed during initialization")
      const current = snapshot().profiles.find((candidate) => candidate.id === profile.id)
      if (!current || profileConnectionFingerprint(current, snapshot().profiles) !== fingerprintAtStart) {
        session.close()
        setStatus("连接期间配置已更新，请重试")
        return
      }
      sftpSession = session
      connectedFingerprint = fingerprintAtStart
      setConnectedProfile(profile)
      session.onDisconnected((error) => {
        if (sftpSession !== session) return
        operationController?.abort("SFTP transport disconnected")
        operationController = null
        remoteReadController?.abort("SFTP transport disconnected")
        remoteReadController = null
        sftpSession = null
        connectedFingerprint = null
        setConnectedProfile(null)
        setSftpBusy(false)
        setTransferProgress(null)
        const message = error ? `SFTP 连接已断开：${error.message}` : "SFTP 连接已断开"
        setRemotePanel((panel) => ({ ...panel, entries: [], loading: false, error: message }))
        setStatus(message)
      })
      const warnings = mapped.issues.filter((issue) => issue.severity === "warning")
      setStatus(warnings.length > 0 ? `SFTP 已连接；${warnings.map((issue) => issue.message).join("；")}` : `SFTP 已连接：${profile.name}`)
      setRemotePanel(initialPanel(remoteHome))
      await readPanel("remote", remoteHome)
    } catch (error) {
      if (controller.signal.aborted) setStatus("SFTP 连接已取消")
      else if (error instanceof SftpCredentialRequiredError) {
        setStatus(`${error.role === "jump" ? "跳板机" : "目标主机"}需要认证信息`)
        setModal({ kind: "sftp-credentials", profile, password: "", jumpPassword: "", field: error.role === "jump" ? 1 : 0 })
      } else {
        setStatus(errorText(error))
        setRemotePanel((panel) => ({ ...panel, loading: false, error: errorText(error) }))
      }
    } finally {
      if (connectionController === controller) {
        connectionController = null
        setSftpBusy(false)
      }
    }
  }

  const openSftpWorkspace = () => {
    const profile = selectedHost()
    if (!profile) return setStatus("请先选择一台主机")
    setPage("sftp")
    const requestedFingerprint = profileConnectionFingerprint(profile, snapshot().profiles)
    if (
      sftpSession &&
      connectedProfile()?.id === profile.id &&
      connectedFingerprint === requestedFingerprint &&
      !sftpSession.closed
    ) {
      void readPanel("remote", remotePanel().path)
      return
    }
    const mapped = connectionFromProfile(profile, snapshot().profiles)
    const unsupported = mapped.issues.filter((issue) => issue.severity === "unsupported")
    if (unsupported.length > 0) {
      setStatus(unsupported.map((issue) => issue.message).join("；"))
      return
    }
    const needsTarget = mapped.issues.some(
      (issue) => issue.severity === "needs-input" && !issue.field.startsWith("jump."),
    )
    const needsJump = mapped.issues.some(
      (issue) => issue.severity === "needs-input" && issue.field.startsWith("jump."),
    )
    if (needsTarget || needsJump) {
      setModal({ kind: "sftp-credentials", profile, password: "", jumpPassword: "", field: needsTarget ? 0 : 1 })
      return
    }
    void establishSftp(profile)
  }

  const refreshSftp = () => {
    void readPanel("local", localPanel().path)
    if (sftpSession) void readPanel("remote", remotePanel().path)
  }

  const performTransfer = async (
    direction: TransferDirection,
    source: string,
    destination: string,
    overwrite = false,
  ): Promise<void> => {
    const session = sftpSession
    if (!session) {
      setStatus("请先连接 SFTP")
      return
    }
    if (sftpBusy()) {
      setStatus("已有 SFTP 操作正在进行")
      return
    }
    const controller = new AbortController()
    operationController = controller
    setSftpBusy(true)
    setTransferProgress(null)
    try {
      const options = {
        overwrite,
        preserveTimes: true,
        signal: controller.signal,
        onProgress: (progress: TransferProgress) => {
          setTransferProgress(progress)
          setStatus(progressText(progress))
        },
      }
      if (direction === "upload") await session.upload(source, destination, options)
      else await session.download(source, destination, options)
      setStatus(`${direction === "upload" ? "上传" : "下载"}完成：${basename(destination)}`)
      await Promise.all([readPanel("local", localPanel().path), readPanel("remote", remotePanel().path)])
    } catch (error) {
      if (controller.signal.aborted) setStatus("传输已取消")
      else if (error instanceof SftpManagerError && error.code === "DESTINATION_EXISTS" && !overwrite) {
        setModal({ kind: "sftp-overwrite", direction, source, destination })
        setStatus("目标已存在，请确认是否覆盖")
      } else setStatus(errorText(error))
    } finally {
      if (operationController === controller) {
        operationController = null
        setSftpBusy(false)
        setTransferProgress(null)
      }
    }
  }

  const uploadSelected = () => {
    const entry = selectedSftpEntry("local")
    if (!entry) return setStatus("请选择一个本地文件")
    if (entry.kind !== "file") return setStatus("首版仅传输普通文件，目录和符号链接请先打包")
    void performTransfer("upload", entry.path, posix.join(remotePanel().path, entry.name))
  }

  const downloadSelected = () => {
    const entry = selectedSftpEntry("remote")
    if (!entry) return setStatus("请选择一个远端文件")
    if (entry.kind !== "file") return setStatus("首版仅传输普通文件，目录和符号链接请先打包")
    void performTransfer("download", entry.path, join(localPanel().path, entry.name))
  }

  const mutatePanel = async (
    side: SftpSide,
    operation: (provider: FileProvider, signal: AbortSignal) => Promise<void>,
    success: string,
  ): Promise<void> => {
    const provider = providerFor(side)
    if (!provider) {
      setStatus("请先连接 SFTP")
      return
    }
    if (sftpBusy()) {
      setStatus("已有 SFTP 操作正在进行")
      return
    }
    const controller = new AbortController()
    operationController = controller
    setSftpBusy(true)
    try {
      await operation(provider, controller.signal)
      setStatus(success)
      await readPanel(side, panelFor(side).path)
    } catch (error) {
      setStatus(controller.signal.aborted ? "操作已取消" : errorText(error))
    } finally {
      if (operationController === controller) {
        operationController = null
        setSftpBusy(false)
      }
    }
  }

  const submitSftpInput = (current: Extract<NonNullable<Modal>, { kind: "sftp-input" }>) => {
    const value = current.value.trim()
    if (!value) return setStatus("名称不能为空")
    setModal(null)
    if (current.action === "mkdir") {
      const path = joinPanelPath(current.side, value)
      void mutatePanel(current.side, (provider, signal) => provider.mkdir(path, { signal }), `已创建目录 ${value}`)
      return
    }
    const entry = current.entry
    const destination = current.side === "local"
      ? join(dirname(entry.path), value)
      : posix.join(posix.dirname(entry.path), value)
    void mutatePanel(
      current.side,
      (provider, signal) => provider.rename(entry.path, destination, { signal }),
      `已改名为 ${value}`,
    )
  }

  const submitSftpCredentials = (
    current: Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>,
  ) => {
    setModal(null)
    const overrides: ProfileConnectionOverrides = {
      ...(current.password ? { password: current.password } : {}),
      ...(current.jumpPassword ? { jump: { password: current.jumpPassword } } : {}),
    }
    void establishSftp(current.profile, overrides)
  }

  const confirmSftpDelete = (current: Extract<NonNullable<Modal>, { kind: "sftp-delete" }>) => {
    setModal(null)
    void mutatePanel(
      current.side,
      (provider, signal) => provider.remove(current.entry.path, { signal, recursive: true }),
      `已删除 ${current.entry.name}`,
    )
  }

  const cancelSftpOperation = () => {
    if (connectionController) connectionController.abort("cancelled by user")
    else if (operationController) operationController.abort("cancelled by user")
    else return setStatus("当前没有可取消的操作")
    setStatus("正在取消…")
  }

  const handleModalKey = (key: KeyEvent, current: NonNullable<Modal>): boolean => {
    if (key.name === "escape") {
      key.preventDefault()
      setModal(null)
      return true
    }
    if (current.kind === "delete" || current.kind === "sftp-delete" || current.kind === "sftp-overwrite") {
      if (key.name === "y" || key.name === "enter") {
        key.preventDefault()
        if (current.kind === "delete") {
          emit({ op: "delete", id: current.profile.id }, `已请求删除 ${current.profile.name}`)
          setModal(null)
        } else if (current.kind === "sftp-delete") {
          confirmSftpDelete(current)
        } else {
          setModal(null)
          void performTransfer(current.direction, current.source, current.destination, true)
        }
      } else if (key.name === "n") {
        key.preventDefault()
        setModal(null)
      }
      return true
    }
    if (current.kind === "edit") {
      if (key.ctrl && key.name === "s") {
        key.preventDefault()
        saveDraft()
        return true
      }
      if (key.name === "tab") {
        key.preventDefault()
        const delta = key.shift ? -1 : 1
        setModal({ ...current, field: move(current.field, delta, FORM_FIELDS.length) })
        return true
      }
      return false
    }
    if (current.kind === "sftp-credentials" && key.name === "tab") {
      key.preventDefault()
      setModal({ ...current, field: current.field === 0 ? 1 : 0 })
      return true
    }
    return false
  }

  useKeyboard((key) => {
    const currentModal = modal()
    if (currentModal && handleModalKey(key, currentModal)) return
    if (currentModal) return

    if (page() === "sftp") {
      if (key.name === "escape" || key.name === "1") {
        key.preventDefault()
        setPage("manager")
      } else if (key.name === "tab") {
        key.preventDefault()
        setSftpSide((side) => (side === "local" ? "remote" : "local"))
      } else if (key.name === "up" || key.name === "k") {
        key.preventDefault()
        navigateSftp(-1)
      } else if (key.name === "down" || key.name === "j") {
        key.preventDefault()
        navigateSftp(1)
      } else if (key.name === "enter") {
        key.preventDefault()
        void openSftpEntry()
      } else if (key.name === "backspace") {
        key.preventDefault()
        void parentDirectory()
      } else if (key.name === "u") {
        key.preventDefault()
        uploadSelected()
      } else if (key.name === "d") {
        key.preventDefault()
        downloadSelected()
      } else if (key.name === "m") {
        key.preventDefault()
        setModal({ kind: "sftp-input", action: "mkdir", side: sftpSide(), value: "" })
      } else if (key.name === "r") {
        key.preventDefault()
        const entry = selectedSftpEntry()
        if (entry) setModal({ kind: "sftp-input", action: "rename", side: sftpSide(), value: entry.name, entry })
        else setStatus("请选择要改名的项目")
      } else if (key.name === "x" || key.name === "delete") {
        key.preventDefault()
        const entry = selectedSftpEntry()
        if (entry) setModal({ kind: "sftp-delete", side: sftpSide(), entry })
        else setStatus("请选择要删除的项目")
      } else if (key.name === "f5") {
        key.preventDefault()
        refreshSftp()
      } else if (key.name === "c") {
        key.preventDefault()
        cancelSftpOperation()
      }
      return
    }

    if (key.name === "2" || key.name === "s") {
      key.preventDefault()
      openSftpWorkspace()
      return
    }
    if (key.name === "tab") {
      key.preventDefault()
      setFocus((current) => (current === "groups" ? "hosts" : current === "hosts" && showDetails() ? "details" : "groups"))
    } else if (key.name === "up" || key.name === "k") {
      key.preventDefault()
      if (focus() === "groups") setGroupIndex((index) => move(index, -1, groups().length))
      else setHostIndex((index) => move(index, -1, hosts().length))
    } else if (key.name === "down" || key.name === "j") {
      key.preventDefault()
      if (focus() === "groups") setGroupIndex((index) => move(index, 1, groups().length))
      else setHostIndex((index) => move(index, 1, hosts().length))
    } else if (key.name === "enter" || key.name === "space" || key.name === " ") {
      key.preventDefault()
      connect(key.ctrl ? "window" : snapshot().defaultWhere)
    } else if (key.name === "/") {
      key.preventDefault()
      setModal({ kind: "filter" })
    } else if (key.name === "p") {
      key.preventDefault()
      setQuickTarget("")
      setModal({ kind: "quick" })
    } else if (key.name === "n") {
      key.preventDefault()
      openEdit()
    } else if (key.name === "e") {
      key.preventDefault()
      openEdit(selectedHost())
    } else if (key.name === "d") {
      key.preventDefault()
      const profile = selectedHost()
      if (!profile) setStatus("请先选择一台主机")
      else if (!profile.editable) setStatus("只读连接不能删除")
      else setModal({ kind: "delete", profile })
    } else if (key.name === "r") {
      key.preventDefault()
      emit({ op: "reload" }, "已请求刷新")
    } else if (key.name === "q" || key.name === "escape") {
      key.preventDefault()
      emit({ op: "hide" }, "正在返回…")
    }
  })

  const selectedBg = (active: boolean) => (active ? theme.selected : theme.surface)
  const selectedFg = (active: boolean) => (active ? theme.blue : theme.text)
  const modalTitle = (current: NonNullable<Modal>): string => {
    switch (current.kind) {
      case "edit": return "编辑连接"
      case "delete": return "确认删除"
      case "filter": return "过滤"
      case "quick": return "快捷连接"
      case "sftp-credentials": return `SFTP 认证 · ${current.profile.name}`
      case "sftp-input": return current.action === "mkdir" ? "新建目录" : "改名"
      case "sftp-delete": return "确认删除文件"
      case "sftp-overwrite": return "确认覆盖"
    }
  }

  return (
    <box width="100%" height="100%" flexDirection="column" backgroundColor={theme.bg}>
      <box height={3} flexDirection="row" alignItems="center" paddingX={1} backgroundColor={theme.surface}>
        <text fg={theme.blue}>◆ SSH Manager</text>
        <text fg={page() === "manager" ? theme.text : theme.muted}>  [1] 主机</text>
        <text fg={page() === "sftp" ? theme.text : theme.muted}>  [2] SFTP</text>
        <text flexGrow={1} />
        <text fg={theme.muted}>{status()}</text>
      </box>

      <Show
        when={page() === "manager"}
        fallback={
          <SftpPage
            active={sftpSide()}
            local={localPanel()}
            remote={remotePanel()}
            connected={Boolean(connectedProfile())}
            busy={sftpBusy()}
            progress={transferProgress() ? progressText(transferProgress()!) : undefined}
            hostLabel={connectedProfile() ? profileTarget(connectedProfile()!) : undefined}
            onActivate={setSftpSide}
            onSelect={setPanelSelected}
          />
        }
      >
        <box flexGrow={1} flexDirection="row" gap={1} padding={1}>
          <box
            width={24}
            flexDirection="column"
            border
            borderStyle="rounded"
            borderColor={focus() === "groups" ? theme.blue : theme.border}
            title="分组"
            backgroundColor={theme.surface}
          >
            <For each={groups()}>
              {(group, index) => (
                <box
                  height={1}
                  flexDirection="row"
                  paddingX={1}
                  backgroundColor={selectedBg(index() === groupIndex())}
                  onMouseDown={() => {
                    setFocus("groups")
                    setGroupIndex(index())
                    setHostIndex(0)
                  }}
                >
                  <text width={17} fg={selectedFg(index() === groupIndex())} truncate>
                    {`${index() === groupIndex() ? "›" : " "} ${group.label}`}
                  </text>
                  <text fg={theme.muted}>{String(group.count).padStart(3)}</text>
                </box>
              )}
            </For>
          </box>

          <box
            flexGrow={1}
            flexDirection="column"
            border
            borderStyle="rounded"
            borderColor={focus() === "hosts" ? theme.blue : theme.border}
            title={`主机 · ${hosts().length}${filter() ? ` · “${filter()}”` : ""}`}
            backgroundColor={theme.surface}
          >
            <box height={1} flexDirection="row" paddingX={1} backgroundColor={theme.elevated}>
              <text width="45%" fg={theme.muted}>名称</text>
              <text width="42%" fg={theme.muted}>目标</text>
              <text flexGrow={1} fg={theme.muted}>状态</text>
            </box>
            <For each={visibleHostRows()}>
              {(row) => {
                const selected = () => row.index === hostIndex()
                return (
                  <box
                    height={1}
                    flexDirection="row"
                    paddingX={1}
                    backgroundColor={selectedBg(selected())}
                    onMouseDown={() => {
                      selectHostFromMouse(row.index, row.profile)
                    }}
                  >
                    <text width="45%" fg={selectedFg(selected())} truncate>
                      {`${selected() ? "›" : " "} ${selectedGroup() === ALL_GROUPS && row.profile.group ? `${row.profile.group}/` : ""}${row.profile.icon ? `${row.profile.icon} ` : ""}${row.profile.name}`}
                    </text>
                    <text width="42%" fg={theme.subtext} truncate>{profileTarget(row.profile)}</text>
                    <text flexGrow={1} fg={row.profile.editable ? theme.green : theme.yellow} truncate>
                      {row.profile.editable ? (row.profile.jumpHost ? "跳板" : "") : "只读"}
                    </text>
                  </box>
                )
              }}
            </For>
            <Show when={hosts().length === 0}>
              <text fg={theme.muted}> 没有匹配的主机，按 / 修改过滤条件</text>
            </Show>
          </box>

          <Show when={showDetails()}>
            <box
              width={34}
              flexDirection="column"
              border
              borderStyle="rounded"
              borderColor={focus() === "details" ? theme.blue : theme.border}
              title="详情"
              padding={1}
              backgroundColor={theme.surface}
              onMouseDown={() => setFocus("details")}
            >
              <Show when={selectedHost()} fallback={<text fg={theme.muted}>未选择主机</text>}>
                {(profile) => (
                  <>
                    <text fg={theme.blue}>{profile().name}</text>
                    <text fg={theme.text}>{profileTarget(profile())}</text>
                    <text fg={theme.muted}>{profile().group ? `分组  ${profile().group}` : "未分组"}</text>
                    <text fg={theme.muted}>{profile().auth ? `认证  ${profile().auth}` : "认证  自动"}</text>
                    <text fg={theme.muted}>{profile().jumpHost ? `跳板  ${profile().jumpHost}` : "跳板  无"}</text>
                    <text fg={profile().editable ? theme.green : theme.yellow}>
                      {profile().editable ? "可编辑配置" : "导入的只读配置"}
                    </text>
                    <box height={1} />
                    <text fg={theme.subtext}>Enter 连接</text>
                    <text fg={theme.subtext}>e 编辑 · s SFTP</text>
                  </>
                )}
              </Show>
            </box>
          </Show>
        </box>
        <box height={2} paddingX={1} alignItems="center" backgroundColor={theme.surface}>
          <text fg={theme.muted}>Tab 切区  ↑↓/jk 选择  Enter 连接  n 新建  e 编辑  d 删除  / 过滤  p 快连  r 刷新  s SFTP  q 返回</text>
        </box>
      </Show>

      <Show when={modal()}>
        {(current) => (
          <box
            position="absolute"
            top={0}
            left={0}
            width="100%"
            height="100%"
            zIndex={99}
            alignItems="center"
            justifyContent="center"
            onMouseDown={(event) => {
              event.stopPropagation()
              event.preventDefault()
            }}
          >
            <box
              width="64%"
              height={current().kind === "edit" ? 22 : current().kind === "sftp-credentials" ? 11 : 7}
              zIndex={100}
              flexDirection="column"
              border
              borderStyle="rounded"
              borderColor={["delete", "sftp-delete", "sftp-overwrite"].includes(current().kind) ? theme.red : theme.blue}
              title={modalTitle(current())}
              padding={1}
              backgroundColor={theme.overlay}
              onMouseDown={(event) => event.stopPropagation()}
            >
            <Show when={current().kind === "filter"}>
              <text fg={theme.subtext}>名称、分组、主机、用户或跳板机</text>
              <input
                value={filter()}
                focused
                placeholder="输入关键字"
                backgroundColor={theme.surface}
                focusedBackgroundColor={theme.surface}
                textColor={theme.text}
                focusedTextColor={theme.text}
                onInput={setFilter}
                onSubmit={() => setModal(null)}
              />
            </Show>
            <Show when={current().kind === "quick"}>
              <text fg={theme.subtext}>输入 [user@]host[:port]</text>
              <input
                focused
                placeholder="ops@example.com:22"
                backgroundColor={theme.surface}
                focusedBackgroundColor={theme.surface}
                textColor={theme.text}
                focusedTextColor={theme.text}
                onInput={setQuickTarget}
                onSubmit={() => {
                  const target = quickTarget()
                  if (target.trim()) emit({ op: "quick", target: target.trim(), where: snapshot().defaultWhere }, `正在连接 ${target.trim()}…`)
                  setModal(null)
                }}
              />
            </Show>
            <Show when={current().kind === "delete"}>
              <text fg={theme.text}>{`确定删除「${current().kind === "delete" ? (current() as Extract<NonNullable<Modal>, { kind: "delete" }>).profile.name : ""}」？`}</text>
              <text fg={theme.muted}>Enter / y 确定，n / Esc 取消</text>
            </Show>
            <Show when={current().kind === "sftp-credentials"}>
              <text fg={theme.subtext}>snapshot 不携带已保存密码；密钥和 Agent 会自动使用。</text>
              <box height={2} flexDirection="row" alignItems="center">
                <text width={14} fg={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).field === 0 ? theme.blue : theme.subtext}>目标密码</text>
                <input
                  flexGrow={1}
                  value={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).password}
                  focused={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).field === 0}
                  placeholder="不需要则留空"
                  backgroundColor={theme.surface}
                  focusedBackgroundColor={theme.elevated}
                  textColor={theme.text}
                  focusedTextColor={theme.text}
                  onInput={(value) => setModal((item) => item?.kind === "sftp-credentials" ? { ...item, password: value } : item)}
                  onSubmit={() => setModal((item) => item?.kind === "sftp-credentials" ? { ...item, field: 1 } : item)}
                />
              </box>
              <box height={2} flexDirection="row" alignItems="center">
                <text width={14} fg={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).field === 1 ? theme.blue : theme.subtext}>跳板机密码</text>
                <input
                  flexGrow={1}
                  value={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).jumpPassword}
                  focused={(current() as Extract<NonNullable<Modal>, { kind: "sftp-credentials" }>).field === 1}
                  placeholder="不需要则留空"
                  backgroundColor={theme.surface}
                  focusedBackgroundColor={theme.elevated}
                  textColor={theme.text}
                  focusedTextColor={theme.text}
                  onInput={(value) => setModal((item) => item?.kind === "sftp-credentials" ? { ...item, jumpPassword: value } : item)}
                  onSubmit={() => {
                    const value = current()
                    if (value.kind === "sftp-credentials") submitSftpCredentials(value)
                  }}
                />
              </box>
              <text fg={theme.muted}>Tab 切字段 · 在第二项按 Enter 连接 · Esc 取消</text>
            </Show>
            <Show when={current().kind === "sftp-input"}>
              <text fg={theme.subtext}>{(current() as Extract<NonNullable<Modal>, { kind: "sftp-input" }>).action === "mkdir" ? "输入新目录名称" : "输入新名称"}</text>
              <input
                focused
                value={(current() as Extract<NonNullable<Modal>, { kind: "sftp-input" }>).value}
                backgroundColor={theme.surface}
                focusedBackgroundColor={theme.elevated}
                textColor={theme.text}
                focusedTextColor={theme.text}
                onInput={(value) => setModal((item) => item?.kind === "sftp-input" ? { ...item, value } : item)}
                onSubmit={() => {
                  const value = current()
                  if (value.kind === "sftp-input") submitSftpInput(value)
                }}
              />
            </Show>
            <Show when={current().kind === "sftp-delete"}>
              <text fg={theme.text}>{`递归删除「${(current() as Extract<NonNullable<Modal>, { kind: "sftp-delete" }>).entry.name}」？`}</text>
              <text fg={theme.muted}>Enter / y 确定，n / Esc 取消</text>
            </Show>
            <Show when={current().kind === "sftp-overwrite"}>
              <text fg={theme.text}>{`目标「${basename((current() as Extract<NonNullable<Modal>, { kind: "sftp-overwrite" }>).destination)}」已存在，覆盖吗？`}</text>
              <text fg={theme.muted}>Enter / y 覆盖，n / Esc 取消</text>
            </Show>
            <Show when={current().kind === "edit"}>
              <For each={FORM_FIELDS}>
                {(field, index) => {
                  const edit = () => {
                    const value = current()
                    return value.kind === "edit" ? value : null
                  }
                  return (
                    <box height={2} flexDirection="row" alignItems="center">
                      <text width={12} fg={index() === edit()?.field ? theme.blue : theme.subtext}>{field.label}</text>
                      <input
                        flexGrow={1}
                        value={String(edit()?.draft[field.key] ?? "")}
                        focused={index() === edit()?.field}
                        placeholder={field.placeholder ?? ""}
                        backgroundColor={theme.surface}
                        focusedBackgroundColor={theme.elevated}
                        textColor={theme.text}
                        focusedTextColor={theme.text}
                        onInput={(value) => updateDraft(field.key, value)}
                        onSubmit={() => {
                          const value = edit()
                          if (!value) return
                          if (value.field === FORM_FIELDS.length - 1) saveDraft()
                          else setModal({ ...value, field: value.field + 1 })
                        }}
                        onMouseDown={() => {
                          const value = edit()
                          if (value) setModal({ ...value, field: index() })
                        }}
                      />
                    </box>
                  )
                }}
              </For>
              <text fg={theme.muted}>Tab / Shift+Tab 切换字段 · Ctrl+S 保存 · Esc 取消</text>
            </Show>
            </box>
          </box>
        )}
      </Show>
    </box>
  )
}
