@echo off
setlocal enabledelayedexpansion
rem Builds shadowtaskbar-rs in release mode, working around coreutils'
rem link.exe shadowing the real MSVC linker in PATH (see HANDOFF.md).

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
    echo [build.cmd] vswhere.exe not found - is Visual Studio installed?
    exit /b 1
)

for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath -latest`) do (
    set "VSINSTALL=%%i"
)
if not defined VSINSTALL (
    echo [build.cmd] No Visual Studio install with the C++ build tools ^(VC.Tools.x86.x64^) was found.
    echo [build.cmd] Install the "Desktop development with C++" workload, or build from an
    echo [build.cmd] "x64 Native Tools" developer shell.
    exit /b 1
)

for /f "usebackq tokens=*" %%i in (`dir /b /ad "%VSINSTALL%\VC\Tools\MSVC"`) do (
    set "MSVCVER=%%i"
)
if not defined MSVCVER (
    echo [build.cmd] Found Visual Studio at "%VSINSTALL%" but no MSVC toolset under VC\Tools\MSVC.
    exit /b 1
)

set "MSVCBIN=%VSINSTALL%\VC\Tools\MSVC\%MSVCVER%\bin\Hostx64\x64"
if not exist "%MSVCBIN%\link.exe" (
    echo [build.cmd] Expected link.exe at "%MSVCBIN%" but it's missing.
    exit /b 1
)

echo [build.cmd] Using MSVC linker from: %MSVCBIN%
set "PATH=%MSVCBIN%;%PATH%"

cargo build --release
exit /b %errorlevel%
