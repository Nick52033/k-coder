@echo off
setlocal EnableDelayedExpansion

rem =====================================================================
rem  k-Coder build + deploy helper.
rem
rem  IMPORTANT: keep this file pure ASCII (7-bit).
rem  cmd.exe reads .bat files with the OEM codepage (cp936 on this
rem  machine), so UTF-8 comments get mis-decoded and leak into the
rem  console as bogus "not recognized as a command" errors.
rem =====================================================================

if "%KC_REPO%"=="" (
    set REPO=D:\code\Nick\k-coder
) else (
    set REPO=%KC_REPO%
)
set RELEASE=%REPO%\src-tauri\target\release
if "%KC_DEST%"=="" (
    set DEST=D:\apps\k-coder
) else (
    set DEST=%KC_DEST%
)

set SKIP_BUILD=0
set LAUNCH_AFTER=1
set PAUSE_AT_END=1
set FULL_BUILD=0
for %%A in (%*) do (
    if /i "%%~A"=="--skip-build" set SKIP_BUILD=1
    if /i "%%~A"=="--no-launch" set LAUNCH_AFTER=0
    if /i "%%~A"=="--no-pause"  set PAUSE_AT_END=0
    if /i "%%~A"=="--full"      set FULL_BUILD=1
)

set STAMP=%REPO%\src-tauri\target\.kc-deploy-stamp
if "%FULL_BUILD%"=="1" (set MODE=full) else (set MODE=fast)
set RC=0

title k-Coder Build and Deploy
echo ============================================================
echo  k-Coder Build and Deploy
echo  Repo : %REPO%
echo  Dest : %DEST%
echo  Mode : %MODE%  SKIP_BUILD=%SKIP_BUILD%  LAUNCH=%LAUNCH_AFTER%
echo ============================================================
echo.

rem ---------------------------------------------------------------
rem Fast mode: release profile overrides for this build only
rem   opt-level=1 + 256 codegen-units + incremental
rem Recompiles drop from minutes to tens of seconds; the binary is
rem slightly slower at runtime. The first switch into or out of fast
rem triggers one full rebuild. Use --full for a release-grade build.
rem ---------------------------------------------------------------
if "%MODE%"=="fast" (
    set CARGO_PROFILE_RELEASE_OPT_LEVEL=1
    set CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256
    set CARGO_PROFILE_RELEASE_INCREMENTAL=true
    set CARGO_PROFILE_RELEASE_DEBUG=false
)

echo [1/4] Build...
if "%SKIP_BUILD%"=="1" goto skip_build

rem --- Source stamp: SHA256 of HEAD + working tree diff. Skip if same ---
pushd "%REPO%"
if errorlevel 1 goto err_repo
git rev-parse HEAD > "%TEMP%\kc_stamp_src.txt" 2>nul
git status --porcelain >> "%TEMP%\kc_stamp_src.txt" 2>nul
git diff --binary HEAD >> "%TEMP%\kc_stamp_src.txt" 2>nul
set HASH=
for /f "skip=1 delims=" %%H in ('certutil -hashfile "%TEMP%\kc_stamp_src.txt" SHA256') do (
    if not defined HASH set "HASH=%%H"
)
del "%TEMP%\kc_stamp_src.txt" >nul 2>&1
set "HASH=!HASH: =!"
if "!HASH!"=="" (
    echo      WARNING: cannot compute source stamp, building anyway.
) else (
    set "CUR=!HASH!^|%MODE%"
    if exist "%STAMP%" (
        set /p OLD=<"%STAMP%"
        if "!OLD!"=="!CUR!" if exist "%RELEASE%\k-coder.exe" (
            popd
            echo      No source changes since last deploy, build skipped.
            goto skip_build
        )
    )
)

echo      Building release (%MODE%), this may take a while on first run...
call pnpm tauri build --no-bundle
set BUILD_RC=!errorlevel!
popd
if not "!BUILD_RC!"=="0" (
    echo *** Build failed, exit code !BUILD_RC!. Deployment skipped.
    goto hardfail
)
echo      Build OK.
if not "!CUR!"=="" >"%STAMP%" echo !CUR!
goto check_exe

:skip_build
echo      Skipped / nothing to do.

:check_exe
if exist "%RELEASE%\k-coder.exe" goto stop_old
echo *** Not found: %RELEASE%\k-coder.exe
echo     Drop --skip-build to build it.
goto hardfail

:stop_old
echo.
echo [2/4] Stopping running k-Coder, if any...
set KILLED=0
taskkill /IM "k-coder.exe" /F >nul 2>&1
if not errorlevel 1 set KILLED=1

set WAIT=0
:wait_dead
set /a WAIT+=1
rem wmic was removed on Win11; use tasklist with filter instead
tasklist /FI "IMAGENAME eq k-coder.exe" | find /I "k-coder.exe" >nul
if errorlevel 1 goto wait_done
if !WAIT! EQU 10 echo      Still running, retrying taskkill...
if !WAIT! GEQ 10 taskkill /IM "k-coder.exe" /F >nul 2>&1
if !WAIT! GEQ 15 (
    echo *** Cannot stop k-coder.exe. Close it manually and retry.
    goto hardfail
)
timeout /t 1 /nobreak >nul
goto wait_dead
:wait_done
if "%KILLED%"=="1" echo      k-Coder stopped.

echo.
echo [3/4] Cleaning stale build artifacts in %DEST%...
if not exist "%DEST%" mkdir "%DEST%"
for %%D in (.fingerprint build deps examples incremental bundle nsis wix) do (
    if exist "%DEST%\%%D" rmdir /s /q "%DEST%\%%D" >nul 2>&1
)
for %%F in (k_coder.pdb k-coder.d .cargo-artifact-lock .cargo-build-lock .cargo-lock) do (
    if exist "%DEST%\%%F" del /q "%DEST%\%%F" >nul 2>&1
)
echo      Cleanup OK.

echo.
echo [4/4] Copying runtime files...
robocopy "%RELEASE%" "%DEST%" k-coder.exe /R:2 /W:1 /NFL /NDL /NJH /NJS /NP
if errorlevel 8 (
    echo *** Failed to copy k-coder.exe, robocopy exit code !errorlevel!
    goto hardfail
)

rem Resource folders declared by bundle.resources in tauri.conf.json.
rem They map to skills/ and tools/ next to the exe. A folder that
rem is absent from the build output is a warning, never a hard failure
rem (robocopy returns 16 when the source directory does not exist).
for %%D in (skills tools) do if not exist "%RELEASE%\%%D" (
    echo      WARNING: %RELEASE%\%%D missing in build output, skipped.
)
for %%D in (skills tools) do if exist "%RELEASE%\%%D" (
    robocopy "%RELEASE%\%%D" "%DEST%\%%D" /MIR /R:2 /W:1 /NFL /NDL /NJH /NJS /NP
    if errorlevel 8 (
        echo *** Failed to copy %RELEASE%\%%D, robocopy exit code !errorlevel!
        goto hardfail
    )
)
echo      Copy OK.

echo.
echo ============================================================
echo  Deploy finished: %DEST%
echo ============================================================
if "%LAUNCH_AFTER%"=="0" goto done

set EXE=%DEST%\k-coder.exe
if not exist "%EXE%" goto err_exe_missing

echo.
echo Starting k-Coder...
start "" /D "%DEST%" "%EXE%"

set ALIVE=0
set V=0
:verify
timeout /t 1 /nobreak >nul
set /a V+=1
tasklist /FI "IMAGENAME eq k-coder.exe" | find /I "k-coder.exe" >nul
if not errorlevel 1 set ALIVE=1
if "!ALIVE!"=="0" if !V! LSS 6 goto verify

if "!ALIVE!"=="1" (
    echo      k-Coder started.
) else (
    echo *** k-Coder did not stay running. Try launching it manually:
    echo     "%EXE%"
)
goto done

:err_repo
echo *** Cannot enter %REPO%
goto hardfail

:err_exe_missing
echo *** %EXE% not found, cannot start.
goto hardfail

:hardfail
set RC=1
echo.
echo *** BUILD/DEPLOY FAILED. See messages above. ***

:done
echo.
if "%PAUSE_AT_END%"=="1" (
    echo Press any key to close this window...
    pause >nul
)
endlocal & exit /b %RC%
