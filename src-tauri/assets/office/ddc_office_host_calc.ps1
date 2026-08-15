# Calc (WPS 表格 / Excel) action implementations for the persistent office
# host. Dot-sourced by office_host.ps1; shares Out-JsonLine / Color-Value.
#
# Workbook handling (open/attach/save) lives in office_host.ps1 — these
# functions receive the live $Workbook object and only mutate it, so every
# write repaints the user's visible window.

function Col-ToNum([string]$letters) {
  $n = 0
  foreach ($ch in $letters.ToUpper().ToCharArray()) {
    $n = $n * 26 + ([int][char]$ch - 64)
  }
  return $n
}

function Col-ToLetters([int]$n) {
  $s = ""
  while ($n -gt 0) {
    $n--
    $s = [char](65 + ($n % 26)) + $s
    $n = [int][math]::Floor($n / 26)
  }
  return $s
}

# Resolve the worksheet by NAME (preferred — stable across add/remove)
# or by 1-based INDEX. Always returns name + index so callers can verify
# where the write actually landed.
function Select-Worksheet {
  param(
    [object]$Workbook,
    [object]$Config
  )
  $name = $null
  $index = 1
  if ($Config.sheet_name -and ([string]$Config.sheet_name).Trim().Length -gt 0) {
    $name = [string]$Config.sheet_name
    $ws = $Workbook.Worksheets.Item($name)
  } else {
    $index = [int]$Config.sheet
    if ($index -lt 1) { $index = 1 }
    $ws = $Workbook.Worksheets.Item($index)
    $name = $ws.Name
  }
  return @{ ws = $ws; name = $name; index = $ws.Index }
}

function Invoke-CalcAction {
  param(
    [object]$App,
    [object]$Workbook,
    [object]$Config
  )
  $action = [string]$Config.action
  switch ($action) {
    "write_cell" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Cells.Item([int]$Config.row, [int]$Config.col).Value2 = [string]$Config.text
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "write_range" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ref = [string]$Config.range_ref
      $m = [regex]::Match($ref, '^([A-Za-z]+)(\d+)$')
      if (-not $m.Success) {
        Write-Output (Out-JsonLine @{ error = "Invalid range_ref: $ref (use A1-style, e.g. A1)" })
        return
      }
      $colStart = Col-ToNum $m.Groups[1].Value
      $rowStart = [int]$m.Groups[2].Value
      if ($Config.data -is [Array]) { $rowsData = $Config.data } else { $rowsData = @($Config.data) }
      $rows = $rowsData.Count
      if ($rows -eq 0) {
        Write-Output (Out-JsonLine @{ action = $action; ok = $true; cells = 0 })
        return
      }
      $cols = 0
      foreach ($r in $rowsData) { if ($r.Count -gt $cols) { $cols = $r.Count } }
      $arr = [object[,]]::new($rows, $cols)
      for ($r = 0; $r -lt $rows; $r++) {
        $row = $rowsData[$r]
        for ($c = 0; $c -lt $cols; $c++) {
          $arr[$r, $c] = [string]$row[$c]
        }
      }
      $endRef = (Col-ToLetters ($colStart + $cols - 1)) + ($rowStart + $rows - 1)
      $range = $ws.Range("$ref`:$endRef")
      try {
        $range.Value2 = $arr
      } catch {
        for ($r = 0; $r -lt $rows; $r++) {
          for ($c = 0; $c -lt $cols; $c++) {
            $ws.Cells.Item($rowStart + $r, $colStart + $c).Value2 = $arr[$r, $c]
          }
        }
      }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; cells = ($rows * $cols); sheet = $t.name; sheet_index = $t.index })
    }
    "set_formula" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Cells.Item([int]$Config.row, [int]$Config.col).Formula = [string]$Config.formula
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "merge_cells" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Range([string]$Config.range_ref).Merge()
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "unmerge_cells" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Range([string]$Config.range_ref).UnMerge()
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "clear_range" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Range([string]$Config.range_ref).ClearContents()
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "add_sheet" {
      $ws = $Workbook.Worksheets.Add()
      if ($Config.name) {
        $try = [string]$Config.name
        $n = 1
        while ($n -lt 100) {
          $exists = $false
          foreach ($w in $Workbook.Worksheets) {
            if ($w.Name -eq $try) { $exists = $true; break }
          }
          if (-not $exists) { break }
          $n++
          $try = [string]$Config.name + $n
        }
        $ws.Name = $try
      }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; name = $ws.Name; index = $ws.Index })
    }
    "rename_sheet" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Name = [string]$Config.name
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $ws.Name; sheet_index = $ws.Index })
    }
    "remove_sheet" {
      if ($Workbook.Worksheets.Count -le 1) {
        Write-Output (Out-JsonLine @{ error = "Cannot remove the last remaining worksheet" })
        return
      }
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $removedName = $ws.Name
      $App.DisplayAlerts = $false
      try { $ws.Delete() } finally { $App.DisplayAlerts = $true }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $removedName })
    }
    "set_column_width" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Columns.Item([string]$Config.col).ColumnWidth = [double]$Config.width
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "set_row_height" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $ws.Rows.Item([int]$Config.row).RowHeight = [double]$Config.height
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "set_cell_style" {
      $t = Select-Worksheet $Workbook $Config
      $ws = $t.ws
      $cell = $ws.Cells.Item([int]$Config.row, [int]$Config.col)
      if ($Config.bold) { $cell.Font.Bold = $true }
      if ($Config.italic) { $cell.Font.Italic = $true }
      if ($Config.font_size) { $cell.Font.Size = [double]$Config.font_size }
      if ($Config.font_color) { $cell.Font.Color = Color-Value $Config.font_color }
      if ($Config.bg_color) { $cell.Interior.Color = Color-Value $Config.bg_color }
      if ($Config.wrap) { $cell.WrapText = $true }
      if ($Config.align) {
        $map = @{ left = -4131; center = -4108; right = -4152; justify = -4130 }
        $key = ([string]$Config.align).ToLower()
        if ($map.ContainsKey($key)) { $cell.HorizontalAlignment = $map[$key] }
      }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true; sheet = $t.name; sheet_index = $t.index })
    }
    "save_as" {
      try { $Workbook.SaveAs2([string]$Config.path, [int]$Config.format) } catch { $Workbook.SaveAs([string]$Config.path, [int]$Config.format) }
      Write-Output (Out-JsonLine @{ action = $action; path = $Config.path; ok = $true })
    }
    "export_pdf" {
      # WPS quirk: ExportAsFixedFormat fails (0x80004005) on a freshly
      # launched instance — retry once after a short settle.
      try {
        $Workbook.ExportAsFixedFormat([string]$Config.path, 0)
      } catch {
        Start-Sleep -Milliseconds 1500
        $Workbook.ExportAsFixedFormat([string]$Config.path, 0)
      }
      Write-Output (Out-JsonLine @{ action = $action; path = $Config.path; ok = $true })
    }
    default {
      Write-Output (Out-JsonLine @{ error = "Unknown calc action: $action" })
    }
  }
}
