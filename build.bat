@echo off
echo 🔨 Building Zentra L1 Workspace in Release Mode...
cargo build --release --workspace
if %ERRORLEVEL% NEQ 0 (
    echo ❌ Build failed!
    exit /b %ERRORLEVEL%
)

echo ✅ Build succeeded! Copying binaries to root...
copy /y target\release\zentrad.exe zentrad.exe
copy /y target\release\zentra-cli.exe zentra-cli.exe
copy /y target\release\zentra-qt.exe zentra-qt.exe

echo 🚀 Binaries copied successfully to root folder:
echo    - zentrad.exe     (Node Daemon and Dashboard Server)
echo    - zentra-cli.exe  (Command Line Wallet Interface / Interactive Shell)
echo    - zentra-qt.exe   (Traditional Desktop GUI Core Wallet)
echo.
echo To start the core GUI wallet directly, double-click:
echo    zentra-qt.exe
echo.
echo To start the node daemon and dashboard, run:
echo    .\zentrad.exe --network devnet
echo.
echo To use the CLI wallet interactively, run:
echo    .\zentra-cli.exe
