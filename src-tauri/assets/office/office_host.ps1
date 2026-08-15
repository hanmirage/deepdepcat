# Persistent office COM host: reads one JSON request per line on stdin,
# keeps ONE WPS/Office instance + its visible window alive, and writes a
# JSON response per request on stdout.
#
# Why persistent: WPS repaints its window only for writes made inside the
# process that owns it. A fresh PowerShell per call attaches cross-process,
# so the document data changes but the visible window never repaints.
# Holding the COM instance in one long-lived process makes every write a
# same-process write — the open window updates live.
#
# Calc/impress action bodies live in the dot-sourced sibling scripts.
# Protocol: request line = one JSON config; response line = one JSON object.

[Console]::InputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\ddc_office_host_calc.ps1"
. "$PSScriptRoot\ddc_office_host_impress.ps1"

function Out-JsonLine($obj) { $obj | ConvertTo-Json -Compress -Depth 8 }

# 0xRRGGBB → Word/Excel/PowerPoint long color (BGR for Word/Excel, RGB for PPT).
function Color-Value([int]$rgb) {
  (($rgb -band 0xFF) -shl 16) -bor ($rgb -band 0xFF00) -bor (($rgb -band 0xFF0000) -shr 16)
}

# Collapsed range where inserts land: before paragraph `position` (1-based)
# when given, otherwise at the end of the document.
function New-InsertRange {
  param([object]$Doc, [object]$Cfg)
  if ($Cfg.position) {
    $p = $Doc.Paragraphs.Item([int]$Cfg.position)
    $rng = $p.Range
    $rng.Collapse(1)
    return $rng
  }
  $rng = $Doc.Content
  $rng.Collapse(0)
  return $rng
}

# Insert text as its own paragraph (at position or end) and apply the
# optional paragraph formatting to the inserted range.
function Add-Para([object]$Doc, [object]$Cfg, [string]$Text, [bool]$Format) {
  $rng = New-InsertRange $Doc $Cfg
  $rng.InsertAfter($Text)
  if ($Format) {
    if ($Cfg.bold) { $rng.Font.Bold = $true }
    if ($Cfg.size) { $rng.Font.Size = [double]$Cfg.size }
    if ($Cfg.color) { $rng.Font.Color = Color-Value $Cfg.color }
    if ($Cfg.italic) { $rng.Font.Italic = $true }
    if ($Cfg.underline) { $rng.Font.Underline = 1 }
    if ($Cfg.font_name) { $rng.Font.Name = [string]$Cfg.font_name }
  }
  $rng.InsertParagraphAfter()
}

$app = $null
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { break }
  if ($line.Trim().Length -eq 0) { continue }
  try {
    $config = $line | ConvertFrom-Json
    $candidates = @()
    if ($config.app -eq "calc") { $candidates = @("KET.Application", "Excel.Application") }
    elseif ($config.app -eq "impress") { $candidates = @("KWPP.Application", "PowerPoint.Application") }
    elseif ($config.app -eq "word") { $candidates = @("Word.Application") }
    elseif ($config.app -eq "wps") { $candidates = @("KWPS.Application") }
    else { $candidates = @("KWPS.Application", "Word.Application") }

    $done = $false
    for ($attempt = 0; $attempt -lt 3 -and -not $done; $attempt++) {
      try {
        if ($null -eq $app) {
          $launched = $false
          foreach ($p in $candidates) {
            try { $app = [System.Runtime.InteropServices.Marshal]::GetActiveObject($p); break } catch { }
          }
          if ($null -eq $app) {
            foreach ($p in $candidates) {
              try { $app = New-Object -ComObject $p; $launched = $true; break } catch { }
            }
          }
          if ($null -eq $app) { throw "NO_OFFICE" }
          $app.Visible = $true
        }

        $action = [string]$config.action
        # save_as/export_pdf carry the OUTPUT path in config.path; the
        # document to operate on comes from source_path (or path itself).
        $targetPath = $config.path
        if ($config.source_path) { $targetPath = $config.source_path }
        $toolOpened = $false
        switch ($config.app) {
          "calc" {
            $wb = $null
            foreach ($w in $app.Workbooks) { if ($w.FullName -eq $targetPath) { $wb = $w; break } }
            if ($null -eq $wb) {
              if (Test-Path -LiteralPath $targetPath) { $wb = $app.Workbooks.Open($targetPath); $toolOpened = $true }
              else { $wb = $app.Workbooks.Add(); try { $wb.SaveAs2($targetPath, 51) } catch { $wb.SaveAs($targetPath, 51) }; $toolOpened = $true }
            }
            Invoke-CalcAction $app $wb $config
            # 文档已由 SaveAs2/打开落盘，Save() 仅兜底——WPS 有时对新建工作簿
            # 报"保存失败"，吞掉以免中断后续流程。
            if ($action -ne "export_pdf") { try { $wb.Save() } catch { } }
            # 工具自己打开/新建的文档执行完即保存并关闭，释放文件锁；
            # 用户原本已打开的工作簿保持打开，供实时预览。
            if ($toolOpened) {
              $app.DisplayAlerts = $false
              try { $wb.Close($true) } catch { try { $wb.Close($false) } catch { } }
              $app.DisplayAlerts = $true
              # WPS 有时 Close 不真正释放窗口；工具开过文档且已无文档时
              # 直接退出，避免遗留僵尸窗口与文件锁（用户文档未打开时安全）。
              if ($app.Workbooks.Count -eq 0) { try { $app.Quit() } catch { } }
            }
          }
          "impress" {
            $pres = $null
            foreach ($p in $app.Presentations) { if ($p.FullName -eq $targetPath) { $pres = $p; break } }
            if ($null -eq $pres) {
              if (Test-Path -LiteralPath $targetPath) { $pres = $app.Presentations.Open($targetPath); $toolOpened = $true }
              else { $pres = $app.Presentations.Add(); $pres.SaveAs($targetPath, 1); $toolOpened = $true }
            }
            Invoke-ImpressAction $app $pres $config
            # 文档已由 SaveAs2/打开落盘，Save() 仅兜底——WPS 有时报"保存失败"，
            # 吞掉以免中断后续流程。
            if ($action -ne "export_pdf") { try { $pres.Save() } catch { } }
            if ($toolOpened) {
              $app.DisplayAlerts = $false
              try { $pres.Save(); $pres.Close() } catch { try { $pres.Close() } catch { } }
              $app.DisplayAlerts = $true
              if ($app.Presentations.Count -eq 0) { try { $app.Quit() } catch { } }
            }
          }
          default {
            # writer: if the document is ALREADY open in the app, edit that
            # instance (user's open window) — never a copy. When `path` is
            # omitted or "active", target the USER'S CURRENT DOCUMENT
            # (ActiveDocument) — the agent writes into whatever the user is
            # looking at right now.
            $doc = $null
            if (-not $config.path -or $config.path -eq "active") {
              try { $doc = $app.ActiveDocument } catch { }
              if ($null -eq $doc) { throw "No active document open in the office app - provide a path instead." }
            } else {
              foreach ($d in $app.Documents) { if ($d.FullName -eq $targetPath) { $doc = $d; break } }
              if ($null -eq $doc) {
                if (Test-Path -LiteralPath $targetPath) { $doc = $app.Documents.Open($targetPath); $toolOpened = $true }
                else { $doc = $app.Documents.Add(); $doc.SaveAs2($targetPath, 16); $toolOpened = $true }
              }
            }
            switch ($action) {
              "replace" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                $p.Range.Text = [string]$config.text
                Write-Output (Out-JsonLine @{ action = $action; para = $config.para; ok = $true })
              }
              "insert" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                $p.Range.InsertBefore([string]$config.text + "`r")
                Write-Output (Out-JsonLine @{ action = $action; para = $config.para; ok = $true })
              }
              "delete" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                $p.Range.Delete()
                Write-Output (Out-JsonLine @{ action = $action; para = $config.para; ok = $true })
              }
              "type_text" {
                $text = [string]$config.text
                $chunkSize = 4
                if ($config.chunk) { $chunkSize = [int]$config.chunk }
                $pace = 180
                if ($config.pace) { $pace = [int]$config.pace }
                # Selection.TypeText = real keyboard-style input. WPS
                # repaints the window for it live; Range.InsertAfter does
                # NOT (data updates, window stays stale).
                try { $doc.Activate() } catch { }
                $sel = $app.Selection
                if ($config.para) {
                  try {
                    $doc.Paragraphs.Item([int]$config.para).Range.Select()
                    $sel.Collapse(0)
                  } catch { try { $sel.EndKey(6) | Out-Null } catch { } }
                } else {
                  try { $sel.EndKey(6) | Out-Null } catch { }
                }
                $i = 0
                while ($i -lt $text.Length) {
                  $len = [Math]::Min($chunkSize, $text.Length - $i)
                  $sel.TypeText($text.Substring($i, $len))
                  $i += $len
                  Start-Sleep -Milliseconds $pace
                }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true; chars = $text.Length })
              }
              "replace_all" {
                $find = $doc.Content.Find
                $find.ClearFormatting()
                $find.Replacement.ClearFormatting()
                $count = $find.Execute([string]$config.find, $false, $false, $false, $false, $false, $true, 1, $false, [string]$config.text, 2)
                Write-Output (Out-JsonLine @{ action = $action; found = $count })
              }
              "set_style" {
                if (-not $config.style) {
                  Write-Output (Out-JsonLine @{ error = "set_style requires a 'style' parameter (e.g. heading 1, normal, title)." })
                  $done = $true
                  continue
                }
                $p = $doc.Paragraphs.Item([int]$config.para)
                $const = @{
                  "normal" = -1; "heading 1" = -2; "heading 2" = -3; "heading 3" = -4;
                  "heading 4" = -5; "heading 5" = -6; "heading 6" = -7;
                  "title" = -63; "subtitle" = -64
                }
                $key = ([string]$config.style).ToLower()
                if ($const.ContainsKey($key)) { $p.Style = $const[$key] }
                else { $p.Style = [string]$config.style }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "set_font" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                if ($config.size) { $p.Range.Font.Size = [int]$config.size }
                if ($config.bold) { $p.Range.Font.Bold = $true }
                if ($config.italic) { $p.Range.Font.Italic = $true }
                if ($config.underline) { $p.Range.Font.Underline = 1 }
                if ($config.font_name) { $p.Range.Font.Name = [string]$config.font_name }
                if ($config.color) { $p.Range.Font.Color = Color-Value $config.color }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "add_paragraph" {
                Add-Para $doc $config ([string]$config.text) $true
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "add_heading" {
                $level = 1
                if ($config.level) { $level = [int]$config.level }
                if ($level -lt 1 -or $level -gt 6) { $level = 1 }
                $rng = New-InsertRange $doc $config
                $rng.InsertAfter([string]$config.text)
                $rng.Font.Bold = $true
                $sizes = @(22, 18, 16, 14, 13, 12)
                $rng.Font.Size = $sizes[$level - 1]
                try { $rng.ParagraphFormat.OutlineLevel = $level } catch { }
                $rng.InsertParagraphAfter()
                Write-Output (Out-JsonLine @{ action = $action; ok = $true; level = $level })
              }
              "add_list" {
                if ($config.items -is [Array]) { $items = $config.items } else { $items = @($config.items) }
                $i = 1
                $usePos = $true
                foreach ($item in $items) {
                  $rng = $null
                  if ($usePos) { $rng = New-InsertRange $doc $config; $usePos = $false }
                  else {
                    $rng = $doc.Content
                    $rng.Collapse(0)
                  }
                  $prefix = "• "
                  if ($config.list_style -and ([string]$config.list_style -eq "number")) { $prefix = "$i. " }
                  $rng.InsertAfter($prefix + [string]$item)
                  $rng.InsertParagraphAfter()
                  $i++
                }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true; items = ($items.Count) })
              }
              "add_table" {
                $rowsData = $null
                if ($config.data -is [Array]) { $rowsData = $config.data }
                elseif ($config.data) { $rowsData = @($config.data) }
                $rows = 0
                $cols = 0
                if ($rowsData) {
                  $rows = $rowsData.Count
                  foreach ($r in $rowsData) { if ($r.Count -gt $cols) { $cols = $r.Count } }
                }
                if ($rows -lt 1) { $rows = [int]$config.rows }
                if ($cols -lt 1) { $cols = [int]$config.cols }
                if ($rows -lt 1 -or $cols -lt 1) {
                  Write-Output (Out-JsonLine @{ error = "add_table needs data (2D array) or rows+cols" })
                  $done = $true
                  continue
                }
                $rng = New-InsertRange $doc $config
                $tbl = $doc.Tables.Add($rng, $rows, $cols)
                try { $tbl.Borders.Enable = $true } catch { }
                for ($r = 1; $r -le $rows; $r++) {
                  for ($c = 1; $c -le $cols; $c++) {
                    $v = ""
                    if ($rowsData -and $r -le $rowsData.Count) {
                      $row = $rowsData[$r - 1]
                      if ($c -le $row.Count) { $v = [string]$row[$c - 1] }
                    }
                    $tbl.Cell($r, $c).Range.Text = $v
                  }
                }
                $header = $true
                if ($config.header -is [bool]) { $header = $config.header }
                if ($header -and $rows -ge 1) {
                  try {
                    $shade = 0xF2F2F2
                    if ($config.header_color) { $shade = Color-Value $config.header_color }
                    for ($c = 1; $c -le $cols; $c++) {
                      $cell = $tbl.Cell(1, $c)
                      $cell.Range.Font.Bold = $true
                      $cell.Shading.BackgroundPatternColor = $shade
                    }
                  } catch { }
                }
                try { $tbl.AutoFitBehavior(2) } catch { }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true; rows = $rows; cols = $cols })
              }
              "add_image" {
                $rng = New-InsertRange $doc $config
                $pic = $doc.InlineShapes.AddPicture([string]$config.image_path, $false, $true, $rng)
                if ($config.width_pt) { $pic.LockAspectRatio = 0; $pic.Width = [double]$config.width_pt }
                if ($config.height_pt) { $pic.LockAspectRatio = 0; $pic.Height = [double]$config.height_pt }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "page_break" {
                $rng = New-InsertRange $doc $config
                $rng.InsertBreak(7)
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "set_alignment" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                $map = @{ left = 0; center = 1; right = 2; justify = 3 }
                $key = ([string]$config.align).ToLower()
                if (-not $map.ContainsKey($key)) {
                  Write-Output (Out-JsonLine @{ error = "set_alignment: invalid align (left|center|right|justify)" })
                  $done = $true
                  continue
                }
                $p.Alignment = $map[$key]
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "set_line_spacing" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                $p.LineSpacingRule = 5
                $p.LineSpacing = [double]$config.multiple
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "set_paragraph_format" {
                $p = $doc.Paragraphs.Item([int]$config.para)
                if ($config.space_before) { $p.SpaceBefore = [double]$config.space_before }
                if ($config.space_after) { $p.SpaceAfter = [double]$config.space_after }
                if ($config.first_line_indent) { $p.FirstLineIndent = [double]$config.first_line_indent }
                if ($config.left_indent) { $p.LeftIndent = [double]$config.left_indent }
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "clear_doc" {
                $doc.Content.Delete()
                Write-Output (Out-JsonLine @{ action = $action; ok = $true })
              }
              "save_as" {
                $doc.SaveAs2([string]$config.path, [int]$config.format)
                Write-Output (Out-JsonLine @{ action = $action; path = $config.path; ok = $true })
              }
              "export_pdf" {
                # WPS quirk: ExportAsFixedFormat fails (0x80004005) on a
                # freshly launched instance — retry after a short settle,
                # then fall back to SaveAs2(wdFormatPDF=17).
                try {
                  $doc.ExportAsFixedFormat([string]$config.path, 17)
                } catch {
                  Start-Sleep -Milliseconds 1500
                  try { $doc.ExportAsFixedFormat([string]$config.path, 17) } catch { $doc.SaveAs2([string]$config.path, 17) }
                }
                Write-Output (Out-JsonLine @{ action = $action; path = $config.path; ok = $true })
              }
              default {
                Write-Output (Out-JsonLine @{ error = "Unknown action: $action" })
              }
            }
            # export_pdf leaves WPS's Save() in a failing state (WPS quirk:
            # exporting doesn't modify the doc, so the trailing save is
            # skipped for exports). Save() is belt-and-suspenders anyway —
            # the doc is already on disk via SaveAs2/open — so failures are
            # swallowed (WPS occasionally reports 保存失败 on fresh docs).
            if ($action -ne "export_pdf") { try { $doc.Save() } catch { } }
            if ($toolOpened) {
              $app.DisplayAlerts = $false
              try { $doc.Close(1) } catch { try { $doc.Close(0) } catch { } }
              $app.DisplayAlerts = $true
              if ($app.Documents.Count -eq 0) { try { $app.Quit() } catch { } }
            }
          }
        }
        # Same-process writes repaint the visible window; refresh + bring
        # the window to the foreground so the user sees the typing live.
        if ($launched) {
          # 由本工具启动的办公窗口固定 1300x900（窗口随文档创建，须在文档就绪后设置）；
          # 附加到用户已开的窗口不改布局。新实例窗口对象就绪有竞态，重试几次。
          try { $app.WindowState = 0 } catch { }
          for ($i = 0; $i -lt 3; $i++) {
            try {
              $win = $app.Windows.Item(1)
              $win.Width = 1300
              $win.Height = 900
              break
            } catch { Start-Sleep -Milliseconds 600 }
          }
        }
        try { $app.ScreenRefresh() } catch { }
        try { $app.Activate() } catch { }
        $done = $true
      } catch {
        $msg = $_.Exception.Message
        if ($msg -match "0x800706BA|0x80010108|RPC|disconnect|0xFFF4001A|0x80004005|null-valued expression") {
          # COM server died (RPC disconnect) OR the WPS document object went
          # stale (0xFFF4001A / 0x80004005 — user closed the document window,
          # so the COM reference is dead) OR the app reference was Quit by a
          # previous tool-opened cleanup (null Documents). Drop the app
          # reference so the next attempt reconnects to a live instance and
          # reopens the document.
          $app = $null
          continue
        }
        Write-Output (Out-JsonLine @{ error = $msg })
        $done = $true
      }
    }
  } catch {
    Write-Output (Out-JsonLine @{ error = $_.Exception.Message })
  }
}
