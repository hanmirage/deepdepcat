/**
 * FilePreviewDialog — Claude-Preview-style full-screen file preview overlay.
 *
 * Opens the selected file's content (FilePreviewBody) in a near-full-screen
 * modal on top of the app. The conversation pane is never reflowed — the
 * overlay simply covers it. Esc / overlay click / close button dismisses back
 * to the panel.
 */

import { useTranslation } from "react-i18next";
import { Maximize2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { FileTreeNode } from "@/stores/depworkStore";
import { FilePreviewBody } from "@/components/depwork/viewers/FilePreviewBody";

interface FilePreviewDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  file: FileTreeNode;
}

export function FilePreviewDialog({ open, onOpenChange, file }: FilePreviewDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[90vh] w-[92vw] max-w-[92vw] flex-col p-0">
        <DialogHeader className="flex shrink-0 flex-row items-center gap-2 border-b border-border px-4 py-3">
          <Maximize2 className="h-4 w-4 shrink-0 text-muted-foreground" />
          <DialogTitle className="truncate text-sm font-semibold">
            {file.name}
          </DialogTitle>
          <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground">
            {file.path}
          </span>
        </DialogHeader>
        <div className="min-h-0 flex-1">
          <FilePreviewBody selectedFile={file} />
        </div>
      </DialogContent>
    </Dialog>
  );
}
