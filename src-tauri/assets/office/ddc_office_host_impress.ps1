# Impress (WPS 演示 / PowerPoint) action implementations for the persistent
# office host. Dot-sourced by office_host.ps1; shares Out-JsonLine / Color-Value.
#
# Presentation handling (open/attach/save) lives in office_host.ps1 — these
# functions receive the live $Presentation object and only mutate it.
#
# Coordinates (x/y/width/height) are in POINTS (1 pt = 1/72 inch; a 16:9
# slide is 960x540 pt). Colors are 0xRRGGBB → PowerPoint RGB long.

function Invoke-ImpressAction {
  param(
    [object]$App,
    [object]$Presentation,
    [object]$Config
  )
  $action = [string]$Config.action
  switch ($action) {
    "add_slide" {
      $slide = $Presentation.Slides.Add([int]$Config.index, 1)
      if ($Config.title) { $slide.Shapes.Item(1).TextFrame.TextRange.Text = [string]$Config.title }
      if ($Config.body) { $slide.Shapes.Item(2).TextFrame.TextRange.Text = [string]$Config.body }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "remove_slide" {
      $Presentation.Slides.Item([int]$Config.index).Delete()
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "set_slide_content" {
      $slide = $Presentation.Slides.Item([int]$Config.index)
      $titleDone = $false
      $bodyDone = $false
      foreach ($shape in $slide.Shapes) {
        if (-not $shape.HasTextFrame) { continue }
        $pt = -1
        try { $pt = [int]$shape.PlaceholderFormat.Type } catch { }
        if ($Config.title -and $pt -eq 1 -and -not $titleDone) {
          $shape.TextFrame.TextRange.Text = [string]$Config.title
          $titleDone = $true
        } elseif ($Config.body -and ($pt -eq 2 -or $pt -eq 4) -and -not $bodyDone) {
          $shape.TextFrame.TextRange.Text = [string]$Config.body
          $bodyDone = $true
        }
      }
      if ($Config.title -and -not $titleDone -and $slide.Shapes.Count -ge 1) {
        try { $slide.Shapes.Item(1).TextFrame.TextRange.Text = [string]$Config.title } catch { }
      }
      if ($Config.body -and -not $bodyDone -and $slide.Shapes.Count -ge 2) {
        try { $slide.Shapes.Item(2).TextFrame.TextRange.Text = [string]$Config.body } catch { }
      }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "add_textbox" {
      $slide = $Presentation.Slides.Item([int]$Config.index)
      $left = 50.0
      $top = 50.0
      $w = 300.0
      $h = 100.0
      if ($Config.x) { $left = [double]$Config.x }
      if ($Config.y) { $top = [double]$Config.y }
      if ($Config.width) { $w = [double]$Config.width }
      if ($Config.height) { $h = [double]$Config.height }
      $tb = $slide.Shapes.AddTextbox(1, $left, $top, $w, $h)
      if ($Config.text) { $tb.TextFrame.TextRange.Text = [string]$Config.text }
      if ($Config.font_size) { $tb.TextFrame.TextRange.Font.Size = [double]$Config.font_size }
      if ($Config.bold) { $tb.TextFrame.TextRange.Font.Bold = $true }
      if ($Config.font_color) { $tb.TextFrame.TextRange.Font.Color.RGB = Color-Value $Config.font_color }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "add_shape" {
      $slide = $Presentation.Slides.Item([int]$Config.index)
      $shapes = @{ rectangle = 1; diamond = 4; rounded = 5; triangle = 7; oval = 9; hexagon = 10; heart = 21; arrow_right = 33; pentagon = 51; chevron = 52; star = 92 }
      $type = 1
      if ($Config.shape) {
        $key = ([string]$Config.shape).ToLower()
        if ($shapes.ContainsKey($key)) { $type = $shapes[$key] }
      }
      $left = 50.0
      $top = 50.0
      $w = 300.0
      $h = 100.0
      if ($Config.x) { $left = [double]$Config.x }
      if ($Config.y) { $top = [double]$Config.y }
      if ($Config.width) { $w = [double]$Config.width }
      if ($Config.height) { $h = [double]$Config.height }
      $sh = $slide.Shapes.AddShape($type, $left, $top, $w, $h)
      if ($Config.fill_color) {
        try {
          $sh.Fill.Solid()
          $sh.Fill.ForeColor.RGB = Color-Value $Config.fill_color
        } catch { }
      }
      if ($Config.text) {
        try {
          $sh.TextFrame.TextRange.Text = [string]$Config.text
          if ($Config.font_size) { $sh.TextFrame.TextRange.Font.Size = [double]$Config.font_size }
        } catch { }
      }
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "add_image" {
      $slide = $Presentation.Slides.Item([int]$Config.index)
      $left = 50.0
      $top = 50.0
      $w = 300.0
      $h = 200.0
      if ($Config.x) { $left = [double]$Config.x }
      if ($Config.y) { $top = [double]$Config.y }
      if ($Config.width) { $w = [double]$Config.width }
      if ($Config.height) { $h = [double]$Config.height }
      $pic = $slide.Shapes.AddPicture([string]$Config.image_path, $false, $true, $left, $top, $w, $h)
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "set_slide_bg" {
      $slide = $Presentation.Slides.Item([int]$Config.index)
      $slide.FollowMasterBackground = 0
      $slide.Background.Fill.Solid()
      $slide.Background.Fill.ForeColor.RGB = Color-Value $Config.color
      Write-Output (Out-JsonLine @{ action = $action; ok = $true })
    }
    "save_as" {
      $Presentation.SaveAs([string]$Config.path, [int]$Config.format)
      Write-Output (Out-JsonLine @{ action = $action; path = $Config.path; ok = $true })
    }
    "export_pdf" {
      $Presentation.SaveAs([string]$Config.path, 32)
      Write-Output (Out-JsonLine @{ action = $action; path = $Config.path; ok = $true })
    }
    default {
      Write-Output (Out-JsonLine @{ error = "Unknown impress action: $action" })
    }
  }
}
