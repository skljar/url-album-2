# ru-board monitor for URL Album 2
# Checks thread every 5 minutes, shows Windows notifications for new posts

$ThreadUrl = "https://forum.ru-board.com/topic.cgi?forum=5&topic=3250&start=860"
$StateFile = "$PSScriptRoot\ruboard-last-seen.txt"
$LogFile   = "$PSScriptRoot\ruboard-monitor.log"

function Write-Log($msg) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg"
    Add-Content -Path $LogFile -Value $line -Encoding UTF8
}

function Show-Toast($title, $body) {
    try {
        Add-Type -AssemblyName System.Windows.Forms
        Add-Type -AssemblyName System.Drawing
        $n = New-Object System.Windows.Forms.NotifyIcon
        $n.Icon = [System.Drawing.SystemIcons]::Information
        $n.BalloonTipTitle = $title
        $n.BalloonTipText  = $body
        $n.BalloonTipIcon  = [System.Windows.Forms.ToolTipIcon]::Info
        $n.Visible = $true
        $n.ShowBalloonTip(8000)
        Start-Sleep -Seconds 3
        $n.Dispose()
    } catch {
        Write-Log "Notification error: $_"
    }
}

function Get-PageHash($url) {
    try {
        $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 20 `
            -Headers @{ "User-Agent" = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124" }
        $html = $r.Content

        # Count message blocks as post count proxy
        $posts = [regex]::Matches($html, 'class="msgtext"').Count

        # Get IDs of individual posts
        $ids = [regex]::Matches($html, 'name="(\d{5,})"') | ForEach-Object { $_.Groups[1].Value }
        if ($ids.Count -eq 0) {
            $ids = [regex]::Matches($html, 'id="p(\d+)"') | ForEach-Object { $_.Groups[1].Value }
        }

        # Hash of tail (last 3000 chars) catches any text change
        $tail = if ($html.Length -gt 3000) { $html.Substring($html.Length - 3000) } else { $html }
        $md5  = [System.Security.Cryptography.MD5]::Create()
        $hash = [BitConverter]::ToString($md5.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($tail))).Replace("-","")

        return @{
            PostCount = $posts
            PostIds   = ($ids | Sort-Object | Select-Object -Unique)
            TailHash  = $hash
            Html      = $html
        }
    } catch {
        Write-Log "Fetch error: $_"
        return $null
    }
}

function Get-LastPost($html) {
    # Try to extract last username + date from HTML
    $matches = [regex]::Matches($html, '(?s)<b>(\w+)</b>[^<]*<span[^>]*>(\d{1,2}:\d{2}\s+\d{2}-\d{2}-\d{4})</span>')
    if ($matches.Count -gt 0) {
        $last = $matches[$matches.Count - 1]
        return "$($last.Groups[1].Value) [$($last.Groups[2].Value)]"
    }
    return ""
}

# ── Main ──────────────────────────────────────────────────────────────────────

Write-Log "Check started"

$page = Get-PageHash $ThreadUrl
if ($null -eq $page) {
    Write-Log "Failed to fetch page"
    exit 1
}

$stateKey = "$($page.TailHash)|$($page.PostCount)"

# Load last state
$lastKey = ""
if (Test-Path $StateFile) {
    $lastKey = (Get-Content $StateFile -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -First 1).Trim()
}

if ($lastKey -eq "") {
    # First run
    Set-Content $StateFile -Value $stateKey -Encoding UTF8
    Write-Log "First run. Posts found: $($page.PostCount). Baseline saved."
    Show-Toast "URL Album Monitor" "Monitor started. Watching ru-board thread."
    exit 0
}

if ($stateKey -ne $lastKey) {
    Write-Log "CHANGE DETECTED. Previous: $lastKey | Current: $stateKey"

    $lastPoster = Get-LastPost $page.Html
    $msg = if ($lastPoster) { "New post by $lastPoster" } else { "New activity in thread" }

    Show-Toast "URL Album - new post on ru-board!" $msg

    Set-Content $StateFile -Value $stateKey -Encoding UTF8
    Write-Log "Notification shown. State updated."
} else {
    Write-Log "No changes. Posts: $($page.PostCount)"
}
