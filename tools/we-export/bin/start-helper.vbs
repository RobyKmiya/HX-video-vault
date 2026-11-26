Option Explicit
' Boot fallback for the VideoVault export helper (2026-11-27: rewritten).
'
' This launcher is pure ASCII on purpose - VBScript reads .vbs files with the
' system ANSI code page, so non-ASCII characters would turn into mojibake.
' The trick: it contains NO paths at all. Everything (node.exe path, the
' supervise.mjs location, which lives under a Chinese directory) comes from
' the HKCU Run entry that install.mjs writes; RegRead returns it as Unicode,
' so Chinese paths flow through fine at runtime.
'
' Chain of survival: HKCU Run (primary) -> this Startup-folder fallback ->
' scheduled task. The supervisor itself keeps helper.mjs alive.

Dim sh, fso, logDir, f, cmd, nodePath, supPath, cand, i, parts

Set sh  = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

' ASCII-only scratch log under TEMP
logDir = sh.ExpandEnvironmentStrings("%TEMP%") & "\VideoVaultHelper"
If Not fso.FolderExists(logDir) Then fso.CreateFolder(logDir)
Set f = fso.OpenTextFile(logDir & "\autostart.log", 8, True)
f.WriteLine Now & "  fallback launcher fired"
f.Close

' The registered autostart command looks like:  "C:\...\node.exe" "C:\...supervise.mjs"
cmd = ""
On Error Resume Next
cmd = sh.RegRead("HKCU\Software\Microsoft\Windows\CurrentVersion\Run\WallpaperExportHelper")
On Error GoTo 0

supPath = ""
nodePath = ""
If cmd <> "" Then
  parts = Split(cmd, """")
  ' Split result: [0]="" [1]=node [2]=" " [3]=supervise.mjs
  If UBound(parts) >= 3 Then
    nodePath = parts(1)
    supPath = parts(3)
  End If
End If

' If the registered node.exe vanished (upgrade/cleanup), probe common locations
If supPath <> "" And nodePath <> "" And Not fso.FileExists(nodePath) Then
  cand = Array( _
    sh.ExpandEnvironmentStrings("%ProgramFiles%") & "\nodejs\node.exe", _
    sh.ExpandEnvironmentStrings("%ProgramFiles(x86)%") & "\nodejs\node.exe", _
    sh.ExpandEnvironmentStrings("%LOCALAPPDATA%") & "\Programs\nodejs\node.exe" )
  nodePath = "node.exe"
  For i = 0 To UBound(cand)
    If fso.FileExists(cand(i)) Then
      nodePath = cand(i)
      Exit For
    End If
  Next
End If

If supPath = "" Or Not fso.FileExists(supPath) Then
  Set f = fso.OpenTextFile(logDir & "\autostart.log", 8, True)
  f.WriteLine Now & "  no supervise.mjs known (Run entry missing), abort"
  f.Close
  WScript.Quit 1
End If

Set f = fso.OpenTextFile(logDir & "\autostart.log", 8, True)
f.WriteLine Now & "  node=" & nodePath
f.WriteLine Now & "  sup=" & supPath
f.Close

sh.Run """" & nodePath & """ """ & supPath & """", 0, False
