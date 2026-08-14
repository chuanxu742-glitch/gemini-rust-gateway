[CmdletBinding()]
param(
    [string]$BaseUrl = "http://127.0.0.1:8000",
    [string]$ApiKey = $env:GEMINI_API_KEY,
    [switch]$Compatibility,
    [switch]$Image,
    [string]$ImagePrompt = "A small matte green robot on a clean white background, product photography",
    [int]$ImageTimeoutSeconds = 960
)

$ErrorActionPreference = "Stop"
$BaseUrl = $BaseUrl.TrimEnd("/")

function Read-EnvValue {
    param([string]$Path, [string]$Name)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    $prefix = "$Name="
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ($line.StartsWith($prefix)) { return $line.Substring($prefix.Length).Trim().Trim('"').Trim("'") }
    }
    return $null
}

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    $ApiKey = Read-EnvValue -Path (Join-Path $PSScriptRoot "..\.env") -Name "GEMINI_API_KEY"
}
if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    throw "No API key found. Run 'gemini-rust-gateway init' or pass -ApiKey."
}

$headers = @{ Authorization = "Bearer $ApiKey" }
function Invoke-GatewayJson {
    param([string]$Method, [string]$Path, [object]$Body)
    $parameters = @{ Method = $Method; Uri = "$BaseUrl$Path"; Headers = $headers; TimeoutSec = 120 }
    if ($null -ne $Body) {
        $parameters.ContentType = "application/json"
        $parameters.Body = $Body | ConvertTo-Json -Depth 10 -Compress
    }
    Invoke-RestMethod @parameters
}

$started = Get-Date
Write-Host "[1/5] Frontend and health"
$page = Invoke-WebRequest -Uri "$BaseUrl/" -UseBasicParsing -TimeoutSec 15
if ($page.StatusCode -ne 200 -or $page.Content -notmatch "Gemini Gateway") { throw "Frontend check failed." }
$health = Invoke-RestMethod -Uri "$BaseUrl/health" -TimeoutSec 15
if ($health.status -ne "ok") { throw "Health check failed." }

Write-Host "[2/5] Readiness"
$ready = Invoke-WebRequest -Uri "$BaseUrl/readyz" -UseBasicParsing -TimeoutSec 30
if ($ready.StatusCode -ne 200) { throw "Gateway is not ready." }

Write-Host "[3/5] Authentication and models"
$models = Invoke-GatewayJson -Method GET -Path "/v1/models"
if (@($models.data).Count -lt 1) { throw "No models returned." }
$model = $models.data[0].id
Write-Host "      model: $model"

Write-Host "[4/5] Non-streaming chat"
$chatWatch = [System.Diagnostics.Stopwatch]::StartNew()
$chat = Invoke-GatewayJson -Method POST -Path "/v1/chat/completions" -Body @{
    model = $model
    messages = @(@{ role = "user"; content = "Reply with a short sentence confirming this Rust gateway chat test works." })
}
$chatWatch.Stop()
$content = [string]$chat.choices[0].message.content
if ([string]::IsNullOrWhiteSpace($content)) { throw "Chat returned no text." }
Write-Host ("      ok in {0:N2}s: {1}" -f $chatWatch.Elapsed.TotalSeconds, $content.Substring(0, [Math]::Min(80, $content.Length)))

if ($Compatibility) {
    Write-Host "[compat] OpenAI Responses API"
    $response = Invoke-GatewayJson -Method POST -Path "/v1/responses" -Body @{
        model = $model; input = "Reply with exactly: rust-responses-ok"
    }
    if ($response.status -ne "completed" -or @($response.output).Count -lt 1) { throw "Responses API check failed." }

    Write-Host "[compat] Gemini v1beta generateContent"
    $geminiHeaders = @{ "x-goog-api-key" = $ApiKey }
    $geminiBody = @{ contents = @(@{ role = "user"; parts = @(@{ text = "Reply with exactly: rust-v1beta-ok" }) }) } | ConvertTo-Json -Depth 10 -Compress
    $gemini = Invoke-RestMethod -Method POST -Uri "$BaseUrl/v1beta/models/$($model):generateContent" -Headers $geminiHeaders -ContentType "application/json" -Body $geminiBody -TimeoutSec 120
    if (@($gemini.candidates).Count -lt 1) { throw "Gemini v1beta check failed." }
}

if ($Image) {
    Write-Host "[5/5] Real image generation (this can take several minutes)"
    $task = Invoke-GatewayJson -Method POST -Path "/v1/generations" -Body @{
        type = "image"; prompt = $ImagePrompt; model = $model; count = 1
    }
    $deadline = (Get-Date).AddSeconds($ImageTimeoutSeconds)
    do {
        Start-Sleep -Seconds 3
        $task = Invoke-GatewayJson -Method GET -Path "/v1/tasks/$($task.id)"
        Write-Host "      $($task.status)  $($task.id)"
        if ($task.status -in @("failed", "interrupted", "rejected", "canceled")) {
            throw "Image task ended as '$($task.status)': $($task.error)"
        }
    } while ($task.status -ne "completed" -and (Get-Date) -lt $deadline)
    if ($task.status -ne "completed") { throw "Image task timed out after $ImageTimeoutSeconds seconds." }
    $images = @($task.result.files | Where-Object { $_.content_type -like "image/*" })
    if ($images.Count -lt 1) { throw "Image task completed without an image artifact." }
    Write-Host "      image: $BaseUrl$($images[0].url)"
} else {
    Write-Host "[5/5] Image test skipped (add -Image to run it)"
}

$elapsed = (Get-Date) - $started
Write-Host ("PASS - smoke test completed in {0:N2}s" -f $elapsed.TotalSeconds) -ForegroundColor Green
