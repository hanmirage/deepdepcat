param(
  [string]$ArgsJson
)
$ErrorActionPreference = "Stop"
function Out-JsonLine($obj) {
  $obj | ConvertTo-Json -Compress -Depth 8
}

function Color-Value([int]$rgb) {
  (($rgb -band 0xFF) -shl 16) -bor ($rgb -band 0xFF00) -bor (($rgb -band 0xFF0000) -shr 16)
}

# 由本工具启动的办公窗口固定 1300x900（窗口随文档创建，须在文档就绪后设置）；
# 附加到用户已开的窗口不改布局。新实例窗口对象就绪有竞态，重试几次。
function Set-WinSize([object]$App, [bool]$Launched) {
  if (-not $Launched) { return }
  try { $App.WindowState = 0 } catch { }
  for ($i = 0; $i -lt 3; $i++) {
    try {
      $win = $App.Windows.Item(1)
      $win.Width = 1300
      $win.Height = 900
      return
    } catch { Start-Sleep -Milliseconds 600 }
  }
}

try {
  $config = $ArgsJson | ConvertFrom-Json

  # ProgID candidates by app family.
  $candidates = @()
  if ($config.app -eq "calc") {
    $candidates = @("KET.Application", "Excel.Application")
  } elseif ($config.app -eq "impress") {
    $candidates = @("KWPP.Application", "PowerPoint.Application")
  } elseif ($config.app -eq "word") {
    $candidates = @("Word.Application")
  } elseif ($config.app -eq "wps") {
    $candidates = @("KWPS.Application")
  } else {
    $candidates = @("KWPS.Application", "Word.Application")
  }

  # 1. Attach to a RUNNING instance first (the user's open WPS, or an
  #    instance left by an earlier call) — GetActiveObject, never a fresh
  #    New-Object, so consecutive calls share the same app + documents.
  $app = $null
  $launched = $false
  foreach ($p in $candidates) {
    try {
      $app = [System.Runtime.InteropServices.Marshal]::GetActiveObject($p)
      break
    } catch { }
  }
  # 2. No running instance — create one (it stays alive for later calls).
  if ($null -eq $app) {
    foreach ($p in $candidates) {
      try { $app = New-Object -ComObject $p; $launched = $true; break } catch { }
    }
  }

  if ($null -eq $app) {
    Write-Output '{"error":"NO_OFFICE"}'
    exit 1
  }

  if ($config.action -eq "detect") {
    Write-Output (Out-JsonLine @{ action = "detect"; app = $app.Name; version = $app.Version })
    try { $app.Quit() } catch { }
    exit 0
  }

  $app.Visible = $true

  switch ($config.app) {
    "calc" {
      $wb = $null
      $openedHere = $false
      foreach ($w in $app.Workbooks) {
        if ($w.FullName -eq $config.path) { $wb = $w; break }
      }
      if ($null -eq $wb) { $wb = $app.Workbooks.Open($config.path); $openedHere = $true }
      Set-WinSize $app $launched
      switch ($config.action) {
        "read_cells" {
          if ($config.sheet_name -and ([string]$config.sheet_name).Trim().Length -gt 0) {
            $ws = $wb.Worksheets.Item([string]$config.sheet_name)
          } else {
            $sheetIndex = [int]$config.sheet
            if ($sheetIndex -lt 1) { $sheetIndex = 1 }
            $ws = $wb.Worksheets.Item($sheetIndex)
          }
          $used = $ws.UsedRange
          $rows = @()
          foreach ($row in $used.Rows) {
            $cells = @()
            foreach ($cell in $row.Cells) {
              $cells += [string]$cell.Value2
            }
            $rows += ($cells -join "`t")
          }
          Write-Output (Out-JsonLine @{ action = "read_cells"; sheet = $ws.Name; sheet_index = $ws.Index; rows = $rows; row_count = $used.Rows.Count })
        }
        "read_cell" {
          if ($config.sheet_name -and ([string]$config.sheet_name).Trim().Length -gt 0) {
            $ws = $wb.Worksheets.Item([string]$config.sheet_name)
          } else {
            $sheetIndex = [int]$config.sheet
            if ($sheetIndex -lt 1) { $sheetIndex = 1 }
            $ws = $wb.Worksheets.Item($sheetIndex)
          }
          $cell = $ws.Cells.Item([int]$config.row, [int]$config.col)
          Write-Output (Out-JsonLine @{ action = "read_cell"; value = [string]$cell.Value2; formula = [string]$cell.Formula; sheet = $ws.Name; sheet_index = $ws.Index; ok = $true })
        }
        "list_sheets" {
          $sheets = @()
          foreach ($ws in $wb.Worksheets) {
            $sheets += ("{0}`t{1}`t{2}" -f $ws.Index, $ws.Name, $ws.UsedRange.Rows.Count)
          }
          Write-Output (Out-JsonLine @{ action = "list_sheets"; sheet_count = $wb.Worksheets.Count; sheets = $sheets })
        }
        default {
          Write-Output (Out-JsonLine @{ error = "Unknown calc action: $($config.action)" })
          exit 1
        }
      }
      # 只读探测：工具自己打开的工作簿读完即关闭，避免遗留文件锁；
      # 用户原本已打开的工作簿保持原样。
      if ($openedHere) {
        $app.DisplayAlerts = $false
        try { $wb.Close($false) } catch { }
        $app.DisplayAlerts = $true
        if ($app.Workbooks.Count -eq 0) { try { $app.Quit() } catch { } }
      }
      exit 0
    }
    "impress" {
      $pres = $null
      $openedHere = $false
      foreach ($p in $app.Presentations) {
        if ($p.FullName -eq $config.path) { $pres = $p; break }
      }
      if ($null -eq $pres) { $pres = $app.Presentations.Open($config.path); $openedHere = $true }
      Set-WinSize $app $launched
      switch ($config.action) {
        "read_slides" {
          $slides = @()
          foreach ($slide in $pres.Slides) {
            $texts = @()
            foreach ($shape in $slide.Shapes) {
              if ($shape.HasTextFrame) {
                if ($shape.TextFrame.HasText) {
                  $texts += $shape.TextFrame.TextRange.Text
                }
              }
            }
            $slides += ($texts -join "`n")
          }
          Write-Output (Out-JsonLine @{ action = "read_slides"; count = $pres.Slides.Count; slides = $slides })
        }
        default {
          Write-Output (Out-JsonLine @{ error = "Unknown impress action: $($config.action)" })
          exit 1
        }
      }
      if ($openedHere) {
        $app.DisplayAlerts = $false
        try { $pres.Close() } catch { }
        $app.DisplayAlerts = $true
        if ($app.Presentations.Count -eq 0) { try { $app.Quit() } catch { } }
      }
      exit 0
    }
    default {
      $doc = $null
      $openedHere = $false
      if (-not $config.path -or $config.path -eq "active") {
        try { $doc = $app.ActiveDocument } catch { }
        if ($null -eq $doc) {
          Write-Output '{"error":"No active document open in the office app — provide a path instead."}'
          exit 1
        }
      } else {
        foreach ($d in $app.Documents) {
          if ($d.FullName -eq $config.path) { $doc = $d; break }
        }
        if ($null -eq $doc) { $doc = $app.Documents.Open($config.path); $openedHere = $true }
      }
      Set-WinSize $app $launched
      switch ($config.action) {
        "read" {
          $text = $doc.Content.Text
          Write-Output (Out-JsonLine @{ action = "read"; paragraphs = $doc.Paragraphs.Count; chars = $text.Length; name = $doc.Name; text = $text })
        }
        "read_paragraphs" {
          $from = 1
          $to = 2147483647
          if ($config.from) { $from = [int]$config.from }
          if ($config.to) { $to = [int]$config.to }
          $paras = @()
          $i = 0
          foreach ($p in $doc.Paragraphs) {
            $i++
            if ($i -lt $from -or $i -gt $to) { continue }
            $t = $p.Range.Text -replace "`r|`a", ""
            $paras += ("{0}`t{1}" -f $i, $t)
          }
          Write-Output (Out-JsonLine @{ action = "read_paragraphs"; count = $doc.Paragraphs.Count; paragraphs = $paras })
        }
        default {
          Write-Output (Out-JsonLine @{ error = "Unknown action: $($config.action)" })
          exit 1
        }
      }
      if ($openedHere) {
        $app.DisplayAlerts = $false
        try { $doc.Close(0) } catch { }
        $app.DisplayAlerts = $true
        if ($app.Documents.Count -eq 0) { try { $app.Quit() } catch { } }
      }
      exit 0
    }
  }
} catch {
  Write-Output (Out-JsonLine @{ error = $_.Exception.Message })
  exit 1
}
