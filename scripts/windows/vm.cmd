@echo off
rem Runs a command in a VM-local copy of the worktree (see docs/cross-platform.md).
rem
rem   prlctl exec "Windows 11" --current-user cmd /c ^
rem     "Y:\Development\streamlib-windows\scripts\windows\vm.cmd cargo test"
rem
rem Mirrors the worktree from the shared folder (TVPIPE_SRC, default: this
rem script's checkout) to %TVPIPE_VM%\src (default C:\tvp), without
rem target\ and .git\, and builds into %TVPIPE_VM%\target: building on the
rem Parallels shared folder is slow. Puts cargo, LLVM (clang for ring on
rem aarch64) and %TVPIPE_VM%\ffmpeg\bin on PATH.
setlocal
if "%TVPIPE_VM%"=="" set TVPIPE_VM=C:\tvp
if "%TVPIPE_SRC%"=="" set TVPIPE_SRC=%~dp0..\..
robocopy "%TVPIPE_SRC%" "%TVPIPE_VM%\src" /MIR /XD target .git /NFL /NDL /NJH /NJS /NP >nul
if errorlevel 8 (echo robocopy failed & exit /b 1)
set PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\LLVM\bin;%TVPIPE_VM%\ffmpeg\bin;%PATH%
set CARGO_TARGET_DIR=%TVPIPE_VM%\target
cd /d "%TVPIPE_VM%\src"
%*
