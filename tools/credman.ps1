<#
.SYNOPSIS
  Read a password out of the Windows Credential Manager, for wezterm-ssh-manager.

.DESCRIPTION
  Tabby stores SSH passwords with keytar, which writes a CRED_TYPE_GENERIC
  credential whose target name is "<service>/<account>", where the service is
  "ssh@<host>" or "ssh@<host>:<port>" and the account is the username. So the
  password for deploy@203.0.113.23:22 lives under the target:

      ssh@203.0.113.23:22/deploy

  keytar stores the CredentialBlob as the raw UTF-8 bytes of the password --
  not UTF-16 as most Windows software does -- so this script decodes it as
  UTF-8. Decoding it as Unicode would return mojibake.

  The password is written to the success pipeline (Write-Output). Callers
  must strip a trailing newline. Do not use [Console]::Out -- wezterm's
  run_child_process only captures the pipe.

.PARAMETER Target
  Full credential target name to read.

.PARAMETER Host_
  Alternative to -Target: build the target from host/port/user.

.PARAMETER List
  List the matching credentials (target and username only, never passwords).

.EXAMPLE
  .\credman.ps1 -List
  .\credman.ps1 -Target 'ssh@203.0.113.23:22/deploy'
  .\credman.ps1 -Host_ 203.0.113.23 -Port 22 -User deploy
#>
[CmdletBinding(DefaultParameterSetName = 'Target')]
param(
  [Parameter(ParameterSetName = 'Target', Position = 0)]
  [string] $Target,

  [Parameter(ParameterSetName = 'Parts', Mandatory = $true)]
  [string] $Host_,
  [Parameter(ParameterSetName = 'Parts')]
  [int] $Port = 22,
  [Parameter(ParameterSetName = 'Parts', Mandatory = $true)]
  [string] $User,

  [Parameter(ParameterSetName = 'List', Mandatory = $true)]
  [switch] $List,
  [Parameter(ParameterSetName = 'List')]
  [string] $Filter = 'ssh@*',
  [Parameter(ParameterSetName = 'List')]
  [switch] $NamesOnly
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace WSM -Name Cred -MemberDefinition @'
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct CREDENTIAL {
    public uint Flags;
    public uint Type;
    public IntPtr TargetName;
    public IntPtr Comment;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
    public uint CredentialBlobSize;
    public IntPtr CredentialBlob;
    public uint Persist;
    public uint AttributeCount;
    public IntPtr Attributes;
    public IntPtr TargetAlias;
    public IntPtr UserName;
}

[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CredReadW")]
public static extern bool CredRead(string target, uint type, uint flags, out IntPtr credential);

[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CredEnumerateW")]
public static extern bool CredEnumerate(string filter, uint flags, out uint count, out IntPtr credentials);

[DllImport("advapi32.dll", EntryPoint = "CredFree")]
public static extern void CredFree(IntPtr buffer);
'@

function Read-Blob([IntPtr] $ptr) {
  $c = [System.Runtime.InteropServices.Marshal]::PtrToStructure($ptr, [type][WSM.Cred+CREDENTIAL])
  if ($c.CredentialBlobSize -eq 0 -or $c.CredentialBlob -eq [IntPtr]::Zero) { return '' }
  $bytes = New-Object byte[] $c.CredentialBlobSize
  [System.Runtime.InteropServices.Marshal]::Copy($c.CredentialBlob, $bytes, 0, $c.CredentialBlobSize)
  # keytar writes UTF-8 bytes; native Windows tools write UTF-16LE. Detect the
  # latter by its interleaved NUL bytes so both kinds of entry read correctly.
  if ($bytes.Length -ge 2 -and ($bytes.Length % 2) -eq 0 -and $bytes[1] -eq 0 -and $bytes[3] -eq 0) {
    return [System.Text.Encoding]::Unicode.GetString($bytes)
  }
  return [System.Text.Encoding]::UTF8.GetString($bytes)
}

if ($List) {
  $count = 0; $ptr = [IntPtr]::Zero
  if (-not [WSM.Cred]::CredEnumerate($Filter, 0, [ref] $count, [ref] $ptr)) {
    $code = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
    if ($code -eq 1168) { Write-Error "no credentials match '$Filter'"; exit 1 }
    Write-Error "CredEnumerate failed (win32 error $code)"; exit 1
  }
  # Collect first. Windows PowerShell 5.1 drops pipeline output emitted inside a
  # try/finally when the script then calls `exit` (the objects never reach
  # Out-Default), which made `credman.ps1 -List` print nothing even when
  # CredEnumerate succeeded.
  $rows = New-Object System.Collections.ArrayList
  try {
    for ($i = 0; $i -lt $count; $i++) {
      $item = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($ptr, $i * [IntPtr]::Size)
      $c = [System.Runtime.InteropServices.Marshal]::PtrToStructure($item, [type][WSM.Cred+CREDENTIAL])
      [void]$rows.Add([pscustomobject]@{
        Target = [System.Runtime.InteropServices.Marshal]::PtrToStringUni($c.TargetName)
        User   = [System.Runtime.InteropServices.Marshal]::PtrToStringUni($c.UserName)
        Bytes  = $c.CredentialBlobSize
      })
    }
  } finally { [WSM.Cred]::CredFree($ptr) }
  $sorted = $rows | Sort-Object Target
  if ($NamesOnly) {
    # Success-pipeline strings (not [Console]::Out): wezterm.run_child_process
    # captures the pipe, which is what the exporter uses to match targets.
    foreach ($r in $sorted) { $r.Target }
  } else {
    $sorted
  }
  return
}

if (-not $Target) {
  $svc = if ($Port -gt 0) { "ssh@${Host_}:${Port}" } else { "ssh@${Host_}" }
  $Target = "$svc/$User"
}

$p = [IntPtr]::Zero
if (-not [WSM.Cred]::CredRead($Target, 1, 0, [ref] $p)) {
  $code = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
  if ($code -eq 1168) { Write-Error "credential not found: $Target"; exit 1 }
  Write-Error "CredRead failed for '$Target' (win32 error $code)"; exit 1
}
try {
  # Success pipeline, not [Console]::Out. wezterm.run_child_process only
  # captures the pipe; Console.Out is the attached tty and comes back empty.
  # Callers strip a trailing newline (Write-Output always adds one).
  Write-Output (Read-Blob $p)
} finally { [WSM.Cred]::CredFree($p) }
