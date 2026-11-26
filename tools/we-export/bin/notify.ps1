param([string]$ResultPath)

$ErrorActionPreference = 'SilentlyContinue'
if (-not (Test-Path $ResultPath)) { exit 0 }

$j = Get-Content -Raw -Encoding UTF8 $ResultPath | ConvertFrom-Json
if ($null -eq $j) { exit 0 }

function Esc([string]$s) {
  if ($null -eq $s) { return '' }
  return $s.Replace('&', '&amp;').Replace('<', '&lt;').Replace('>', '&gt;')
}

$title = if ($j.title) { [string]$j.title } else { '壁纸导出' }
$text  = if ($j.text)  { [string]$j.text }  else { '' }

$shown = $false
try {
  [void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
  [void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom, ContentType = WindowsRuntime]
  $xmlText = "<toast><visual><binding template=""ToastGeneric""><text>$(Esc $title)</text><text>$(Esc $text)</text></binding></visual></toast>"
  $doc = New-Object Windows.Data.Xml.Dom.XmlDocument
  $doc.LoadXml($xmlText)
  $toast = New-Object Windows.UI.Notifications.ToastNotification $doc
  $appId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
  [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
  $shown = $true
} catch {
  $shown = $false
}

if (-not $shown) {
  try {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    $ni = New-Object System.Windows.Forms.NotifyIcon
    $ni.Icon = [System.Drawing.SystemIcons]::Information
    $ni.Visible = $true
    $ni.BalloonTipTitle = $title
    $ni.BalloonTipText = $text
    $ni.ShowBalloonTip(6000)
    Start-Sleep -Seconds 7
    $ni.Dispose()
  } catch { }
}
