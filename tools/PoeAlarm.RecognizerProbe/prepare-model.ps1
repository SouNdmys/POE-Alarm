param(
    [string]$PythonExecutable = "python"
)

$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$toolsRoot = Join-Path $repositoryRoot ".tools"
$modelRoot = Join-Path $toolsRoot "models\chinese_cht_PP-OCRv3_legacy"
$archivePath = Join-Path $toolsRoot "models\chinese_cht_PP-OCRv3_rec_infer_legacy.tar"
$inferenceRoot = Join-Path $modelRoot "chinese_cht_PP-OCRv3_rec_infer"
$converterRoot = Join-Path $toolsRoot "python\paddle2onnx-1.3.1"
$modelUrl = "https://paddleocr.bj.bcebos.com/PP-OCRv3/multilingual/chinese_cht_PP-OCRv3_rec_infer.tar"
$dictionaryUrl = "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/dict/chinese_cht_dict.txt"
$modelArchiveSha256 = "D4ECF6B9C0C055E5112091AE281FCE53EB46081D9E8BD5A3C694B5ED07092977"
$dictionarySha256 = "832551FEE1F2FBC97508772D81EBDC8DBA12C00DE97A35C71C9DDF43DDAC1A83"

function Assert-FileHash {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Expected
    )

    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($actual -ne $Expected) {
        throw "SHA-256 mismatch for '$Path'. Expected $Expected, received $actual."
    }
}

New-Item -ItemType Directory -Force -Path (Split-Path $archivePath), $modelRoot | Out-Null

if (-not (Test-Path -LiteralPath $archivePath)) {
    Invoke-WebRequest -Uri $modelUrl -OutFile $archivePath
}
Assert-FileHash -Path $archivePath -Expected $modelArchiveSha256

if (-not (Test-Path -LiteralPath (Join-Path $inferenceRoot "inference.pdmodel"))) {
    tar -xf $archivePath -C $modelRoot
    if ($LASTEXITCODE -ne 0) {
        throw "tar failed with exit code $LASTEXITCODE."
    }
}

$nativeConverter = Get-ChildItem -LiteralPath $converterRoot -Filter "paddle2onnx_cpp2py_export*.pyd" -Recurse -ErrorAction SilentlyContinue
if ($nativeConverter.Count -ne 1) {
    & $PythonExecutable -m pip install --disable-pip-version-check --target $converterRoot "paddle2onnx==1.3.1"
    if ($LASTEXITCODE -ne 0) {
        throw "Installing Paddle2ONNX failed with exit code $LASTEXITCODE."
    }
}

$dictionaryPath = Join-Path $inferenceRoot "chinese_cht_dict.txt"
if (-not (Test-Path -LiteralPath $dictionaryPath)) {
    Invoke-WebRequest -Uri $dictionaryUrl -OutFile $dictionaryPath
}
Assert-FileHash -Path $dictionaryPath -Expected $dictionarySha256

$outputPath = Join-Path $inferenceRoot "model.onnx"
& $PythonExecutable (Join-Path $PSScriptRoot "convert_legacy_model.py") `
    --converter-root $converterRoot `
    --model (Join-Path $inferenceRoot "inference.pdmodel") `
    --params (Join-Path $inferenceRoot "inference.pdiparams") `
    --output $outputPath
if ($LASTEXITCODE -ne 0) {
    throw "Model conversion failed with exit code $LASTEXITCODE."
}

$outputHash = (Get-FileHash -LiteralPath $outputPath -Algorithm SHA256).Hash
Write-Host "Recognizer model ready: $outputPath"
Write-Host "ONNX SHA-256: $outputHash"
