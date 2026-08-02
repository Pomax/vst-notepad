<#
.SYNOPSIS
Screenshot the real editor window.

.DESCRIPTION
The headless snapshot renderer proves the drawing code is right, but it does not
prove the actual window is: that path goes through baseview and OpenGL, and the
background is the renderer's clear colour rather than anything egui draws. This
launches the real preview window, waits for it to appear, captures its pixels
off the screen with BitBlt, and closes it.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File src/tools/capture-window.ps1 -Theme light -Out target/window-light.png
#>
param(
    [ValidateSet('light', 'dark', 'auto')]
    [string]$Theme = 'auto',
    [string]$Out = 'target/window.png',
    [string]$Exe = 'target/debug/examples/preview.exe',
    [string]$Notes = '',
    [int]$SettleMs = 2500
)

$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Capture {
    // Without this the capturing process is DPI-virtualised: window coordinates
    // come back in physical pixels while CopyFromScreen works in scaled ones,
    // and the grab lands offset from the window.
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    // GetWindowRect gives screen coordinates directly. GetClientRect +
    // ClientToScreen was tried first and returned a bogus size here.
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

    // A window's outer rect includes the invisible resize border the compositor
    // owns, so a grab of it picks up a strip of whatever is behind. This is the
    // rect that is actually painted.
    [DllImport("dwmapi.dll")]
    public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attr, out RECT value, int size);
    const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;

    /// Painted bounds, falling back to the outer rect where DWM has no answer.
    public static RECT VisibleRect(IntPtr hWnd) {
        RECT r;
        int hr = DwmGetWindowAttribute(hWnd, DWMWA_EXTENDED_FRAME_BOUNDS, out r, Marshal.SizeOf(typeof(RECT)));
        if (hr == 0 && r.Right > r.Left && r.Bottom > r.Top) return r;
        GetWindowRect(hWnd, out r);
        return r;
    }

    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    // CharSet.Unicode matters: without it the wide title marshals as ANSI and
    // comes back as just its first character.
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int count);

    /// The process's visible window whose title matches, else its largest one.
    /// `MainWindowHandle` is unreliable here — it can name whichever window
    /// happened to be created first, which is not the editor.
    /// When `strict`, only an exact title match counts. The wait loop needs
    /// that: the process also owns a console window that appears first, and
    /// falling back to "largest" immediately would grab it.
    public static IntPtr FindWindow(uint targetPid, string wantedTitle, bool strict) {
        IntPtr match = IntPtr.Zero;
        IntPtr biggest = IntPtr.Zero;
        long biggestArea = 0;
        EnumWindows((h, l) => {
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            if (pid != targetPid || !IsWindowVisible(h)) return true;
            RECT r;
            if (!GetWindowRect(h, out r)) return true;
            long area = (long)(r.Right - r.Left) * (r.Bottom - r.Top);
            var title = new StringBuilder(256);
            GetWindowTextW(h, title, 256);
            if (title.ToString() == wantedTitle) match = h;
            if (area > biggestArea) { biggestArea = area; biggest = h; }
            return true;
        }, IntPtr.Zero);
        if (match != IntPtr.Zero) return match;
        return strict ? IntPtr.Zero : biggest;
    }
}
'@

[Win32Capture]::SetProcessDPIAware() | Out-Null

if (-not (Test-Path $Exe)) {
    throw "$Exe not found - run: cargo build -p notepad-plugin --example preview"
}

$outDir = Split-Path -Parent $Out
if ($outDir -and -not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Force $outDir | Out-Null
}

$launchArgs = @($Theme)
if ($Notes) { $launchArgs += $Notes }
$proc = Start-Process -FilePath $Exe -ArgumentList $launchArgs -PassThru

try {
    # Wait for the editor window to exist.
    $hwnd = [IntPtr]::Zero
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($hwnd -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 200
        $hwnd = [Win32Capture]::FindWindow([uint32]$proc.Id, 'Notepad', $true)
    }
    if ($hwnd -eq [IntPtr]::Zero) {
        # No window titled "Notepad" turned up; take the largest one and say so.
        $hwnd = [Win32Capture]::FindWindow([uint32]$proc.Id, 'Notepad', $false)
        Write-Warning 'no window titled "Notepad" found; falling back to the largest one'
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw 'the preview window never appeared' }
    [Win32Capture]::ShowWindow($hwnd, 5) | Out-Null   # SW_SHOW
    [Win32Capture]::SetForegroundWindow($hwnd) | Out-Null

    # Let the GL context draw a few frames before grabbing pixels.
    Start-Sleep -Milliseconds $SettleMs

    $rect = [Win32Capture]::VisibleRect($hwnd)
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    if ($width -le 0 -or $height -le 0) { throw "bad window rect ${width}x${height}" }

    Add-Type -AssemblyName System.Drawing
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    # Copy straight off the screen: PrintWindow returns black for GL surfaces.
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size $width, $height))

    $full = Join-Path (Get-Location) $Out
    $bitmap.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()

    Write-Output "captured ${width}x${height} -> $Out"
}
finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}
