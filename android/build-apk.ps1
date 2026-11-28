# Build the shipping KimiOS library-OS APK (native ARM64) on Windows.
# Mirrors android/build-apk.sh: Rust staticlib -> libkimios.so -> dex -> aapt2 ->
# zip -> zipalign -> apksigner. Fails loudly instead of reusing a stale lib.
$ErrorActionPreference = "Continue"

$sdk    = "$env:LOCALAPPDATA\Android\Sdk"
$BT     = "$sdk\build-tools\34.0.0"
$NDK    = "$sdk\ndk\26.1.10909125"
$PRE    = "$NDK\toolchains\llvm\prebuilt\windows-x86_64"
$CC     = "$PRE\bin\aarch64-linux-android26-clang.cmd"
$SYSLIB = "$PRE\sysroot\usr\lib\aarch64-linux-android\26"
$JAR    = "$sdk\platforms\android-34\android.jar"
$repo   = "C:\Users\Siu\Documents\kimios"
$AD     = "$repo\android"
$OUT    = "$AD\build"
$env:JAVA_HOME = "$env:LOCALAPPDATA\Programs\jdk17"
$env:PATH = "$env:JAVA_HOME\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"

New-Item -ItemType Directory -Force -Path $OUT | Out-Null

Write-Host "[1/7] Rust staticlib (aarch64-linux-android)"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = $CC
Push-Location "$repo\user"
$log = cargo build --release -p kimi-gui --lib --target aarch64-linux-android 2>&1
$rc = $LASTEXITCODE
Pop-Location
if ($rc -ne 0) {
    Write-Host "  RUST BUILD FAILED - aborting"
    ($log | Select-String '^error' -Context 0,5 | Select-Object -First 6) | ForEach-Object { $_.Line; $_.Context.PostContext | ForEach-Object { "    $_" } }
    exit 1
}
$RUSTLIB = "$repo\user\target\aarch64-linux-android\release\libkimi_gui.a"
if (-not (Test-Path $RUSTLIB)) { Write-Host "  missing $RUSTLIB"; exit 1 }
Write-Host "  OK ($([math]::Round((Get-Item $RUSTLIB).Length/1MB,2)) MB)"

Write-Host "[2/7] libkimios.so (JNI dispatcher + guest ELF loader + Rust)"
& $CC -shared -fPIC -O2 -o "$OUT\libkimios.so" `
    "$AD\jni\kimios_jni.c" "$AD\jni\kimi_elf.c" $RUSTLIB `
    -L"$SYSLIB" -landroid -llog 2>&1 | Select-String 'error' | Select-Object -First 5
if (-not (Test-Path "$OUT\libkimios.so")) { Write-Host "  .so link FAILED"; exit 1 }
Write-Host "  OK ($([math]::Round((Get-Item "$OUT\libkimios.so").Length/1MB,2)) MB)"

Write-Host "[3/7] javac + d8"
Remove-Item "$OUT\classes","$OUT\dex" -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$OUT\classes","$OUT\dex" | Out-Null
$javaFiles = Get-ChildItem "$AD\java" -Recurse -Filter *.java | ForEach-Object { $_.FullName }
& "$env:JAVA_HOME\bin\javac.exe" -nowarn -source 8 -target 8 -bootclasspath $JAR -d "$OUT\classes" @javaFiles 2>&1 | Select-String 'error:' | Select-Object -First 5
if ($LASTEXITCODE -ne 0) { Write-Host "  javac FAILED"; exit 1 }
$classFiles = Get-ChildItem "$OUT\classes" -Recurse -Filter *.class | ForEach-Object { $_.FullName }
& "$BT\d8.bat" --release --lib $JAR --output "$OUT\dex" @classFiles 2>&1 | Select-String -Pattern 'error|Error' | Select-Object -First 5
if (-not (Test-Path "$OUT\dex\classes.dex")) { Write-Host "  d8 FAILED"; exit 1 }
Write-Host "  OK (dex $([math]::Round((Get-Item "$OUT\dex\classes.dex").Length/1KB,1)) KB)"

Write-Host "[4/7] aapt2 compile + link (with assets)"
& "$BT\aapt2.exe" compile --dir "$AD\res" -o "$OUT\res.zip" 2>&1 | Select-String 'error' | Select-Object -First 3
$assetsArg = @()
if (Test-Path "$AD\assets") { $assetsArg = @("-A", "$AD\assets") }
& "$BT\aapt2.exe" link -o "$OUT\base.apk" -I $JAR --manifest "$AD\AndroidManifest.xml" "$OUT\res.zip" --auto-add-overlay @assetsArg 2>&1 | Select-String 'error' | Select-Object -First 3
if (-not (Test-Path "$OUT\base.apk")) { Write-Host "  aapt2 link FAILED"; exit 1 }
Write-Host "  OK"

Write-Host "[5/7] package classes.dex + lib/arm64-v8a/libkimios.so"
Copy-Item "$OUT\base.apk" "$OUT\unsigned.apk" -Force
$pyCode = @"
import sys, zipfile
apk, dex, so = sys.argv[1], sys.argv[2], sys.argv[3]
with zipfile.ZipFile(apk, 'a', compression=zipfile.ZIP_DEFLATED) as z:
    z.write(dex, 'classes.dex')
    z.write(so, 'lib/arm64-v8a/libkimios.so')
"@
& python -c $pyCode "$OUT\unsigned.apk" "$OUT\dex\classes.dex" "$OUT\libkimios.so"
if ($LASTEXITCODE -ne 0) { Write-Host "  packaging FAILED"; exit 1 }
Write-Host "  OK"

Write-Host "[6/7] zipalign"
& "$BT\zipalign.exe" -f 4 "$OUT\unsigned.apk" "$OUT\aligned.apk" 2>&1 | Select-Object -First 3
if ($LASTEXITCODE -ne 0) { Write-Host "  zipalign FAILED"; exit 1 }
Write-Host "  OK"

Write-Host "[7/7] apksigner (debug key)"
$KS = "$env:USERPROFILE\Downloads\debug.keystore"
& "$BT\apksigner.bat" sign --ks $KS --ks-pass pass:android --key-pass pass:android `
    --ks-key-alias androiddebugkey --out "$OUT\KimiOS-libos.apk" "$OUT\aligned.apk" 2>&1 | Select-Object -First 3
if ($LASTEXITCODE -ne 0) { Write-Host "  signing FAILED"; exit 1 }
& "$BT\apksigner.bat" verify "$OUT\KimiOS-libos.apk" 2>&1 | Select-Object -First 5
Write-Host ""
Write-Host "DONE: $OUT\KimiOS-libos.apk"
Get-Item "$OUT\KimiOS-libos.apk" | ForEach-Object { "  size: $([math]::Round($_.Length/1KB,1)) KB" }
Get-FileHash "$OUT\KimiOS-libos.apk" -Algorithm SHA256 | ForEach-Object { "  sha256: $($_.Hash)" }