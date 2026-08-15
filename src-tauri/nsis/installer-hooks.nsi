; DeepDepCat installer hooks — brand-folder auto-suffix.
;
; The installer already shows the directory-choose page (MUI_PAGE_DIRECTORY
; in the tauri NSIS template). By default a user who picks a drive root (D:\)
; would install straight into the root. This hook appends the product folder
; (\DeepDepCat) to whatever directory the user chose, so installation always
; lands inside a folder named after the app.
;
; NOTE: this file is include()'d verbatim — it is NOT run through Tauri's
; template renderer, so handlebars placeholders like ${PRODUCTNAME} would
; stay literal. Keep the product name hardcoded here.
;
; MUI_PAGE_CUSTOMFUNCTION_LEAVE is consumed by the template's directory page
; (declared AFTER this file is included), so the callback wires itself in.

!define MUI_PAGE_CUSTOMFUNCTION_LEAVE EnsureBrandFolder

Function EnsureBrandFolder
  ${GetFileName} "$INSTDIR" $R0
  ${IfNot} $R0 == "DeepDepCat"
    StrCpy $INSTDIR "$INSTDIR\DeepDepCat"
  ${EndIf}
FunctionEnd
