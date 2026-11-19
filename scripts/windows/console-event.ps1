# Runs tvpipe in a console of its own and, after -Secs seconds, sends it
# what a person at that console would: Ctrl-C (-Event 0), Ctrl-Break
# (-Event 1) or `q` + Enter (-Event q). Prints the time to exit, the exit
# code (130 after Ctrl-C/Break, 0 after q) and tvpipe's output.
#
# `prlctl exec` gives a process no console, so this is how the console
# paths of crates/tvpipe/src/signals.rs and crates/tvpipe-cli/src/interrupt.rs
# are exercised from the Mac:
#
#   prlctl exec "Windows 11" --current-user powershell -NoProfile
#     -ExecutionPolicy Bypass -File Y:\...\scripts\windows\console-event.ps1
#     -Secs 6 -Event 0 -TvArgs "debug pattern --to cast:10.211.55.2:8011"
#
# (prlctl splits arguments at spaces; wrap the call in a .cmd file when
# -TvArgs has several words.)
param(
  [int]$Secs = 6,
  [string]$Event = "0",
  [string]$TvArgs = "--help",
  [string]$Exe = "C:\tvp\target\release\tvpipe.exe",
  [string]$Log = "C:\tvp\console-event.log",
  [int]$SendTo = 0
)
$env:PATH = "C:\tvp\ffmpeg\bin;" + $env:PATH
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Con {
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool AttachConsole(uint pid);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool FreeConsole();
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool GenerateConsoleCtrlEvent(uint ev, uint group);
  [StructLayout(LayoutKind.Explicit, CharSet=CharSet.Unicode)] public struct KEY_INPUT {
    [FieldOffset(0)] public ushort EventType;
    [FieldOffset(4)] public int bKeyDown;
    [FieldOffset(8)] public ushort wRepeatCount;
    [FieldOffset(10)] public ushort wVirtualKeyCode;
    [FieldOffset(12)] public ushort wVirtualScanCode;
    [FieldOffset(14)] public char UnicodeChar;
    [FieldOffset(16)] public uint dwControlKeyState;
  }
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool WriteConsoleInputW(IntPtr h, KEY_INPUT[] rec, uint n, out uint written);
  // Ctrl-C/Break go to every process on the console, the sender included:
  // it ignores Ctrl-C; for Ctrl-Break it must be a throwaway process.
  public static string Signal(uint pid, uint ev) {
    FreeConsole();
    if (!AttachConsole(pid)) return "AttachConsole failed: " + Marshal.GetLastWin32Error();
    SetConsoleCtrlHandler(IntPtr.Zero, true);
    bool ok = GenerateConsoleCtrlEvent(ev, 0);
    FreeConsole();
    return "GenerateConsoleCtrlEvent(" + ev + ") = " + ok;
  }
  public static string Type(uint pid, string text) {
    FreeConsole();
    if (!AttachConsole(pid)) return "AttachConsole failed: " + Marshal.GetLastWin32Error();
    IntPtr h = CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
    var recs = new System.Collections.Generic.List<KEY_INPUT>();
    foreach (char c in text) {
      for (int down = 1; down >= 0; down--) {
        var k = new KEY_INPUT();
        k.EventType = 1; k.bKeyDown = down; k.wRepeatCount = 1;
        k.wVirtualKeyCode = c == '\r' ? (ushort)0x0D : (ushort)char.ToUpper(c);
        k.UnicodeChar = c;
        recs.Add(k);
      }
    }
    uint written;
    bool ok = WriteConsoleInputW(h, recs.ToArray(), (uint)recs.Count, out written);
    FreeConsole();
    return "typed " + written + " key events: " + ok;
  }
}
"@
if ($SendTo -ne 0) { [Con]::Signal([uint32]$SendTo, [uint32]$Event); exit 0 }

$p = Start-Process -FilePath "cmd.exe" -ArgumentList "/c $Exe $TvArgs > $Log 2>&1" -PassThru -WindowStyle Minimized
Start-Sleep -Seconds 1
$tv = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id) and Name='$(Split-Path $Exe -Leaf)'"
if (-not $tv) { "tvpipe did not start"; Get-Content $Log; exit 1 }
# Write something before attaching to tvpipe's console: PowerShell opens
# its stdout lazily and would otherwise pick up that console.
"tvpipe pid $($tv.ProcessId)"
Start-Sleep -Seconds $Secs
switch ($Event) {
  "q" { [Con]::Type([uint32]$tv.ProcessId, "q`r") }
  "1" {
    Start-Process powershell -Wait -WindowStyle Hidden -ArgumentList `
      "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -SendTo $($tv.ProcessId) -Event 1"
    "sent Ctrl-Break"
  }
  default { [Con]::Signal([uint32]$tv.ProcessId, 0) }
}
$t0 = Get-Date
$done = $p.WaitForExit(30000)
"exited: $done after $([int]((Get-Date) - $t0).TotalMilliseconds) ms, exit code $($p.ExitCode)"
Get-Content $Log
