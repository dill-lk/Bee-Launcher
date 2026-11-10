[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $ExecutablePath,

    [Parameter(Mandatory = $true)]
    [string] $IconPath,

    [Parameter(Mandatory = $true)]
    [string] $AppUserModelId,

    [Parameter(Mandatory = $true)]
    [string] $ShortcutPaths
)

$ErrorActionPreference = 'Stop'

$interop = @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

namespace BeeLauncher.ShortcutMetadata
{
    [StructLayout(LayoutKind.Sequential)]
    public struct PropertyKey
    {
        public Guid FormatId;
        public uint PropertyId;

        public PropertyKey(Guid formatId, uint propertyId)
        {
            FormatId = formatId;
            PropertyId = propertyId;
        }
    }

    [StructLayout(LayoutKind.Explicit, Size = 16)]
    public struct PropVariant
    {
        [FieldOffset(0)] public ushort VariantType;
        [FieldOffset(8)] public IntPtr StringValue;
    }

    [ComImport]
    [Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IPropertyStore
    {
        [PreserveSig] int GetCount(out uint count);
        [PreserveSig] int GetAt(uint index, out PropertyKey key);
        [PreserveSig] int GetValue(ref PropertyKey key, out PropVariant value);
        [PreserveSig] int SetValue(ref PropertyKey key, ref PropVariant value);
        [PreserveSig] int Commit();
    }

    [ComImport]
    [Guid("00021401-0000-0000-C000-000000000046")]
    [ClassInterface(ClassInterfaceType.None)]
    public class ShellLink
    {
    }

    public static class ShortcutProperties
    {
        [DllImport("ole32.dll")]
        private static extern int PropVariantClear(ref PropVariant value);

        public static void SetAppUserModelId(string shortcutPath, string appUserModelId)
        {
            object shellLink = new ShellLink();
            IPersistFile persistFile = (IPersistFile)shellLink;
            persistFile.Load(shortcutPath, 0);

            IPropertyStore propertyStore = (IPropertyStore)shellLink;
            PropertyKey key = new PropertyKey(
                new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 5);
            PropVariant value = new PropVariant();
            value.VariantType = 31; // VT_LPWSTR
            value.StringValue = Marshal.StringToCoTaskMemUni(appUserModelId);

            try
            {
                int result = propertyStore.SetValue(ref key, ref value);
                if (result < 0) Marshal.ThrowExceptionForHR(result);
                result = propertyStore.Commit();
                if (result < 0) Marshal.ThrowExceptionForHR(result);
                persistFile.Save(shortcutPath, true);
            }
            finally
            {
                PropVariantClear(ref value);
            }
        }
    }
}
'@

try {
    Add-Type -TypeDefinition $interop -Language CSharp

    $exeFullPath = [System.IO.Path]::GetFullPath($ExecutablePath)
    $iconFullPath = [System.IO.Path]::GetFullPath($IconPath)
    $shell = New-Object -ComObject WScript.Shell
    $targets = New-Object 'System.Collections.Generic.List[string]'
    $knownPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)

    foreach ($path in ($ShortcutPaths -split '\|' | Where-Object { $_ })) {
        if ($knownPaths.Add([System.IO.Path]::GetFullPath($path))) {
            $targets.Add($path)
        }
    }

    # A taskbar pin is a separate .lnk copy. Repair only pins that launch this
    # install, leaving unrelated user-pinned applications untouched.
    $taskbarPins = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar'
    if (Test-Path -LiteralPath $taskbarPins -PathType Container) {
        Get-ChildItem -LiteralPath $taskbarPins -Filter '*.lnk' -File -ErrorAction SilentlyContinue |
            ForEach-Object {
                $pinPath = $_.FullName
                try {
                    $pin = $shell.CreateShortcut($pinPath)
                    if ($pin.TargetPath -and
                        [System.String]::Equals(
                            [System.IO.Path]::GetFullPath($pin.TargetPath),
                            $exeFullPath,
                            [System.StringComparison]::OrdinalIgnoreCase)) {
                        if ($knownPaths.Add([System.IO.Path]::GetFullPath($pinPath))) {
                            $targets.Add($pinPath)
                        }
                    }
                }
                catch {
                    # Ignore non-readable shell links; explicit install links
                    # are still processed below and report any write failures.
                }
            }
    }

    foreach ($path in $targets) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Expected Bee Launcher shortcut was not created: $path"
        }
        $shortcut = $shell.CreateShortcut($path)
        $shortcut.IconLocation = "$iconFullPath,0"
        $shortcut.Save()
        [BeeLauncher.ShortcutMetadata.ShortcutProperties]::SetAppUserModelId($path, $AppUserModelId)
    }
}
catch {
    Write-Error $_
    exit 1
}
