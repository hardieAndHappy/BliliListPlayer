@echo off
cd /d "%~dp0"
title BiliListPlayer Dev
echo ========================================
echo   BiliListPlayer Dev Mode
echo ========================================
echo.
where node >/dev/null 2>nul
if errorlevel 1 (
  echo [ERROR] Node.js not found. Install Node.js 18+ first.
  goto fail
)
where cargo >/dev/null 2>nul
if errorlevel 1 (
  echo [ERROR] Rust not found. Install via rustup: https://rustup.rs
  goto fail
)
echo Checking pnpm...
where corepack >/dev/null 2>nul
if not errorlevel 1 (
  corepack enable pnpm >/dev/null 2>nul
  if errorlevel 1 corepack prepare pnpm@latest --activate >/dev/null 2>nul
)
if not exist "node_modules" (
  echo.
  echo First run: installing dependencies, please wait...
  echo.
  call corepack pnpm install --no-frozen-lockfile
  if errorlevel 1 (
    echo.
    echo [ERROR] Dependency install failed.
    goto fail
  )
  echo.
  echo Dependencies installed.
  echo.
)
echo Starting app (first Rust build may take several minutes, do not close this window)...
echo Close the app window to exit dev mode.
echo.
call corepack pnpm tauri dev
if errorlevel 1 (
  echo.
  echo [ERROR] Start failed. See errors above.
  goto fail
)
echo.
echo App exited.
pause
exit /b 0

:fail
echo.
echo Something went wrong. Press any key to close.
pause >nul
exit /b 1
