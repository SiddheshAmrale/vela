# Real OS input smoke test (Windows): drives the VELA window with genuine mouse clicks
# and key presses (SendInput), then checks the player state over the pipe.
param([string]$Exe = "$PSScriptRoot\..\target\release\vela.exe", [string]$Media = "$PSScriptRoot\..\.cache\testmedia\multi-hevc.mkv")

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class In {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, IntPtr e);
  public static void Click(int x, int y) { SetCursorPos(x, y); System.Threading.Thread.Sleep(150); mouse_event(2,0,0,0,IntPtr.Zero); mouse_event(4,0,0,0,IntPtr.Zero); }
  public static void Wheel(int x, int y, int d) { SetCursorPos(x, y); mouse_event(0x800,0,0,(uint)d,IntPtr.Zero); }
  public static void Key(byte vk) { keybd_event(vk,0,0,IntPtr.Zero); keybd_event(vk,0,2,IntPtr.Zero); }
}
'@

# Physical pixel coordinates everywhere (otherwise Windows virtualises them).
[In]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
$cv = Join-Path $PSScriptRoot 'cv.py'
function State { python $cv state | ConvertFrom-Json }
$results = @()
# Poll a condition (scriptblock over the state) for up to 2 s.
function Check($name, [scriptblock]$cond) {
  $ok = $false
  for ($i = 0; $i -lt 20 -and -not $ok; $i++) { $s = State; $ok = [bool](& $cond $s); if (-not $ok) { Start-Sleep -Milliseconds 100 } }
  $script:results += [pscustomobject]@{ Check = $name; Pass = $ok }
  "{0} {1}" -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), $name
}

$p = Start-Process -FilePath $Exe -ArgumentList $Media -PassThru
Start-Sleep 3
$p.Refresh()
# Windows only lets a background process change the foreground window right
# after input, so tap Alt first, then make sure VELA really is in front.
[In]::Key(0x12)
[In]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep 0.3
if ([In]::GetForegroundWindow() -ne $p.MainWindowHandle) {
  python $cv quit | Out-Null
  throw 'VELA could not take keyboard focus (another app is in front); re-run without touching the PC.'
}
# Windows may refuse to bring a test-launched window to the front; pin it on
# top so other windows can't swallow the synthetic clicks.
python $cv action ontop | Out-Null
$r = New-Object In+RECT; $null = [In]::GetWindowRect($p.MainWindowHandle, [ref]$r)
$cx = [int](($r.L + $r.R) / 2); $cy = [int](($r.T + $r.B) / 2)
Start-Sleep 0.5

# click on the video toggles pause
$before = (State).pause
[In]::Click($cx, $cy); Start-Sleep 0.6
Check 'mouse click on video toggles pause' { param($s) $s.pause -ne $before }
[In]::Click($cx, $cy); Start-Sleep 0.6
Check 'second click resumes' { param($s) $s.pause -eq $before }

# keyboard: space, arrows, M, F, Esc
[In]::Key(0x20); Start-Sleep 0.5; Check 'Space key pauses' { param($s) $s.pause }
[In]::Key(0x20); Start-Sleep 0.5
$t = (State).time; [In]::Key(0x27); Start-Sleep 0.6; Check 'Right arrow seeks forward' { param($s) $s.time -gt $t + 3 }
$v = (State).volume; [In]::Key(0x28); Start-Sleep 0.4; Check 'Down arrow lowers volume' { param($s) $s.volume -lt $v }
[In]::Key(0x4D); Start-Sleep 0.4; Check 'M mutes' { param($s) $s.mute }
[In]::Key(0x4D); Start-Sleep 0.4
[In]::Key(0x46); Start-Sleep 1.0; Check 'F enters fullscreen' { param($s) $s.fullscreen }
[In]::Key(0x1B); Start-Sleep 1.0; Check 'Esc leaves fullscreen' { param($s) -not $s.fullscreen }

# mouse wheel = volume
$v = (State).volume; [In]::Wheel($cx, $cy, 120); Start-Sleep 0.4; Check 'wheel up raises volume' { param($s) $s.volume -gt $v }

# double click = fullscreen
[In]::SetCursorPos($cx, $cy); Start-Sleep -Milliseconds 150; [In]::mouse_event(2,0,0,0,[IntPtr]::Zero); [In]::mouse_event(4,0,0,0,[IntPtr]::Zero); Start-Sleep -Milliseconds 60; [In]::mouse_event(2,0,0,0,[IntPtr]::Zero); [In]::mouse_event(4,0,0,0,[IntPtr]::Zero); Start-Sleep 1.0
Check 'double-click toggles fullscreen' { param($s) $s.fullscreen }
[In]::Key(0x1B); Start-Sleep 1.0

# click on the seek bar (bottom of window) jumps there
$null = [In]::GetWindowRect($p.MainWindowHandle, [ref]$r)
[In]::SetCursorPos($cx, $cy - 50); Start-Sleep 0.4
$k = [In]::GetDpiForWindow($p.MainWindowHandle) / 96.0   # UI layout is in logical pixels
[In]::Click([int]($r.L + 22 * $k + 0.75 * ($r.R - $r.L - 44 * $k)), [int]($r.B - 72 * $k)); Start-Sleep 0.8
Check 'click on seek bar at 75%' { param($s) [math]::Abs($s.time / $s.duration - 0.75) -lt 0.1 }

# right click opens the context menu
[In]::SetCursorPos($cx, $cy); Start-Sleep -Milliseconds 150; [In]::mouse_event(8,0,0,0,[IntPtr]::Zero); [In]::mouse_event(16,0,0,0,[IntPtr]::Zero); Start-Sleep 0.5
Check 'right-click opens context menu' { param($s) $s.menu -eq 'Context' }
[In]::Key(0x1B); Start-Sleep 0.3

python $cv quit | Out-Null
"{0}/{1} real-input checks passed" -f @($results | Where-Object Pass).Count, $results.Count
