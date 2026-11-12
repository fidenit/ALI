# ALI 빌드·실행 스크립트
#   .\run.ps1            빌드해서 실행 (개발용. 데이터는 이 폴더의 data, vault 를 쓴다)
#   .\run.ps1 build      빌드만
#   .\run.ps1 test       테스트
#   .\run.ps1 pack       들고 다닐 수 있는 폴더 portable\ALI 를 만들거나 갱신
#
# portable\ALI 폴더는 그 자체로 완결이다. 통째로 어디에 복사해도 그대로 동작한다.
#   ali.exe         프로그램 (설치 불필요)
#   WebView2Loader.dll   화면 안의 브라우저를 불러오는 파일 (프로그램과 같은 폴더에 있어야 함)
#   key.txt              Claude API 키
#   jev.txt              (선택) TypeSafe Jev 키. 있으면 판단 모델이 켜진다
#   data\                보드 상태와 설정
#   vault\               기록과 기억 (옵시디언 보관함)
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter 'BrechtSanders.WinLibs*' |
    ForEach-Object { Join-Path $_.FullName 'mingw64\bin' } | Where-Object { Test-Path $_ } | Select-Object -First 1
$env:PATH = "$env:USERPROFILE\.cargo\bin;$mingw;$env:PATH"
Set-Location $PSScriptRoot

# 내장 브라우저(WebView2)를 불러오는 작은 DLL. 실행 파일 옆에 있어야 프로그램이 뜬다.
function Copy-Loader($dest) {
    $dll = Get-ChildItem target\release\build -Recurse -Filter WebView2Loader.dll -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\x64\*' } | Select-Object -First 1
    if ($dll) { Copy-Item $dll.FullName $dest -Force }
}

switch ($args[0]) {
    'build' { cargo build --release; Copy-Loader target\release }
    'test'  { cargo test }
    'pack'  {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { exit 1 }
        $out = Join-Path $PSScriptRoot 'portable\ALI'
        $first = -not (Test-Path $out)
        New-Item -ItemType Directory -Force $out | Out-Null
        Get-Process ali -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $out 'ali.exe') } | Stop-Process -Force
        Start-Sleep -Milliseconds 300
        Copy-Item target\release\ali.exe $out -Force
        Copy-Loader $out
        if ($first) {
            # 처음 만들 때만 지금까지의 상태를 옮겨 준다. 그 뒤로는 프로그램만 새것으로 바꾼다.
            foreach ($item in 'key.txt', 'data', 'vault') {
                if (Test-Path $item) { Copy-Item $item $out -Recurse -Force }
            }
        }
        "만들었습니다: $out"
    }
    default { cargo build --release; Copy-Loader target\release; cargo run --release }
}
