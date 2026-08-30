import { describe, expect, test } from "bun:test"
import {
  ALL_GROUPS,
  draftFromProfile,
  draftFromTarget,
  groupSummaries,
  normalizeSnapshot,
  parseTarget,
  profileTarget,
  rawFromDraft,
  visibleProfiles,
} from "../src/model.ts"

const snapshot = normalizeSnapshot({
  store_path: "/tmp/profiles.lua",
  default_where: "tab",
  groups: ["prod"],
  profiles: [
    {
      id: "prod/db",
      name: "数据库",
      group: "prod",
      editable: true,
      host: "192.0.2.8",
      user: "ops",
      port: 2222,
      has_password: true,
      sftp: { host: "files.example.com", user: "ops", privateKeys: ["~/.ssh/id_ed25519"] },
      raw: { name: "数据库", group: "prod", options: { host: "192.0.2.8", user: "ops", port: 2222 } },
    },
    { id: "lab", name: "实验机", group: "lab", editable: false, host: "lab.example.com", port: 22 },
  ],
})

describe("snapshot model", () => {
  test("normalizes profiles and discovers groups", () => {
    expect(snapshot.storePath).toBe("/tmp/profiles.lua")
    expect(snapshot.groups).toEqual(["prod", "lab"])
    expect(snapshot.profiles[0]?.hasPassword).toBe(true)
    expect(snapshot.profiles[0]?.sftp).toEqual({
      host: "files.example.com",
      user: "ops",
      privateKeys: ["~/.ssh/id_ed25519"],
    })
    expect(groupSummaries(snapshot)).toEqual([
      { id: ALL_GROUPS, label: "全部", count: 2 },
      { id: "prod", label: "prod", count: 1 },
      { id: "lab", label: "lab", count: 1 },
    ])
  })

  test("filters all profile search fields case-insensitively", () => {
    expect(visibleProfiles(snapshot, "prod", "OPS").map((profile) => profile.id)).toEqual(["prod/db"])
    expect(visibleProfiles(snapshot, ALL_GROUPS, "实验").map((profile) => profile.id)).toEqual(["lab"])
    expect(visibleProfiles(snapshot, "prod", "missing")).toEqual([])
  })

  test("renders ssh targets", () => {
    expect(profileTarget(snapshot.profiles[0]!)).toBe("ops@192.0.2.8:2222")
    expect(profileTarget(snapshot.profiles[1]!)).toBe("lab.example.com")
  })
})

describe("profile editing", () => {
  test("parses host, user, port, and bracketed IPv6", () => {
    expect(parseTarget("ops@example.com:2200")).toEqual({ user: "ops", host: "example.com", port: 2200 })
    expect(parseTarget("root@[2001:db8::1]:2222")).toEqual({ user: "root", host: "2001:db8::1", port: 2222 })
  })

  test("creates a draft from quick target", () => {
    expect(draftFromTarget("ops@example.com:2200", "prod")).toMatchObject({
      name: "example.com",
      group: "prod",
      host: "example.com",
      user: "ops",
      port: "2200",
    })
  })

  test("preserves nested raw data and an existing password when left blank", () => {
    const draft = draftFromProfile(snapshot.profiles[0]!)
    draft.name = "DB"
    draft.password = ""
    const result = rawFromDraft(draft)
    expect(result.error).toBeUndefined()
    expect(result.raw).toEqual({
      name: "DB",
      group: "prod",
      options: { host: "192.0.2.8", user: "ops", port: 2222 },
    })
  })

  test("validates ports", () => {
    const draft = draftFromTarget("example.com")
    draft.port = "70000"
    expect(rawFromDraft(draft)).toEqual({ error: "端口需要是 1–65535 的整数" })
  })
})
