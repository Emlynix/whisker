# Screenshots of a debug build in demo mode: every screen without a mouse,
# without touching the installed app, and without taking the focus.
#
#   pwsh tools/shots.ps1 -Exe <whisker.exe> -Out <dir> [-Only overview,dpi]
#
# Each scene starts the app with WHISKER_DEMO (no hardware, no input hook,
# no single-instance claim) and its own copy of generated demo data, on the
# secondary monitor without taking focus; clicks go through PostMessage and
# the picture through PrintWindow. Only the started process is touched.
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Out,
    [string[]]$Only = @(),
    # Unix time the demo data is built around; pass the same value to compare two builds.
    [long]$Now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
)
$ErrorActionPreference = "Stop"
$Only = @($Only | ForEach-Object { $_ -split "," } | Where-Object { $_ })
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public class Shots {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct SI { public int cb; public string r, d, t; public int x, y, w, h, cx, cy, fa, fl; public short sw, r2; public IntPtr r3, i, o, e; }
  [StructLayout(LayoutKind.Sequential)]
  public struct PI { public IntPtr p, t; public int pid, tid; }
  public struct RECT { public int L, T, R, B; }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inh, int fl, IntPtr env, string dir, ref SI si, out PI pi);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, int f);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, int f);
}
"@

# Scene = name, WHISKER_OPEN value, config overrides, clicks (client x,y).
$NAV = @{ overview = 56; dpi = 88; performance = 120; lighting = 152; buttons = 184; profiles = 216; statistics = 248; settings = 556 }
$scenes = @(
    @{ name = "01-overview" }
    @{ name = "02-dpi"; clicks = @(, @(80, $NAV.dpi)) }
    @{ name = "03-performance"; clicks = @(, @(80, $NAV.performance)) }
    @{ name = "04-lighting"; clicks = @(, @(80, $NAV.lighting)) }
    @{ name = "05-buttons"; clicks = @(, @(80, $NAV.buttons)) }
    @{ name = "06-buttons-action"; clicks = @(@(80, $NAV.buttons), @(600, 172)) }
    @{ name = "07-buttons-macros"; clicks = @(@(80, $NAV.buttons), @(295, 107)) }
    @{ name = "08-statistics"; clicks = @(, @(80, $NAV.statistics)) }
    @{ name = "09-statistics-week"; clicks = @(@(80, $NAV.statistics), @(346, 105)) }
    @{ name = "10-settings-general"; clicks = @(, @(80, $NAV.settings)) }
    @{ name = "11-settings-appearance"; clicks = @(@(80, $NAV.settings), @(310, 107)) }
    @{ name = "12-settings-mouse"; clicks = @(@(80, $NAV.settings), @(392, 107)) }
    @{ name = "13-about"; clicks = @(@(80, $NAV.settings), @(460, 107)) }
    @{ name = "14-picker"; open = "detect" }
    @{ name = "15-picker-list"; open = "detect-list" }
    @{ name = "16-picker-none"; open = "detect-none" }
    @{ name = "17-overview-light"; cfg = @{ theme = "light" } }
    @{ name = "18-overview-sk"; cfg = @{ lang = "sk" } }
    @{ name = "19-dpi-ja"; cfg = @{ lang = "ja"; theme = "dracula" }; clicks = @(, @(80, $NAV.dpi)) }
    @{ name = "20-overview-m705"; cfg = @{ device = "logitech/auto" } }
    @{ name = "21-profiles"; clicks = @(, @(80, $NAV.profiles)) }
    @{ name = "23-profiles-rename"; clicks = @(@(80, $NAV.profiles), @(659, 148)) }
    @{ name = "22-overlay"; clicks = @(, @(80, 326)) }
    @{ name = "24-settings-language"; clicks = @(@(80, $NAV.settings), @(490, 456)) }
    # Text mode: the menu is rows of the grid.
    @{ name = "30-tui-overview"; cfg = @{ tui = "1" } }
    @{ name = "31-tui-dpi"; cfg = @{ tui = "1" ; theme = "light" }; clicks = @(, @(80, 123)) }
    @{ name = "32-tui-performance"; cfg = @{ tui = "1" }; clicks = @(, @(80, 141)) }
    @{ name = "33-tui-lighting"; cfg = @{ tui = "1" }; clicks = @(, @(80, 159)) }
    @{ name = "34-tui-buttons"; cfg = @{ tui = "1" }; clicks = @(, @(80, 177)) }
    @{ name = "35-tui-statistics"; cfg = @{ tui = "1" }; clicks = @(, @(80, 213)) }
    @{ name = "36-tui-profiles"; cfg = @{ tui = "1" }; clicks = @(, @(80, 195)) }
    @{ name = "39-tui-profiles-rename"; cfg = @{ tui = "1" }; clicks = @(@(80, 195), @(417, 177)) }
    @{ name = "37-tui-overlay"; cfg = @{ tui = "1" }; clicks = @(, @(80, 249)) }
    @{ name = "38-tui-settings"; cfg = @{ tui = "1" }; clicks = @(, @(80, 555)) }
)

function New-DemoData([string]$dir, [hashtable]$overrides) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $cfg = [ordered]@{
        stages = "8,14,22,30,48,50"; enabled = "63"; active = "2"; polling = "500"; lighting = "1,2,2,0"
        steady = "FF00FF"; breathing = "FF0000,00FF00,0000FF"
        colorful = "FF0000,00FF00,0000FF,FFFF00,00FFFF,FA00FA,FFFFFF"
        dpi_colors = "FF0000,00FF00,0000FF,00FFFF,FFFF00,FF00FF,FFFFFF,FFFFFF"
        debounce = "12"; lod = "2"; sleep = "6"; dark_moving = "1"
        buttons = "100100,100200,100400,100800,101000,400100"
        device = "whiteshark/lionel"; device_usb = ""; stage = "0"; setup_asked = "1"
        theme = "tokyo"; lang = "en"; ignored_mice = ""
        update_check = "0"; update_checked = "$Now"; update_skip = ""; update_notified = ""
        # Three profiles, the second active; the third swaps Back and Forward.
        profile = "1"; "profile1.name" = "FPS"; "profile2.name" = "Desktop"
        "profile2.buttons" = "100100,100200,100400,101000,100800,400100"
    }
    foreach ($k in $overrides.Keys) { $cfg[$k] = $overrides[$k] }
    Set-Content "$dir/config.txt" ($cfg.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -Encoding utf8NoBOM

    # A week of clicks and a battery sliding from 100 % to 78 %.
    $days = @(@(3120, 180, 95, 60, 5200, 212.40), @(2870, 150, 80, 44, 4800, 190.20), @(3410, 210, 120, 70, 6100, 240.80),
        @(2650, 140, 66, 38, 4300, 175.50), @(3300, 190, 101, 55, 5600, 221.00), @(2980, 170, 90, 49, 5000, 201.70), @(1751, 30, 46, 26, 3469, 35.92))
    $today = [DateTimeOffset]::FromUnixTimeSeconds($Now).LocalDateTime.Date
    $lines = for ($i = 0; $i -lt 7; $i++) {
        $d = $days[$i]
        "day={0:yyyy-MM-dd} {1} {2} {3} {4} {5} {6}" -f $today.AddDays($i - 6), $d[0], $d[1], $d[2], $d[3], $d[4], $d[5].ToString("0.00", [Globalization.CultureInfo]::InvariantCulture)
    }
    $start = $Now - 6 * 86400
    $lines += for ($t = $start; $t -le $Now - 300; $t += 1800) { "bat=$t $([math]::Round(100 - 22 * ($t - $start) / ($Now - $start))) 0" }
    Set-Content "$dir/stats.txt" $lines -Encoding utf8NoBOM
}

function Click([IntPtr]$h, [int]$x, [int]$y) {
    $lp = [IntPtr](($y -shl 16) -bor $x)
    [Shots]::PostMessage($h, 0x0200, [IntPtr]::Zero, $lp) | Out-Null   # WM_MOUSEMOVE
    [Shots]::PostMessage($h, 0x0201, [IntPtr]1, $lp) | Out-Null         # WM_LBUTTONDOWN
    [Shots]::PostMessage($h, 0x0202, [IntPtr]::Zero, $lp) | Out-Null    # WM_LBUTTONUP
    Start-Sleep -Milliseconds 600
}

$exePath = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $Out | Out-Null
$outDir = (Resolve-Path $Out).Path
$screen = [System.Windows.Forms.Screen]::AllScreens | Where-Object { -not $_.Primary } | Select-Object -First 1
$work = Join-Path ([IO.Path]::GetTempPath()) "whisker-shots"

foreach ($s in $scenes) {
    if ($Only -and -not ($Only | Where-Object { $s.name -like "*$_*" })) { continue }
    $data = Join-Path $work $s.name
    New-DemoData $data ($s.cfg ?? @{})
    $env:WHISKER_DEMO = "1"; $env:WHISKER_DATA = $data
    # An empty variable would still count as set.
    if ($s.open) { $env:WHISKER_OPEN = $s.open } else { Remove-Item Env:WHISKER_OPEN -ErrorAction SilentlyContinue }

    $si = New-Object Shots+SI; $si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
    $si.fl = 1; $si.sw = 4   # STARTF_USESHOWWINDOW, SW_SHOWNOACTIVATE
    $pi = New-Object Shots+PI
    [Shots]::CreateProcess($exePath, "`"$exePath`"", [IntPtr]::Zero, [IntPtr]::Zero, $false, 0, [IntPtr]::Zero, (Split-Path $exePath), [ref]$si, [ref]$pi) | Out-Null

    $h = [IntPtr]::Zero
    for ($i = 0; $i -lt 80 -and $h -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 100
        $p = Get-Process -Id $pi.pid -ErrorAction SilentlyContinue
        if ($p) { $h = $p.MainWindowHandle }
    }
    if ($h -eq [IntPtr]::Zero) { "$($s.name): no window"; Stop-Process -Id $pi.pid -Force -ErrorAction SilentlyContinue; continue }
    if ($screen) { [Shots]::SetWindowPos($h, [IntPtr]::Zero, $screen.Bounds.X + 100, $screen.Bounds.Y + 100, 0, 0, 0x15) | Out-Null }  # NOSIZE|NOZORDER|NOACTIVATE
    Start-Sleep -Milliseconds 1500
    foreach ($c in ($s.clicks ?? @())) { Click $h $c[0] $c[1] }
    Start-Sleep -Milliseconds 400

    $r = New-Object Shots+RECT; [Shots]::GetClientRect($h, [ref]$r) | Out-Null
    $bmp = New-Object System.Drawing.Bitmap ($r.R), ($r.B)
    $g = [System.Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
    [Shots]::PrintWindow($h, $dc, 3) | Out-Null   # PW_CLIENTONLY | PW_RENDERFULLCONTENT
    $g.ReleaseHdc($dc); $g.Dispose()
    $bmp.Save("$outDir/$($s.name).png"); $bmp.Dispose()
    Stop-Process -Id $pi.pid -Force
    "$($s.name): $($r.R)x$($r.B)"
}
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
