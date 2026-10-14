# Downloads the most frequent words per language and trims them to a
# commit-friendly size.
#
#   .\tools\fetch-lexicons.ps1
#
# Source: hermitdave/FrequencyWords, content/2018 (OpenSubtitles corpus).
# The repository is MIT for the code and CC-BY-SA-4.0 for the content; see
# src-tauri/resources/lexicon/SOURCES.md.

param(
    [int]$Top = 10000
)

$ErrorActionPreference = 'Stop'

$languages = @(
    'en', 'ru', 'de', 'fr', 'es', 'it', 'pt', 'pl', 'cs', 'sk',
    'uk', 'bg', 'sr', 'hr', 'sl',
    'tr', 'ro', 'hu',
    'sv', 'da', 'no', 'fi', 'is',
    'nl', 'el', 'ca', 'id', 'vi', 'gl'
)

$base = 'https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018'
$outDir = Join-Path $PSScriptRoot '..\src-tauri\resources\lexicon'
$outDir = [System.IO.Path]::GetFullPath($outDir)
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$client = New-Object System.Net.WebClient

foreach ($lang in $languages) {
    $url = "$base/$lang/${lang}_50k.txt"
    $target = Join-Path $outDir "$lang.txt"

    try {
        $bytes = $client.DownloadData($url)
    } catch {
        Write-Warning "$lang : download failed ($($_.Exception.Message))"
        continue
    }

    $text = [System.Text.Encoding]::UTF8.GetString($bytes)
    # OpenSubtitles lines look like `the 13200962`; the count is ignored.
    $words = New-Object System.Collections.Generic.List[string]
    $seen = New-Object System.Collections.Generic.HashSet[string]

    foreach ($line in ($text -split "`n")) {
        if ($words.Count -ge $Top) { break }

        $word = ($line -split ' ')[0]
        if (-not $word) { continue }

        $word = $word.Normalize([System.Text.NormalizationForm]::FormC).ToLowerInvariant()

        # Keep letters only: numbers and punctuation carry no signal for
        # deciding which language a word belongs to.
        $letters = -join ($word.ToCharArray() | Where-Object { [char]::IsLetter($_) })
        if (-not $letters) { continue }
        if ($letters.Length -gt 24) { continue }

        if ($seen.Add($letters)) { $words.Add($letters) }
    }

    if ($words.Count -eq 0) {
        Write-Warning "$lang : no usable words"
        continue
    }

    [System.IO.File]::WriteAllText(
        $target,
        (($words -join "`n") + "`n"),
        (New-Object System.Text.UTF8Encoding($false))
    )

    '{0}  {1,6} words  {2,7:N0} bytes' -f $lang, $words.Count, (Get-Item $target).Length
}