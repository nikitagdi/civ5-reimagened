# Pins the processes of one Chrome executable to the performance cores of a hybrid CPU (the bench, Windows only).
# On a hybrid Intel CPU the scheduler can run the renderer's main thread on an efficiency core for a whole launch,
# which makes every frame about twice as slow. The cores of the highest
# EfficiencyClass (GetSystemCpuSetInformation, processor group 0) are the performance ones; a CPU with one class is
# left alone. Prints `pinned <n> mask 0x...` or `skip: <why>`.
#   powershell -NoProfile -File pin-pcores.ps1 -Exe <path of chrome.exe>
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = "Stop"
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class CpuSets {
  [DllImport("kernel32.dll", SetLastError = true)]
  static extern bool GetSystemCpuSetInformation(IntPtr info, uint len, out uint returned, IntPtr process, uint flags);
  // The mask of the logical processors of group 0 with the highest efficiency class; 0 when all have the same class.
  public static long PerformanceMask() {
    uint len;
    GetSystemCpuSetInformation(IntPtr.Zero, 0, out len, IntPtr.Zero, 0);
    IntPtr buf = Marshal.AllocHGlobal((int)len);
    try {
      if (!GetSystemCpuSetInformation(buf, len, out len, IntPtr.Zero, 0)) return 0;
      byte[] b = new byte[len];
      Marshal.Copy(buf, b, 0, (int)len);
      int max = -1, min = 256;
      for (int at = 0; at < len; at += BitConverter.ToInt32(b, at)) {
        if (BitConverter.ToInt32(b, at + 4) != 0 || BitConverter.ToUInt16(b, at + 12) != 0) continue;
        int c = b[at + 18];
        if (c > max) max = c;
        if (c < min) min = c;
      }
      if (max == min) return 0;
      long mask = 0;
      for (int at = 0; at < len; at += BitConverter.ToInt32(b, at)) {
        if (BitConverter.ToInt32(b, at + 4) != 0 || BitConverter.ToUInt16(b, at + 12) != 0) continue;
        if (b[at + 18] == max) mask |= 1L << b[at + 14];
      }
      return mask;
    } finally { Marshal.FreeHGlobal(buf); }
  }
}
"@
$mask = [CpuSets]::PerformanceMask()
if ($mask -eq 0) { "skip: not a hybrid CPU"; exit 0 }
$full = [IO.Path]::GetFullPath($Exe)
$n = 0
foreach ($p in Get-Process -ErrorAction SilentlyContinue) {
  try {
    if ($p.Path -and [string]::Equals([IO.Path]::GetFullPath($p.Path), $full, [StringComparison]::OrdinalIgnoreCase)) {
      $p.ProcessorAffinity = [IntPtr]$mask
      $n++
    }
  } catch {}
}
"pinned $n mask 0x{0:X}" -f $mask
